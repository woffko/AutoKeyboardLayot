//! Narrow installer helper. Never kills processes or changes installed files.
use std::path::{Path, PathBuf};

use windows::{
    Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, HWND, LPARAM,
            STILL_ACTIVE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
        },
        Security::{EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::{
            RemoteDesktop::{
                ProcessIdToSessionId, WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFOW,
                WTSEnumerateProcessesW, WTSFreeMemory,
            },
            Threading::{
                CreateMutexW, GetCurrentProcess, GetExitCodeProcess, MUTEX_MODIFY_STATE,
                OpenMutexW, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
                ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
            },
        },
        UI::WindowsAndMessaging::{
            FindWindowW, GetPropW, GetWindowThreadProcessId, PostMessageW, RemovePropW, SetPropW,
            WM_CLOSE,
        },
    },
    core::{HSTRING, PCWSTR, PWSTR, w},
};

use super::{AGENT_MUTEX, ProcessHandle, WINDOW_CLASS, settings_window};

const SAFE_CLOSE_PROPERTY: PCWSTR = w!("AutoKeyboardLayot.GracefulClose.v1");

pub(super) fn advertise_safe_close(window: HWND) {
    // Failure leaves the capability absent: automatic upgrade then refuses.
    unsafe {
        let _ = SetPropW(window, SAFE_CLOSE_PROPERTY, Some(HANDLE(1usize as _)));
    }
}

pub(super) fn remove_safe_close(window: HWND) {
    unsafe {
        let _ = RemovePropW(window, SAFE_CLOSE_PROPERTY);
    }
}

fn require_owned_lease(name: PCWSTR) -> Result<ProcessHandle, ()> {
    let handle = ProcessHandle(unsafe {
        OpenMutexW(
            SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE,
            false,
            name,
        )
        .map_err(|_| ())?
    });
    match unsafe { WaitForSingleObject(handle.0, 0) } {
        WAIT_TIMEOUT => Ok(handle),
        WAIT_OBJECT_0 | WAIT_ABANDONED => {
            unsafe {
                let _ = ReleaseMutex(handle.0);
            }
            Err(())
        }
        _ => Err(()),
    }
}

/// Explicit setup-only first-profile initialization. Existing unified or
/// legacy profiles are preserved byte-for-byte; errors never imply reset.
pub fn initialize_installation() -> i32 {
    let Ok(_agent_lease) = require_owned_lease(AGENT_MUTEX) else {
        return 20;
    };
    let Ok(_settings_lease) = require_owned_lease(settings_window::SETTINGS_MUTEX) else {
        return 20;
    };
    let Some(directory) = super::configuration_directory() else {
        return 20;
    };
    let Ok(_guard) = super::ConfigurationWriteGuard::acquire() else {
        return 20;
    };
    if let Err(error) = autokeyboardlayot::installer_profile::initialize_if_absent(
        &directory,
        super::write_configuration_file_atomically,
    ) {
        return if error.kind() == std::io::ErrorKind::Unsupported {
            22
        } else {
            21
        };
    }
    0
}

/// Exit codes are an installer protocol, not localized diagnostic text:
/// 0 = processes exited; 10 = invalid target; 11 = identity/query failure;
/// 12 = close refused, busy, startup in progress, or wait failed.
/// The caller must acquire and HOLD both singleton mutexes after success,
/// checking ERROR_ALREADY_EXISTS, before changing files. This closes the race
/// with a newly starting instance; success alone is not an installation lease.
pub fn prepare_upgrade(directory: &Path) -> i32 {
    if !directory.is_absolute() {
        return 10;
    }
    let executable = directory.join("AutoKeyboardLayot.exe");
    if let Err(code) = require_no_foreign_session_target(&executable) {
        return code;
    }
    let settings_class = HSTRING::from(settings_window::SETTINGS_WINDOW_CLASS);
    for (class, mutex) in [
        (
            PCWSTR(settings_class.as_ptr()),
            settings_window::SETTINGS_MUTEX,
        ),
        (WINDOW_CLASS, AGENT_MUTEX),
    ] {
        if let Err(code) = close_instance(class, mutex, &executable) {
            return code;
        }
    }
    require_no_foreign_session_target(&executable).map_or_else(|code| code, |()| 0)
}

