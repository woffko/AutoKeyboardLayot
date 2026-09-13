//! One retained background operation for the out-of-process installer helper.
//! Cancellation never frees the worker slot until its thread actually exits.
use crate::{
    installer_session::{InstallerSession, SessionError, Ticket},
    package_install::{InstallError, PreparedCatalog, PreparedOnlineInstall},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

enum Completion {
    Catalog(Result<PreparedCatalog, InstallError>),
    Download(Result<PreparedOnlineInstall, InstallError>),
    Install(Result<(), InstallError>),
}

struct Pending {
    ticket: Ticket,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Completion>,
}

#[derive(Default)]
pub struct InstallerWorker {
    pending: Option<Pending>,
}

impl InstallerWorker {
    pub fn start_install(
        &mut self,
        session: &mut InstallerSession,
        ticket: Ticket,
        work: impl FnOnce() -> Result<(), InstallError> + Send + 'static,
    ) -> Result<(), SessionError> {
        if self.busy() || !session.is_installing(ticket) {
            return Err(SessionError::WrongState);
        }
        self.spawn(ticket, move |_| Completion::Install(work()))
            .inspect_err(|_| {
                let _ = session.finish_install(ticket);
            })
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Reserve session state only after checking that no earlier worker exists.
    pub fn start_catalog(
        &mut self,
        session: &mut InstallerSession,
        work: impl FnOnce(&AtomicBool) -> Result<PreparedCatalog, InstallError> + Send + 'static,
    ) -> Result<(), SessionError> {
        if self.busy() {
            return Err(SessionError::WrongState);
        }
        let ticket = session.begin_catalog()?;
        self.spawn(ticket, move |cancel| Completion::Catalog(work(cancel)))
            .inspect_err(|_| {
                let _ = session.cancel();
            })
    }

    /// The caller obtains this ticket only by consuming explicit download approval.
    pub fn start_download(
        &mut self,
        session: &mut InstallerSession,
        ticket: Ticket,
        work: impl FnOnce(&AtomicBool) -> Result<PreparedOnlineInstall, InstallError> + Send + 'static,
    ) -> Result<(), SessionError> {
        if self.busy() || !session.is_downloading(ticket) {
            return Err(SessionError::WrongState);
        }
        self.spawn(ticket, move |cancel| Completion::Download(work(cancel)))
            .inspect_err(|_| {
                let _ = session.cancel();
            })
    }

    fn spawn(
        &mut self,
        ticket: Ticket,
        work: impl FnOnce(&AtomicBool) -> Completion + Send + 'static,
    ) -> Result<(), SessionError> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let handle = thread::Builder::new()
            .name("installer-package-worker".into())
            .spawn(move || work(&worker_cancel))
            .map_err(|_| SessionError::WrongState)?;
        self.pending = Some(Pending {
            ticket,
            cancel,
            handle,
        });
        Ok(())
    }

    /// Nonblocking status check; no join until is_finished confirms termination.
    /// Cancelled results are never delivered, even if the worker returned success.
    pub fn poll(&mut self, session: &mut InstallerSession) -> Option<Result<(), SessionError>> {
        if !self.pending.as_ref()?.handle.is_finished() {
            return None;
        }
        let pending = self.pending.take().expect("observed pending operation");
        let result = pending.handle.join();
        if pending.cancel.load(Ordering::Acquire) {
            return Some(Err(SessionError::Install(InstallError::Cancelled)));
        }
        Some(match result {
            Ok(Completion::Catalog(result)) => session.finish_catalog(pending.ticket, result),
            Ok(Completion::Download(result)) => session.finish_download(pending.ticket, result),
            Ok(Completion::Install(result)) => session
                .finish_install(pending.ticket)
                .and_then(|()| result.map_err(SessionError::Install)),
            Err(_) => {
                let _ = session.cancel();
                Err(SessionError::WrongState)
            }
        })
    }

    /// On UI cancellation or parent death, revoke both worker and session approval.
    /// Keep polling this same worker; do not replace it with another operation.
    pub fn cancel(&mut self, session: &mut InstallerSession) -> Result<(), SessionError> {
        session.cancel()?;
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
}

impl Drop for InstallerWorker {
    fn drop(&mut self) {
        // Out-of-process only: never blocks the setup UI on a cooperative network
        // timeout. The native helper must retain this owner until poll completes.
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn cancel_keeps_slot_until_actual_thread_exit_and_rejects_replacement() {
        let mut session = InstallerSession::default();
        let mut worker = InstallerWorker::default();
        let (release, wait) = mpsc::channel();
        let (observed, result) = mpsc::channel();
        worker
            .start_catalog(&mut session, move |cancel| {
                wait.recv_timeout(Duration::from_secs(2)).unwrap();
                observed.send(cancel.load(Ordering::Acquire)).unwrap();
                Err(InstallError::Cancelled)
            })
            .unwrap();
        worker.cancel(&mut session).unwrap();
        assert!(worker.busy());
        assert!(worker.poll(&mut session).is_none());
        assert!(
            worker
                .start_catalog(&mut session, |_| panic!("replacement started"))
                .is_err()
        );
        release.send(()).unwrap();
        assert!(result.recv_timeout(Duration::from_secs(2)).unwrap());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.poll(&mut session) {
                assert!(matches!(
                    result,
                    Err(SessionError::Install(InstallError::Cancelled))
                ));
                break;
            }
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert!(!worker.busy());
        assert!(session.selection().is_err());
    }
}
