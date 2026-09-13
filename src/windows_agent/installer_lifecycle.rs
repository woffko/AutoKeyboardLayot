//! Narrow installer helper. Never kills processes or changes installed files.
use std::path::{Path, PathBuf};

use windows::{
    Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, ERROR_NO_MORE_FILES, GetLastError, HANDLE, HWND, LPARAM,
            WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                CreateMutexW, MUTEX_MODIFY_STATE, OpenMutexW, OpenProcess, PROCESS_NAME_WIN32,
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
/// process snapshot refuses an already-running same-path instance elsewhere.
/// It is not an atomic cross-session startup lease; do not claim that it is.
fn require_no_foreign_session_target(executable: &Path) -> Result<(), i32> {
    let own_pid = std::process::id();
    let mut own_session = 0;
    unsafe { ProcessIdToSessionId(own_pid, &mut own_session) }.map_err(|_| 11)?;
    let snapshot =
        ProcessHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(|_| 11)?);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    unsafe { Process32FirstW(snapshot.0, &mut entry) }.map_err(|_| 11)?;
    loop {
        let length = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if entry.th32ProcessID != own_pid && name.eq_ignore_ascii_case("AutoKeyboardLayot.exe") {
            let process = ProcessHandle(
                unsafe {
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                        false,
                        entry.th32ProcessID,
                    )
                }
                .map_err(|_| 11)?,
            );
            let state = unsafe { WaitForSingleObject(process.0, 0) };
            if state != WAIT_OBJECT_0 {
                if state != WAIT_TIMEOUT {
                    return Err(11);
                }
                let mut session = 0;
                unsafe { ProcessIdToSessionId(entry.th32ProcessID, &mut session) }
                    .map_err(|_| 11)?;
                if session != own_session {
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
                    let image = String::from_utf16(&buffer[..length as usize]).map_err(|_| 11)?;
                    let actual = PathBuf::from(image).canonicalize().map_err(|_| 11)?;
                    let expected = executable.canonicalize().map_err(|_| 11)?;
                    if foreign_session_target(own_session, session, &expected, &actual) {
                        return Err(12);
                    }
                }
            }
        }
        match unsafe { Process32NextW(snapshot.0, &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code().0 as u32 == (0x80070000 | ERROR_NO_MORE_FILES.0) => break,
            Err(_) => return Err(11),
        }
    }
    Ok(())
}

fn foreign_session_target(
    own_session: u32,
    other_session: u32,
    expected: &Path,
    actual: &Path,
) -> bool {
    own_session != other_session && same_image_path(expected, actual)
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
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
        .map_err(|_| 11)?;
    }
    let image = String::from_utf16(&buffer[..length as usize]).map_err(|_| 11)?;
    let expected = executable.canonicalize().map_err(|_| 11)?;
    let actual = PathBuf::from(image).canonicalize().map_err(|_| 11)?;
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
    fn same_target_in_another_session_is_never_authorized_by_local_guards() {
        let target = Path::new(r"C:\Users\Test\Programs\AutoKeyboardLayot\AutoKeyboardLayot.exe");
        assert!(foreign_session_target(0, 1, target, target));
        assert!(foreign_session_target(2, 1, target, target));
        assert!(!foreign_session_target(1, 1, target, target));
        assert!(!foreign_session_target(
            0,
            1,
            target,
            Path::new(r"C:\Other\AutoKeyboardLayot.exe")
        ));
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