/// Window lookup and Local mutexes cannot see another session. This read-only
/// process list refuses this user's already-running same-path instance elsewhere.
/// It is not an atomic cross-session startup lease; do not claim that it is.
fn require_no_foreign_session_target(executable: &Path) -> Result<(), i32> {
    let Some(expected) = installed_target(executable)? else {
        return Ok(());
    };
    let own_pid = std::process::id();
    let mut own_session = 0;
    unsafe { ProcessIdToSessionId(own_pid, &mut own_session) }.map_err(|_| 11)?;
    let own_user = current_token_user()?;
    let own_sid = unsafe { (*own_user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let processes = SessionProcesses::enumerate()?;
    for row in processes.rows() {
        if row.pProcessName.is_null() {
            continue;
        }
        let name = unsafe { row.pProcessName.to_string() }.map_err(|_| 11)?;
        let process = RunningProcess {
            id: row.ProcessId,
            session: row.SessionId,
            name: &name,
            // The list carries the SID only of processes this user may query, never another
            // user's (unless setup runs elevated, when EqualSid tells them apart).
            same_user: !row.pUserSid.0.is_null()
                && unsafe { EqualSid(own_sid, row.pUserSid) }.is_ok(),
        };
        if !needs_inspection(own_pid, own_session, &process) {
            continue;
        }
        // Only the limited information: Windows grants it to this user also for an elevated
        // process, which SYNCHRONIZE is not; a copy of another image is never asked for more.
        let handle =
            match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, row.ProcessId) } {
                Ok(handle) => ProcessHandle(handle),
                // The process ended after the list was taken.
                Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => continue,
                Err(_) => return Err(11),
            };
        if runs_target(
            &expected,
            || canonical_existing(&image_path(&handle)?),
            || still_running(&handle),
        )? {
            return Err(12);
        }
    }
    Ok(())
}

/// The canonical install target, or None while nothing is installed there: no process
/// runs an executable that does not exist, so a first installation compares nothing.
fn installed_target(executable: &Path) -> Result<Option<PathBuf>, i32> {
    match executable.canonicalize() {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(11),
    }
}

struct RunningProcess<'a> {
    id: u32,
    session: u32,
    name: &'a str,
    same_user: bool,
}

/// Only this user's instances in other sessions can hold the install directory unseen
/// by the window and mutex checks. Another user's processes cannot be opened by this
/// user and run that user's own copy, so they never block this user's setup.
fn needs_inspection(own_pid: u32, own_session: u32, process: &RunningProcess) -> bool {
    process.id != own_pid
        && process.session != own_session
        && process.same_user
        && process.name.eq_ignore_ascii_case("AutoKeyboardLayot.exe")
}

/// WTSEnumerateProcessesW rows, freed on drop.
struct SessionProcesses {
    rows: *mut WTS_PROCESS_INFOW,
    count: u32,
}
impl SessionProcesses {
    fn enumerate() -> Result<Self, i32> {
        let mut rows = std::ptr::null_mut();
        let mut count = 0;
        unsafe {
            WTSEnumerateProcessesW(Some(WTS_CURRENT_SERVER_HANDLE), 0, 1, &mut rows, &mut count)
        }
        .map_err(|_| 11)?;
        Ok(Self { rows, count })
    }
    fn rows(&self) -> &[WTS_PROCESS_INFOW] {
        if self.rows.is_null() {
            return &[];
        }
        // WTSEnumerateProcessesW returned `count` rows that live until WTSFreeMemory.
        unsafe { std::slice::from_raw_parts(self.rows, self.count as usize) }
    }
}
impl Drop for SessionProcesses {
    fn drop(&mut self) {
        if !self.rows.is_null() {
            unsafe { WTSFreeMemory(self.rows.cast()) };
        }
    }
}

/// TOKEN_USER of this process, in a pointer-aligned buffer; its SID points into it.
fn current_token_user() -> Result<Vec<usize>, i32> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|_| 11)?;
        let token = ProcessHandle(token);
        let mut required = 0u32;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut required);
        if (required as usize) < std::mem::size_of::<TOKEN_USER>() {
            return Err(11);
        }
        let mut buffer = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            required,
            &mut required,
        )
        .map_err(|_| 11)?;
        Ok(buffer)
    }
}

