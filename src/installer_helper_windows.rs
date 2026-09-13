//! Native helper protocol v1. Missing stores use a session-only empty preview.
use autokeyboardlayot::{
    installer_protocol::{Command, RequestDecoder},
    installer_session::{InstallerSession, SessionError},
    installer_worker::InstallerWorker,
    language_package::PackageTrust,
    package_catalog::DEFAULT_PACKAGE_REPOSITORY,
    package_download::{self, DownloadError},
    package_install::{InstallError, PreparedCatalog},
    package_store::{PackageStore, StoreError},
};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{
            MUTEX_MODIFY_STATE, OpenMutexW, OpenProcess, PROCESS_SYNCHRONIZE, ReleaseMutex,
            SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
        },
    },
    core::w,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn clock() -> std::result::Result<u64, DownloadError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| DownloadError::Clock)
}
fn plain_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err("absolute path required".into());
    }
    for part in path.ancestors() {
        if fs::symlink_metadata(part)?.file_attributes() & 0x400 != 0 {
            return Err("reparse path refused".into());
        }
    }
    Ok(())
}
fn plain_future_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("invalid destination".into());
    }
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => {
                return Err("reparse destination".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    plain_path(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x00200000)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 || metadata.len() > limit {
        return Err("request type/size".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("input size".into());
    }
    Ok(bytes)
}
fn publish(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    plain_path(root)?;
    let target = root.join(name);
    let temporary = root.join(format!("{name}.writing"));
    if target.try_exists()? {
        return Err("response exists".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, target)?;
    Ok(())
}
fn leases() -> std::result::Result<Vec<Handle>, InstallError> {
    let mut handles = Vec::new();
    for name in [
        w!("Local\\AutoKeyboardLayot.Agent"),
        w!("Local\\AutoKeyboardLayot.Settings.Singleton"),
    ] {
        let handle = Handle(
            unsafe {
                OpenMutexW(
                    SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE,
                    false,
                    name,
                )
            }
            .map_err(|_| StoreError::Busy)?,
        );
        match unsafe { WaitForSingleObject(handle.0, 0) } {
            WAIT_TIMEOUT => handles.push(handle),
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                unsafe {
                    let _ = ReleaseMutex(handle.0);
                }
                return Err(StoreError::Busy.into());
            }
            _ => return Err(StoreError::Busy.into()),
        }
    }
    Ok(handles)
}

pub fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        return Err("usage: helper PARENT_PID NEW_SESSION_DIRECTORY STORE_ROOT SESSION_ID".into());
    }
    let pid: u32 = args[0].to_str().ok_or("pid")?.parse()?;
    if pid == 0 || pid == std::process::id() {
        return Err("invalid parent".into());
    }
    let parent = Handle(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }?);
    if unsafe { WaitForSingleObject(parent.0, 0) } != WAIT_TIMEOUT {
        return Err("parent unavailable".into());
    }
    let directory = Path::new(&args[1]);
    plain_path(directory.parent().ok_or("session parent")?)?;
    let store_root = Path::new(&args[2]);
    plain_future_path(store_root)?;
    if store_root.starts_with(directory) || directory.starts_with(store_root) {
        return Err("overlapping store/session paths".into());
    }
    let trust = PackageTrust::release()?;
    let mut real_seen = store_root.try_exists()?;
    if real_seen {
        PackageStore::open(store_root)?.load(&trust)?;
    }
    let identity = args[3].to_str().ok_or("session id")?;
    let mut decoder =
        RequestDecoder::new(identity.to_owned()).map_err(|e| format!("protocol: {e:?}"))?;
    fs::create_dir(directory)?; // exclusive fresh session; never resume prior requests
    let preview = if real_seen {
        None
    } else {
        Some(PackageStore::initialize(&directory.join("preview-store"))?)
    };
    let mut session = InstallerSession::default();
    let mut worker = InstallerWorker::default();
    let result = (|| -> Result<()> {
        publish(
            directory,
            "reply-0.ini",
            format!(
                "[helper]\r\nformat=1\r\nsession={identity}\r\nstate=ready\r\npid={}\r\n",
                std::process::id()
            )
            .as_bytes(),
        )?;
        let mut sequence = 1u64;
        let mut last_activity = Instant::now();
        let mut outcome = "none";
        loop {
            if let Some(result) = worker.poll(&mut session) {
                outcome = match result {
                    Ok(()) => "ok",
                    Err(SessionError::Install(InstallError::Store(
                        StoreError::CommitUncertain,
                    ))) => "commit_uncertain",
                    Err(_) => "failed",
                };
            }
            if unsafe { WaitForSingleObject(parent.0, 0) } != WAIT_TIMEOUT
                || last_activity.elapsed() > Duration::from_secs(300)
            {
                break;
            }
            let request = directory.join(format!("request-{sequence}.json"));
            if !request.try_exists()? {
                thread::sleep(Duration::from_millis(25));
                continue;
            }
            let command = decoder
                .accept(&read_bounded(&request, 8192)?)
                .map_err(|e| format!("protocol: {e:?}"))?;
            let ids = if matches!(command, Command::Select { .. }) {
                Some(
                    command
                        .selected_ids()
                        .map_err(|e| format!("selection: {e:?}"))?,
                )
            } else {
                None
            };
            last_activity = Instant::now();
            if matches!(command, Command::Close {}) {
                break;
            }
            let operation = (|| -> Result<()> {
                match command {
                    Command::CheckCatalog { local_file } => {
                        // Once observed, disappearance of a real store never
                        // downgrades this session back to the empty preview.
                        let store = if store_root.try_exists()? {
                            real_seen = true;
                            PackageStore::open(store_root)?
                        } else if !real_seen {
                            preview.clone().ok_or("missing preview")?
                        } else {
                            return Err("real store disappeared".into());
                        };
                        let trust = trust.clone();
                        worker
                            .start_catalog(&mut session, move |cancel| {
                                if let Some(path) = local_file {
                                    let raw = read_bounded(Path::new(&path), 1024 * 1024)
                                        .map_err(|_| StoreError::InvalidFile)?;
                                    PreparedCatalog::from_bytes(
                                        raw,
                                        DEFAULT_PACKAGE_REPOSITORY,
                                        &store.load(&trust)?,
                                        &trust,
                                        clock()?,
                                    )
                                } else {
                                    PreparedCatalog::from_github(
                                        &store,
                                        DEFAULT_PACKAGE_REPOSITORY,
                                        &trust,
                                        cancel,
                                    )
                                }
                            })
                            .map_err(|e| format!("check: {e:?}"))?;
                        outcome = "pending";
                    }
                    Command::Select { view, .. } => {
                        if worker.busy() {
                            return Err("busy".into());
                        }
                        session
                            .replace_selection(view, ids.ok_or("ids")?, &trust, clock()?)
                            .map_err(|e| format!("selection: {e:?}"))?;
                    }
                    Command::ConfirmDownload { view } => {
                        if worker.busy() || view != session.view() {
                            return Err("stale or busy".into());
                        }
                        let _leases = leases()?;
                        let store = PackageStore::open(store_root)?;
                        real_seen = true;
                        if let Some((ticket, selected)) = session
                            .begin_download(&trust, clock()?)
                            .map_err(|e| format!("approval: {e:?}"))?
                        {
                            let trust = trust.clone();
                            worker
                                .start_download(&mut session, ticket, move |cancel| {
                                    let _leases = leases()?;
                                    let deadline = Instant::now() + Duration::from_secs(600);
                                    selected.accept_and_download_with(
                                        &store,
                                        &trust,
                                        cancel,
                                        || {
                                            if Instant::now() >= deadline {
                                                Err(DownloadError::Deadline)
                                            } else {
                                                clock()
                                            }
                                        },
                                        |pin| package_download::download(pin, &trust, cancel),
                                    )
                                })
                                .map_err(|e| format!("download: {e:?}"))?;
                            outcome = "pending";
                        }
                    }
                    Command::ConfirmInstall { view } => {
                        if worker.busy() || view != session.view() {
                            return Err("stale or busy".into());
                        }
                        let _leases = leases()?;
                        let store = PackageStore::open(store_root)?;
                        let ticket = session.review().map_err(|e| format!("review: {e:?}"))?.0;
                        let prepared = session
                            .begin_install(ticket)
                            .map_err(|e| format!("approval: {e:?}"))?;
                        let trust = trust.clone();
                        worker
                            .start_install(&mut session, ticket, move || {
                                let _leases = leases()?;
                                prepared.confirm(&store, &trust, clock()?).map(|_| ())
                            })
                            .map_err(|e| format!("install: {e:?}"))?;
                        outcome = "pending";
                    }
                    Command::Cancel {} => {
                        worker
                            .cancel(&mut session)
                            .map_err(|e| format!("cancel: {e:?}"))?;
                        outcome = "cancelled";
                    }
                    Command::Poll {} => {}
                    Command::Close {} => unreachable!(),
                }
                Ok(())
            })();
            let mut reply = format!(
                "[helper]\r\nformat=1\r\nsession={identity}\r\nsequence={sequence}\r\nview={}\r\nstate={}\r\nbusy={}\r\nresult={}\r\noperation={}\r\n",
                session.view(),
                session.phase(),
                u8::from(worker.busy()),
                outcome,
                if operation.is_ok() { "ok" } else { "rejected" }
            );
            if let Ok(selection) = session.selection() {
                reply.push_str(&selection.page_data()?);
            }
            // Authenticated review files are separate from the INI control data.
            if let Ok((_, review)) = session.review() {
                reply.push_str(&format!(
                    "\r\n[review]\r\ncount={}\r\n",
                    review.packages().len()
                ));
                for (index, downloaded) in review.packages().iter().enumerate() {
                    let package = downloaded.package();
                    let name = format!("review-{}-{index}.txt", session.view());
                    if !directory.join(&name).try_exists()? {
                        publish(
                            directory,
                            &name,
                            format!(
                                "{}\r\n\r\n{}\r\n\r\n{}",
                                package.id(),
                                package.license(),
                                package.notice()
                            )
                            .as_bytes(),
                        )?;
                    }
                    reply.push_str(&format!("file{index}={name}\r\n"));
                }
            }
            publish(
                directory,
                &format!("reply-{sequence}.ini"),
                reply.as_bytes(),
            )?;
            sequence = sequence.checked_add(1).ok_or("sequence exhausted")?;
            if sequence > 4096 {
                return Err("session request limit".into());
            }
        }
        Ok(())
    })();
    let _ = worker.cancel(&mut session);
    while worker.busy() {
        let _ = worker.poll(&mut session);
        thread::sleep(Duration::from_millis(25));
    }
    result
}
