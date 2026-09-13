//! Accepted catalog receipts and an explicit selected-download transaction.
//! No package is installed by preparation or download. Accepting a download
//! does persist the authenticated catalog high-water mark even if it is cancelled.

use crate::{
    PackId,
    language_package::PackageTrust,
    package_catalog::{
        DownloadPlan, PinnedPackage, ReleaseError, VerifiedReleaseCatalog, repository_id,
    },
    package_download::{DownloadError, DownloadedPackage},
    package_inventory::{InventoryError, MAX_OPTIONAL_PACKAGES, is_base},
    package_store::{PackageStore, StoreError, StoreSnapshot},
};
use std::{
    collections::BTreeSet,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug)]
pub enum InstallError {
    Store(StoreError),
    Catalog(ReleaseError),
    Inventory(InventoryError),
    Download(DownloadError),
    EmptySelection,
    Cancelled,
}
impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "package_install_{self:?}")
    }
}
impl std::error::Error for InstallError {}
impl From<StoreError> for InstallError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}
impl From<ReleaseError> for InstallError {
    fn from(value: ReleaseError) -> Self {
        Self::Catalog(value)
    }
}
impl From<InventoryError> for InstallError {
    fn from(value: InventoryError) -> Self {
        Self::Inventory(value)
    }
}
impl From<DownloadError> for InstallError {
    fn from(value: DownloadError) -> Self {
        Self::Download(value)
    }
}

