//! Consuming installer workflow, independent of the eventual process transport.
//! Generation tickets prevent late worker results and replayed approvals from
//! being reused after cancellation, catalog refresh or a new operation.
use crate::{
    installer_packages::InstallerPackageSelection,
    language_package::PackageTrust,
    package_install::{InstallError, PreparedCatalog, PreparedOnlineInstall, SelectedDownload},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(u64);

enum State {
    Empty,
    Checking(Ticket),
    Selecting(InstallerPackageSelection),
    Downloading(Ticket),
    Reviewing(Ticket, PreparedOnlineInstall),
    Installing(Ticket),
}

pub struct InstallerSession {
    generation: u64,
    state: State,
}

#[derive(Debug)]
pub enum SessionError {
    WrongState,
    Exhausted,
    Install(InstallError),
}

impl Default for InstallerSession {
    fn default() -> Self {
        Self {
            generation: 0,
            state: State::Empty,
        }
    }
}

impl InstallerSession {
    pub fn view(&self) -> u64 {
        self.generation
    }
    pub fn phase(&self) -> &'static str {
        match self.state {
            State::Empty => "idle",
            State::Checking(_) => "checking",
            State::Selecting(_) => "selecting",
            State::Downloading(_) => "downloading",
            State::Reviewing(_, _) => "reviewing",
            State::Installing(_) => "installing",
        }
    }
    pub fn replace_selection(
        &mut self,
        view: u64,
        ids: std::collections::BTreeSet<crate::PackId>,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<(), SessionError> {
        if view != self.generation {
            return Err(SessionError::WrongState);
        }
        let next = self
            .generation
            .checked_add(1)
            .ok_or(SessionError::Exhausted)?;
        self.selection()?
            .replace_selection(ids, trust, now)
            .map_err(SessionError::Install)?;
        self.generation = next;
        Ok(())
    }
    pub(crate) fn is_installing(&self, ticket: Ticket) -> bool {
        matches!(self.state, State::Installing(current) if current == ticket)
    }
    pub(crate) fn is_downloading(&self, ticket: Ticket) -> bool {
        matches!(self.state, State::Downloading(current) if current == ticket)
    }

    fn next(&mut self) -> Result<Ticket, SessionError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(SessionError::Exhausted)?;
        Ok(Ticket(self.generation))
    }

    /// Reserve a generation before asynchronous catalog retrieval begins.
    pub fn begin_catalog(&mut self) -> Result<Ticket, SessionError> {
        if matches!(
            self.state,
            State::Checking(_) | State::Downloading(_) | State::Installing(_)
        ) {
            return Err(SessionError::WrongState);
        }
        let ticket = self.next()?;
        self.state = State::Checking(ticket);
        Ok(ticket)
    }

    pub fn finish_catalog(
        &mut self,
        ticket: Ticket,
        result: Result<PreparedCatalog, InstallError>,
    ) -> Result<(), SessionError> {
        if !matches!(self.state, State::Checking(current) if current == ticket) {
            return Err(SessionError::WrongState);
        }
        self.state = State::Empty;
        self.state = State::Selecting(InstallerPackageSelection::new(
            result.map_err(SessionError::Install)?,
        ));
        Ok(())
    }

    /// Synchronous catalog injection; asynchronous adapters use the ticket pair.
    /// Refresh clears selection and invalidates the previous review.
    pub fn show_catalog(&mut self, catalog: PreparedCatalog) -> Result<(), SessionError> {
        if matches!(
            self.state,
            State::Checking(_) | State::Downloading(_) | State::Installing(_)
        ) {
            return Err(SessionError::WrongState);
        }
        self.next()?;
        self.state = State::Selecting(InstallerPackageSelection::new(catalog));
        Ok(())
    }

    pub fn selection(&mut self) -> Result<&mut InstallerPackageSelection, SessionError> {
        match &mut self.state {
            State::Selecting(selection) => Ok(selection),
            _ => Err(SessionError::WrongState),
        }
    }

    /// Explicit download approval takes the selection out of UI ownership.
    /// The caller owns the cancellation flag and must retain the worker handle.
    pub fn begin_download(
        &mut self,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<Option<(Ticket, SelectedDownload)>, SessionError> {
        if !matches!(self.state, State::Selecting(_)) {
            return Err(SessionError::WrongState);
        }
        let ticket = self.next()?;
        let State::Selecting(selection) = std::mem::replace(&mut self.state, State::Empty) else {
            unreachable!()
        };
        let selected = selection
            .confirm_download(trust, now)
            .map_err(SessionError::Install)?;
        if let Some(selected) = selected {
            self.state = State::Downloading(ticket);
            Ok(Some((ticket, selected)))
        } else {
            Ok(None)
        }
    }

    /// Late or duplicate completion is rejected and its retained artifacts drop.
    /// A failed download returns to idle, requiring a fresh catalog check.
    pub fn finish_download(
        &mut self,
        ticket: Ticket,
        result: Result<PreparedOnlineInstall, InstallError>,
    ) -> Result<(), SessionError> {
        if !matches!(self.state, State::Downloading(current) if current == ticket) {
            return Err(SessionError::WrongState);
        }
        self.state = State::Empty;
        self.state = State::Reviewing(ticket, result.map_err(SessionError::Install)?);
        Ok(())
    }

    pub fn review(&self) -> Result<(Ticket, &PreparedOnlineInstall), SessionError> {
        match &self.state {
            State::Reviewing(ticket, prepared) => Ok((*ticket, prepared)),
            _ => Err(SessionError::WrongState),
        }
    }

    /// Separate approval after authenticated license/notice review. The worker
    /// still calls PreparedOnlineInstall::confirm under the installer leases.
    pub fn begin_install(&mut self, ticket: Ticket) -> Result<PreparedOnlineInstall, SessionError> {
        if !matches!(self.state, State::Reviewing(current, _) if current == ticket) {
            return Err(SessionError::WrongState);
        }
        let State::Reviewing(_, prepared) =
            std::mem::replace(&mut self.state, State::Installing(ticket))
        else {
            unreachable!()
        };
        Ok(prepared)
    }

    /// Result success/uncertainty reporting remains with the transaction worker.
    /// No automatic retry is created, including after an uncertain commit.
    pub fn finish_install(&mut self, ticket: Ticket) -> Result<(), SessionError> {
        if !matches!(self.state, State::Installing(current) if current == ticket) {
            return Err(SessionError::WrongState);
        }
        self.state = State::Empty;
        Ok(())
    }

    /// Revokes UI approval only. The adapter must ALSO set the active worker's
    /// monotonic cancel flag and join/observe it before starting another worker.
    /// Installation cannot be revoked midway through a store transaction.
    pub fn cancel(&mut self) -> Result<(), SessionError> {
        if matches!(self.state, State::Installing(_)) {
            return Err(SessionError::WrongState);
        }
        self.next()?;
        self.state = State::Empty;
        Ok(())
    }
}