/// Whether one of this user's processes in another session runs the install target. `image`
/// gives its executable's canonical path (None when that path does not resolve here), `running`
/// whether it still runs.
fn runs_target(
    expected: &Path,
    image: impl FnOnce() -> Result<Option<PathBuf>, i32>,
    running: impl FnOnce() -> Result<bool, i32>,
) -> Result<bool, i32> {
    let Some(actual) = image()? else {
        // The target exists (installed_target), so a path that does not resolve is another copy.
        return Ok(false);
    };
    if !same_image_path(expected, &actual) {
        return Ok(false);
    }
    running()
}

/// The canonical form of a process's image path; None when it does not resolve here.
fn canonical_existing(image: &str) -> Result<Option<PathBuf>, i32> {
    match PathBuf::from(image).canonicalize() {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(11),
    }
}

/// Whether a process opened for its limited information still runs.
fn still_running(process: &ProcessHandle) -> Result<bool, i32> {
    let mut code = 0;
    unsafe { GetExitCodeProcess(process.0, &mut code) }.map_err(|_| 11)?;
    Ok(code == STILL_ACTIVE.0 as u32)
}

fn image_path(process: &ProcessHandle) -> Result<String, i32> {
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(|_| 11)?;
    String::from_utf16(&buffer[..length as usize]).map_err(|_| 11)
}

fn close_instance(class: PCWSTR, mutex: PCWSTR, executable: &Path) -> Result<(), i32> {
    let Ok(window) = (unsafe { FindWindowW(class, None) }) else {
        return require_no_instance(mutex);
    };
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
    if process_id == 0 {
        return Err(11);
    }
    let process = ProcessHandle(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            process_id,
        )
        .map_err(|_| 11)?
    });
    let actual = canonical_existing(&image_path(&process)?)?.ok_or(11)?;
    let expected = executable.canonicalize().map_err(|_| 11)?;
    if !same_image_path(&expected, &actual) {
        // A portable/debug copy or unproven window is outside the install scope.
        return Err(11);
    }
    let mut current_process_id = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut current_process_id)) };
    if current_process_id != process_id {
        return Err(11);
    }
    if unsafe { GetPropW(window, SAFE_CLOSE_PROPERTY) }.0 as usize != 1 {
        // Old versions may discard drafts or retained input on WM_CLOSE.
        // Do not send any close message without the versioned capability.
        return Err(12);
    }
    unsafe {
        PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).map_err(|_| 12)?;
        if WaitForSingleObject(process.0, 15_000) != WAIT_OBJECT_0 {
            return Err(12);
        }
    }
    require_no_instance(mutex)
}

fn same_image_path(expected: &Path, actual: &Path) -> bool {
    expected
        .to_str()
        .zip(actual.to_str())
        .is_some_and(|(expected, actual)| expected.eq_ignore_ascii_case(actual))
}