pub struct PreparedCatalog {
    raw: Vec<u8>,
    repository: String,
    expected: StoreSnapshot,
    catalog: VerifiedReleaseCatalog,
}
impl PreparedCatalog {
    /// Explicit metadata check only, with an independently chosen/bound source.
    /// The mutable release pointer is safe only because returned metadata is
    /// authenticated and checked against the persisted catalog high-water mark.
    #[cfg(windows)]
    pub fn from_github(
        store: &PackageStore,
        repository: &str,
        trust: &PackageTrust,
        cancel: &AtomicBool,
    ) -> Result<Self, InstallError> {
        check_cancel(cancel)?;
        let repository = repository_id(repository)?;
        let expected = store.load(trust)?;
        if expected
            .inventory()
            .repository()
            .is_some_and(|old| old != repository)
        {
            return Err(ReleaseError::WrongRepository.into());
        }
        let raw = crate::package_download::catalog_bytes(&repository, cancel)?;
        check_cancel(cancel)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| DownloadError::Clock)?
            .as_secs();
        Self::from_bytes(raw, &repository, &expected, trust, now)
    }
    /// `repository` is independently approved application/store policy, never
    /// taken from the untrusted catalog. This preparation makes no writes.
    pub fn from_bytes(
        raw: Vec<u8>,
        repository: &str,
        expected: &StoreSnapshot,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<Self, InstallError> {
        let repository = repository_id(repository)?;
        if expected
            .inventory()
            .repository()
            .is_some_and(|old| old != repository)
        {
            return Err(ReleaseError::WrongRepository.into());
        }
        let catalog = VerifiedReleaseCatalog::verify(
            &raw,
            trust,
            &repository,
            expected.inventory().catalog_checkpoint(),
            now,
        )?;
        Ok(Self {
            raw,
            repository,
            expected: expected.clone(),
            catalog,
        })
    }
    pub fn repository(&self) -> &str {
        &self.repository
    }
    pub fn packages(&self) -> impl Iterator<Item = &PinnedPackage> {
        self.catalog.packages()
    }

    /// A preview is not approval. The caller obtains explicit confirmation before
    /// calling accept_and_download_with. Nothing is silently selected or removed.
    pub fn select(
        &self,
        ids: &BTreeSet<PackId>,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<SelectedDownload, InstallError> {
        if ids.is_empty() {
            return Err(InstallError::EmptySelection);
        }
        let catalog = VerifiedReleaseCatalog::verify(
            &self.raw,
            trust,
            &self.repository,
            self.expected.inventory().catalog_checkpoint(),
            now,
        )?;
        let plan = catalog.select(ids, now)?;
        let mut installed: BTreeSet<_> = self
            .expected
            .inventory()
            .receipts()
            .map(|receipt| receipt.id())
            .collect();
        for pin in plan.packages() {
            if is_base(pin.id()) {
                return Err(InventoryError::ProtectedBase.into());
            }
            installed.insert(pin.id());
            if let Some(previous) = self.expected.inventory().highest_seen(&pin.id()) {
                if pin.revision() < previous.revision() {
                    return Err(InventoryError::Rollback.into());
                }
                if pin.revision() == previous.revision()
                    && (pin.sha256() != previous.sha256() || pin.bytes() != previous.bytes())
                {
                    return Err(InventoryError::RevisionConflict.into());
                }
            }
        }
        if installed.len() > MAX_OPTIONAL_PACKAGES {
            return Err(InventoryError::TooLarge.into());
        }
        Ok(SelectedDownload {
            raw: self.raw.clone(),
            repository: self.repository.clone(),
            expected: self.expected.clone(),
            ids: ids.clone(),
            plan,
        })
    }
}

pub struct SelectedDownload {
    raw: Vec<u8>,
    repository: String,
    expected: StoreSnapshot,
    ids: BTreeSet<PackId>,
    plan: DownloadPlan,
}
impl SelectedDownload {
    pub fn plan(&self) -> &DownloadPlan {
        &self.plan
    }

    /// Call only after user approval. First persist the accepted catalog receipt
    /// under the store lock. Cancellation/failure retains that anti-rollback
    /// receipt but installs no packages. Exact cached bytes are reauthenticated;
    /// only absent selected artifacts reach `fetch`. The caller bounds the
    /// transport, reports progress and supports cancellation during its I/O.
    pub fn accept_and_download_with(
        self,
        store: &PackageStore,
        trust: &PackageTrust,
        cancel: &AtomicBool,
        now: impl FnMut() -> Result<u64, DownloadError>,
        fetch: impl FnMut(&PinnedPackage) -> Result<DownloadedPackage, DownloadError>,
    ) -> Result<PreparedOnlineInstall, InstallError> {
        self.accept_and_download_with_progress(store, trust, cancel, now, fetch, |_, _| {})
    }

    /// Report one completed artifact at a time, including exact cache hits.
    pub fn accept_and_download_with_progress(
        mut self,
        store: &PackageStore,
        trust: &PackageTrust,
        cancel: &AtomicBool,
        mut now: impl FnMut() -> Result<u64, DownloadError>,
        mut fetch: impl FnMut(&PinnedPackage) -> Result<DownloadedPackage, DownloadError>,
        mut progress: impl FnMut(usize, u64),
    ) -> Result<PreparedOnlineInstall, InstallError> {
        check_cancel(cancel)?;
        let current = store.load(trust)?;
        if current.state_sha256() != self.expected.state_sha256() {
            return Err(StoreError::StaleSnapshot.into());
        }
        let time = now()?;
        let catalog = VerifiedReleaseCatalog::verify(
            &self.raw,
            trust,
            &self.repository,
            current.inventory().catalog_checkpoint(),
            time,
        )?;
        self.plan = catalog.select(&self.ids, time)?;
        let receipt = catalog.select(&BTreeSet::new(), time)?;
        let accepted = current.inventory().stage_plan(&receipt, &[], time)?;
        check_cancel(cancel)?;
        self.expected = store.commit(&current, &accepted, &[], trust)?;
        let mut downloads = Vec::with_capacity(self.plan.packages().len());
        let mut completed_bytes = 0u64;
        for pin in self.plan.packages() {
            check_cancel(cancel)?;
            pin.check_current(now()?)?;
            let downloaded = match store.cached_artifact(pin)? {
                Some(bytes) => DownloadedPackage::from_bytes(pin, bytes, trust, now()?)?,
                None => fetch(pin)?,
            };
            check_cancel(cancel)?;
            pin.check_package(downloaded.package(), now()?)?;
            completed_bytes += downloaded.bytes().len() as u64;
            downloads.push(downloaded);
            progress(downloads.len(), completed_bytes);
        }
        let packages: Vec<_> = downloads.iter().map(|p| Arc::clone(p.package())).collect();
        self.expected
            .inventory()
            .stage_plan(&self.plan, &packages, now()?)?;
        check_cancel(cancel)?;
        Ok(PreparedOnlineInstall {
            selection: self,
            downloads,
        })
    }
}

pub struct PreparedOnlineInstall {
    selection: SelectedDownload,
    downloads: Vec<DownloadedPackage>,
}
impl PreparedOnlineInstall {
    pub fn packages(&self) -> &[DownloadedPackage] {
        &self.downloads
    }
    pub fn plan(&self) -> &DownloadPlan {
        &self.selection.plan
    }

    /// Separate explicit installation confirmation: recheck catalog trust/time,
    /// the complete selection and store snapshot before publishing any artifacts.
    pub fn confirm(
        self,
        store: &PackageStore,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<StoreSnapshot, InstallError> {
        let selected = self.selection;
        let catalog = VerifiedReleaseCatalog::verify(
            &selected.raw,
            trust,
            &selected.repository,
            selected.expected.inventory().catalog_checkpoint(),
            now,
        )?;
        let plan = catalog.select(&selected.ids, now)?;
        let packages: Vec<_> = self
            .downloads
            .iter()
            .map(|p| Arc::clone(p.package()))
            .collect();
        let candidate = selected
            .expected
            .inventory()
            .stage_plan(&plan, &packages, now)?;
        let artifacts: Vec<_> = self.downloads.iter().map(|p| p.bytes()).collect();
        Ok(store.commit(&selected.expected, &candidate, &artifacts, trust)?)
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), InstallError> {
    if cancel.load(Ordering::Relaxed) {
        Err(InstallError::Cancelled)
    } else {
        Ok(())
    }
}