fn require_no_instance(name: PCWSTR) -> Result<(), i32> {
    let _probe = ProcessHandle(unsafe { CreateMutexW(None, false, name).map_err(|_| 12)? });
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        Err(12)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_users_instances_in_other_sessions_are_inspected() {
        let process = |id, session, name, same_user| RunningProcess {
            id,
            session,
            name,
            same_user,
        };
        // Own session 1, own process 40.
        for (row, inspected) in [
            (process(7, 0, "AutoKeyboardLayot.exe", true), true),
            (process(8, 2, "autokeyboardlayot.EXE", true), true),
            // Another signed-in user's agent: no SID, no handle, never a reason to refuse.
            (process(9, 2, "AutoKeyboardLayot.exe", false), false),
            (process(10, 1, "AutoKeyboardLayot.exe", true), false),
            (process(40, 0, "AutoKeyboardLayot.exe", true), false),
            (process(11, 0, "AutoKeyboardLayot-old.exe", true), false),
        ] {
            assert_eq!(needs_inspection(40, 1, &row), inspected, "{}", row.id);
        }
    }

    #[test]
    fn only_a_running_process_of_the_target_image_blocks_setup() {
        let target = Path::new(
            "C:\\Users\\u\\AppData\\Local\\Programs\\AutoKeyboardLayot\\AutoKeyboardLayot.exe",
        );
        let refused = || -> Result<bool, i32> { Err(11) };
        // A copy whose path does not resolve in this session (a deleted folder, another
        // session's drive letter) is not the target, which exists; whether it runs is not asked.
        assert_eq!(runs_target(target, || Ok(None), refused), Ok(false));
        // Another copy is never opened for more than its image path.
        let portable = PathBuf::from("D:\\tools\\AutoKeyboardLayot.exe");
        assert_eq!(
            runs_target(target, || Ok(Some(portable.clone())), refused),
            Ok(false)
        );
        // The target itself blocks while it runs.
        assert_eq!(
            runs_target(target, || Ok(Some(target.to_path_buf())), || Ok(true)),
            Ok(true)
        );
        assert_eq!(
            runs_target(target, || Ok(Some(target.to_path_buf())), || Ok(false)),
            Ok(false)
        );
        // An image path that cannot be read at all still fails closed.
        assert_eq!(runs_target(target, || Err(11), || Ok(true)), Err(11));
    }

    #[test]
    fn an_image_path_that_does_not_resolve_is_no_path() {
        let directory = tempfile::tempdir().unwrap();
        let gone = directory.path().join("gone").join("AutoKeyboardLayot.exe");
        assert_eq!(canonical_existing(gone.to_str().unwrap()), Ok(None));
        let here = directory.path().join("AutoKeyboardLayot.exe");
        std::fs::write(&here, b"").unwrap();
        assert_eq!(
            canonical_existing(here.to_str().unwrap()),
            Ok(Some(here.canonicalize().unwrap()))
        );
    }

    #[test]
    fn a_first_installation_has_no_target_to_compare() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("AutoKeyboardLayot.exe");
        assert_eq!(installed_target(&executable), Ok(None));
        assert_eq!(require_no_foreign_session_target(&executable), Ok(()));
        std::fs::write(&executable, b"").unwrap();
        assert_eq!(
            installed_target(&executable),
            Ok(Some(executable.canonicalize().unwrap()))
        );
        // The real process list (this user's agent may run in this session) holds no
        // instance of a fresh temporary copy.
        assert_eq!(require_no_foreign_session_target(&executable), Ok(()));
    }

    #[test]
    fn initialization_requires_mutexes_owned_by_another_thread() {
        let name = format!("Local\\AutoKeyboardLayot.Test.InstallLease.{}", unsafe {
            windows::Win32::System::Threading::GetCurrentProcessId()
        });
        let wide = HSTRING::from(&name);
        assert!(require_owned_lease(PCWSTR(wide.as_ptr())).is_err());
        let unowned = ProcessHandle(unsafe { CreateMutexW(None, false, &wide).unwrap() });
        assert!(require_owned_lease(PCWSTR(wide.as_ptr())).is_err());
        drop(unowned);
        let (ready, receive_ready) = std::sync::mpsc::sync_channel(1);
        let (release, receive_release) = std::sync::mpsc::sync_channel::<()>(1);
        let owner = std::thread::spawn(move || {
            let wide = HSTRING::from(name);
            let handle = ProcessHandle(unsafe { CreateMutexW(None, true, &wide).unwrap() });
            ready.send(()).unwrap();
            let _ = receive_release.recv_timeout(std::time::Duration::from_secs(5));
            unsafe {
                ReleaseMutex(handle.0).unwrap();
            }
        });
        receive_ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let borrowed = require_owned_lease(PCWSTR(wide.as_ptr())).unwrap();
        release.send(()).unwrap();
        owner.join().unwrap();
        drop(borrowed);
        assert!(require_owned_lease(PCWSTR(wide.as_ptr())).is_err());
    }

    #[test]
    fn relative_target_is_rejected_before_any_window_request() {
        assert_eq!(prepare_upgrade(Path::new("relative")), 10);
    }

    #[test]
    fn helper_never_authorizes_another_binary_or_directory() {
        let expected = Path::new(r"C:\Users\Test\Programs\AutoKeyboardLayot\AutoKeyboardLayot.exe");
        assert!(same_image_path(expected, expected));
        assert!(same_image_path(
            expected,
            Path::new(r"c:\users\test\programs\autokeyboardlayot\autokeyboardlayot.exe")
        ));
        assert!(!same_image_path(
            expected,
            Path::new(r"C:\Other\AutoKeyboardLayot.exe")
        ));
        assert!(!same_image_path(
            expected,
            Path::new(r"C:\Users\Test\Programs\AutoKeyboardLayot\Other.exe")
        ));
        let unicode = Path::new(r"C:\Users\Test\Программы\AutoKeyboardLayot.exe");
        assert!(same_image_path(unicode, unicode));
    }
}
