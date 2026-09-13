//! Explicit, transactional local storage for authenticated language packages.
//!
//! The caller supplies an absolute path below a protected per-user directory.
//! This is not a sandbox against another process able to replace that directory
//! or edit its contents. Only cooperating writers are serialized by the lock.
//! No network, configuration, selections, lexicons or live hooks are touched.

use crate::{
    language_package::{PackageError, PackageTrust, VerifiedLanguagePackage, decode_hex},
    package_inventory::{InventoryError, MAX_INVENTORY_STATE_BYTES, PackageInventory},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Inventory(InventoryError),
    Package(PackageError),
    InvalidFile,
    MissingArtifact,
    UnexpectedArtifact,
    Integrity,
    Busy,
    StaleSnapshot,
    MissingMigrationInput,
    ExistingMigrationState,
    /// The pointer may already be visible. Reload; do not blindly retry.
    CommitUncertain,
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "package_store_{self:?}")
    }
}
impl std::error::Error for StoreError {}
impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<InventoryError> for StoreError {
    fn from(error: InventoryError) -> Self {
        Self::Inventory(error)
    }
}
impl From<PackageError> for StoreError {
    fn from(error: PackageError) -> Self {
        Self::Package(error)
    }
}

#[derive(Debug, Clone)]
pub struct StoreSnapshot {
    inventory: PackageInventory,
    state_hash: [u8; 32],
}
impl StoreSnapshot {
    pub fn inventory(&self) -> &PackageInventory {
        &self.inventory
    }
    pub const fn state_sha256(&self) -> [u8; 32] {
        self.state_hash
    }
}

/// Authenticated migration inputs, prepared without creating a store. The
/// configuration-mode switch is a separate, final caller transaction.
pub struct PreparedStoreInitialization {
    artifacts: Vec<Vec<u8>>,
    packages: Vec<Arc<VerifiedLanguagePackage>>,
    candidate: PackageInventory,
}
impl PreparedStoreInitialization {
    pub fn from_files(
        paths: &[PathBuf],
        required_inputs: &BTreeSet<crate::PackId>,
        trust: &PackageTrust,
    ) -> Result<Self, StoreError> {
        if paths.len() > crate::package_inventory::MAX_OPTIONAL_PACKAGES
            || required_inputs.len() > 64
        {
            return Err(StoreError::UnexpectedArtifact);
        }
        let mut total = 0u64;
        let mut artifacts = Vec::with_capacity(paths.len());
        let mut packages = Vec::with_capacity(paths.len());
        for path in paths {
            validate_path(path)?;
            let remaining = crate::package_catalog::MAX_SELECTED_DOWNLOAD_BYTES - total;
            let bytes = read_file(
                path,
                remaining.min(crate::language_package::MAX_PACKAGE_BYTES as u64),
            )?;
            total += bytes.len() as u64;
            packages.push(Arc::new(VerifiedLanguagePackage::verify(&bytes, trust)?));
            artifacts.push(bytes);
        }
        let candidate = PackageInventory::default().stage_import(&packages)?;
        let registry = candidate
            .dictionary_snapshot(&crate::DictionaryRegistry::english_base(), &BTreeSet::new())?;
        if required_inputs
            .iter()
            .any(|id| !registry.installed_ids().any(|installed| installed == id))
        {
            return Err(StoreError::MissingMigrationInput);
        }
        Ok(Self {
            artifacts,
            packages,
            candidate,
        })
    }

    pub fn packages(&self) -> &[Arc<VerifiedLanguagePackage>] {
        &self.packages
    }

    pub fn total_bytes(&self) -> u64 {
        self.artifacts.iter().map(|bytes| bytes.len() as u64).sum()
    }

    /// Confirm initialization only. It never edits configuration, selections or
    /// legacy files. Existing initialized state is reused only if empty or
    /// exactly this same previously confirmed candidate; unrelated state is
    /// never overwritten/adopted. Partial initialization fails closed.
    /// The caller must save managed mode last, after this operation succeeds.
    pub fn confirm(self, root: &Path, trust: &PackageTrust) -> Result<StoreSnapshot, StoreError> {
        // Reauthenticate before any initialization writes if trust has changed.
        for bytes in &self.artifacts {
            VerifiedLanguagePackage::verify(bytes, trust)?;
        }
        let store = match PackageStore::open(root) {
            Ok(store) => store,
            Err(StoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                PackageStore::initialize(root)?
            }
            Err(error) => return Err(error),
        };
        let expected = store.load(trust)?;
        let current = expected.inventory().state_bytes()?;
        let candidate = self.candidate.state_bytes()?;
        let empty = PackageInventory::default().state_bytes()?;
        if current != empty && current != candidate {
            return Err(StoreError::ExistingMigrationState);
        }
        let artifacts: Vec<&[u8]> = self.artifacts.iter().map(Vec::as_slice).collect();
        store.commit(&expected, &self.candidate, &artifacts, trust)
    }
}

/// One authenticated file awaiting explicit user confirmation. Keeps the exact
/// bytes previewed, not a path that could point to different content later.
/// Dropping this value cancels the import without touching the store.
pub struct PreparedImport {
    bytes: Vec<u8>,
    package: Arc<VerifiedLanguagePackage>,
    expected: StoreSnapshot,
    candidate: PackageInventory,
}
impl PreparedImport {
    /// Read-only preparation against a loaded store snapshot. Call outside the
    /// input hook/UI event loop; even valid packages can be large.
    pub fn from_file(
        path: &Path,
        expected: &StoreSnapshot,
        trust: &PackageTrust,
    ) -> Result<Self, StoreError> {
        validate_path(path)?;
        let bytes = read_file(path, crate::language_package::MAX_PACKAGE_BYTES as u64)?;
        let package = Arc::new(VerifiedLanguagePackage::verify(&bytes, trust)?);
        let candidate = expected
            .inventory()
            .stage_import(std::slice::from_ref(&package))?;
        Ok(Self {
            bytes,
            package,
            expected: expected.clone(),
            candidate,
        })
    }

    pub fn package(&self) -> &VerifiedLanguagePackage {
        &self.package
    }

    /// Consume the confirmation once. Current trust and the exact original
    /// inventory are checked again by commit; stale previews must be recreated.
    /// Does not enable input packs or change configuration/runtime snapshots.
    pub fn confirm(
        self,
        store: &PackageStore,
        trust: &PackageTrust,
    ) -> Result<StoreSnapshot, StoreError> {
        store.commit(&self.expected, &self.candidate, &[&self.bytes], trust)
    }
}

#[derive(Debug, Clone)]
pub struct PackageStore {
    root: PathBuf,
}

/// Exact cached previous version awaiting separate rollback confirmation.
/// No historical directory scan, network request, or CURRENT-pointer rewind.
pub struct PreparedRollback {
    bytes: Vec<u8>,
    package: Arc<VerifiedLanguagePackage>,
    from_revision: u64,
    expected: StoreSnapshot,
    candidate: PackageInventory,
}
impl PreparedRollback {
    pub fn from_store(
        store: &PackageStore,
        id: crate::PackId,
        trust: &PackageTrust,
    ) -> Result<Self, StoreError> {
        let expected = store.load(trust)?;
        let target = expected
            .inventory()
            .rollback_target(&id)
            .ok_or(InventoryError::Rollback)?;
        let bytes = read_file(
            &store.root.join(blob_name(&target.sha256())),
            target.bytes(),
        )?;
        if bytes.len() as u64 != target.bytes() || digest(&bytes) != target.sha256() {
            return Err(StoreError::Integrity);
        }
        let package = Arc::new(VerifiedLanguagePackage::verify(&bytes, trust)?);
        let from_revision = expected
            .inventory()
            .receipts()
            .find(|receipt| receipt.id() == id)
            .ok_or(InventoryError::InvalidState)?
            .revision();
        let candidate = expected.inventory().stage_rollback(package.clone())?;
        Ok(Self {
            bytes,
            package,
            from_revision,
            expected,
            candidate,
        })
    }

    pub fn package(&self) -> &VerifiedLanguagePackage {
        &self.package
    }

    pub const fn from_revision(&self) -> u64 {
        self.from_revision
    }

    pub fn confirm(
        self,
        store: &PackageStore,
        trust: &PackageTrust,
    ) -> Result<StoreSnapshot, StoreError> {
        store.commit(&self.expected, &self.candidate, &[&self.bytes], trust)
    }
}

impl PackageStore {
    /// Explicit initialization only: the target MUST NOT exist. Its protected
    /// parent must already exist. An interrupted initialization fails closed on
    /// open; it is never interpreted as an empty inventory or auto-repaired.
    pub fn initialize(root: &Path) -> Result<Self, StoreError> {
        validate_path(root)?;
        check_directory(root.parent().ok_or(StoreError::InvalidFile)?)?;
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(root)?;
        let store = Self {
            root: root.to_owned(),
        };
        let lock = create_file(&root.join("writer.lock"))?;
        lock.sync_all()?;
        let bytes = PackageInventory::default().state_bytes()?;
        let hash = digest(&bytes);
        store.put_immutable(&state_name(&hash), &bytes)?;
        let mut pointer = create_file(&root.join("CURRENT"))?;
        pointer.write_all(hex(&hash).as_bytes())?;
        pointer.sync_all()?;
        store.sync_directory()?;
        #[cfg(unix)]
        File::open(root.parent().ok_or(StoreError::InvalidFile)?)?.sync_all()?;
        Ok(store)
    }

    /// Does not create or repair files. Use load to authenticate the inventory.
    pub fn open(root: &Path) -> Result<Self, StoreError> {
        validate_path(root)?;
        check_directory(root)?;
        Ok(Self {
            root: root.to_owned(),
        })
    }

    pub fn load(&self, trust: &PackageTrust) -> Result<StoreSnapshot, StoreError> {
        check_directory(&self.root)?;
        let pointer = read_file(&self.root.join("CURRENT"), 64)?;
        let hash = parse_pointer(&pointer)?;
        let state = read_file(
            &self.root.join(state_name(&hash)),
            MAX_INVENTORY_STATE_BYTES as u64,
        )?;
        if digest(&state) != hash {
            return Err(StoreError::Integrity);
        }
        let receipts = PackageInventory::required_artifacts(&state)?;
        let mut packages = Vec::with_capacity(receipts.len());
        for receipt in receipts {
            let bytes = read_file(
                &self.root.join(blob_name(&receipt.sha256())),
                receipt.bytes(),
            )?;
            if bytes.len() as u64 != receipt.bytes() || digest(&bytes) != receipt.sha256() {
                return Err(StoreError::Integrity);
            }
            packages.push(Arc::new(VerifiedLanguagePackage::verify(&bytes, trust)?));
        }
        Ok(StoreSnapshot {
            inventory: PackageInventory::restore(&state, &packages)?,
            state_hash: hash,
        })
    }

    /// Read only the exact pinned cache object, never scan or select another
    /// revision. Missing data permits a selected download; corrupt data fails
    /// closed and is not silently overwritten. Signature verification remains
    /// mandatory before using the returned bytes.
    pub fn cached_artifact(
        &self,
        pin: &crate::package_catalog::PinnedPackage,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        check_directory(&self.root)?;
        let bytes = match read_file(&self.root.join(blob_name(&pin.sha256())), pin.bytes()) {
            Ok(bytes) => bytes,
            Err(StoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if bytes.len() as u64 != pin.bytes() || digest(&bytes) != pin.sha256() {
            return Err(StoreError::Integrity);
        }
        Ok(Some(bytes))
    }

    /// Commit a staged candidate against the exact loaded state. Supply raw
    /// envelopes for new artifacts only (existing matching artifacts may also
    /// be supplied). Every candidate artifact is authenticated under current
    /// trust before any publication. A successful no-op does not create files.
    pub fn commit(
        &self,
        expected: &StoreSnapshot,
        candidate: &PackageInventory,
        artifacts: &[&[u8]],
        trust: &PackageTrust,
    ) -> Result<StoreSnapshot, StoreError> {
        self.commit_inner(expected, candidate, artifacts, trust, |_| Ok(()))
    }

    /// Pin the exact authenticated store while a caller performs its final
    /// configuration activation. Cooperating store writers stay excluded until
    /// the callback returns. The callback must not reenter store operations.
    /// When both locks are needed, acquire this store lock before the config
    /// writer lock; callers must not hold the config lock before entering here.
    pub fn with_current_snapshot(
        &self,
        expected: &StoreSnapshot,
        trust: &PackageTrust,
        activate: impl FnOnce() -> io::Result<()>,
    ) -> Result<(), StoreError> {
        check_directory(&self.root)?;
        let _lock = self.writer_lock()?;
        let current = self.load(trust)?;
        if current.state_hash != expected.state_hash {
            return Err(StoreError::StaleSnapshot);
        }
        activate().map_err(StoreError::Io)
    }

    fn commit_inner(
        &self,
        expected: &StoreSnapshot,
        candidate: &PackageInventory,
        artifacts: &[&[u8]],
        trust: &PackageTrust,
        mut checkpoint: impl FnMut(CommitPoint) -> io::Result<()>,
    ) -> Result<StoreSnapshot, StoreError> {
        check_directory(&self.root)?;
        let _lock = self.writer_lock()?;
        let current = self.load(trust)?;
        if current.state_hash != expected.state_hash {
            return Err(StoreError::StaleSnapshot);
        }
        current.inventory.check_successor(candidate)?;
        let state = candidate.state_bytes()?;
        let receipts = PackageInventory::required_artifacts(&state)?;
        if artifacts.len() > receipts.len()
            || artifacts
                .iter()
                .try_fold(0u64, |total, bytes| total.checked_add(bytes.len() as u64))
                .is_none_or(|total| total > crate::package_catalog::MAX_SELECTED_DOWNLOAD_BYTES)
        {
            return Err(StoreError::UnexpectedArtifact);
        }
        let mut supplied = BTreeMap::new();
        for bytes in artifacts {
            if bytes.len() > crate::language_package::MAX_PACKAGE_BYTES {
                return Err(StoreError::UnexpectedArtifact);
            }
            let hash = digest(bytes);
            if !receipts
                .iter()
                .any(|r| r.sha256() == hash && r.bytes() == bytes.len() as u64)
                || supplied.insert(hash, *bytes).is_some()
            {
                return Err(StoreError::UnexpectedArtifact);
            }
        }
        // Keep raw new artifacts borrowed. Existing bytes are read one at a time.
        let mut verified = Vec::with_capacity(receipts.len());
        for receipt in &receipts {
            let existing;
            let bytes = match supplied.get(&receipt.sha256()) {
                Some(bytes) => *bytes,
                None => {
                    existing = read_file(
                        &self.root.join(blob_name(&receipt.sha256())),
                        receipt.bytes(),
                    )
                    .map_err(|error| match error {
                        StoreError::Io(ref io) if io.kind() == io::ErrorKind::NotFound => {
                            StoreError::MissingArtifact
                        }
                        other => other,
                    })?;
                    &existing
                }
            };
            if bytes.len() as u64 != receipt.bytes() || digest(bytes) != receipt.sha256() {
                return Err(StoreError::Integrity);
            }
            verified.push(Arc::new(VerifiedLanguagePackage::verify(bytes, trust)?));
        }
        let inventory = PackageInventory::restore(&state, &verified)?;
        let state_hash = digest(&state);
        if state_hash == current.state_hash {
            return Ok(current);
        }
        checkpoint(CommitPoint::Validated)?;
        for (hash, bytes) in supplied {
            self.put_immutable(&blob_name(&hash), bytes)?;
        }
        checkpoint(CommitPoint::BlobsPublished)?;
        self.put_immutable(&state_name(&state_hash), &state)?;
        self.sync_directory()?;
        checkpoint(CommitPoint::StatePublished)?;
        let temporary = self.temporary(hex(&state_hash).as_bytes())?;
        checkpoint(CommitPoint::BeforePointer)?;
        // Same directory/volume. No copy/delete fallback across volumes.
        replace_pointer(&temporary.path, &self.root.join("CURRENT"))?;
        // Any failure from here is ambiguous to the caller: the new state may
        // already be in use. Old immutable states and artifacts remain intact.
        self.sync_directory()
            .map_err(|_| StoreError::CommitUncertain)?;
        checkpoint(CommitPoint::PointerPublished).map_err(|_| StoreError::CommitUncertain)?;
        Ok(StoreSnapshot {
            inventory,
            state_hash,
        })
    }

    fn writer_lock(&self) -> Result<File, StoreError> {
        let file = open_file(&self.root.join("writer.lock"), true)?;
        if file.metadata()?.len() != 0 {
            return Err(StoreError::InvalidFile);
        }
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(TryLockError::WouldBlock) => Err(StoreError::Busy),
            Err(TryLockError::Error(error)) => Err(StoreError::Io(error)),
        }
    }

    fn temporary(&self, contents: &[u8]) -> Result<Temporary, StoreError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = self
                .root
                .join(format!("tmp-{}-{serial}", std::process::id()));
            let mut file = match create_file(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            let temporary = Temporary { path };
            file.write_all(contents)?;
            file.sync_all()?;
            return Ok(temporary);
        }
        Err(StoreError::Busy)
    }

    fn put_immutable(&self, name: &str, contents: &[u8]) -> Result<(), StoreError> {
        let destination = self.root.join(name);
        match read_file(&destination, contents.len() as u64) {
            Ok(existing) => {
                return if existing == contents {
                    Ok(())
                } else {
                    Err(StoreError::Integrity)
                };
            }
            Err(StoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let temporary = self.temporary(contents)?;
        // Hard-link publication is atomic and cannot replace an existing name.
        // Filesystems without this operation are unsupported and fail closed.
        match fs::hard_link(&temporary.path, &destination) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if read_file(&destination, contents.len() as u64)? == contents {
                    Ok(())
                } else {
                    Err(StoreError::Integrity)
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    fn sync_directory(&self) -> io::Result<()> {
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        // Windows flushes newly written file contents before namespace rename.
        // FileRenameInfoEx has no write-through flag; namespace durability after
        // power loss is not established by successful API return or unit tests.
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommitPoint {
    Validated,
    BlobsPublished,
    StatePublished,
    BeforePointer,
    PointerPublished,
}
struct Temporary {
    path: PathBuf,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn blob_name(hash: &[u8; 32]) -> String {
    format!("blob-{}.aklp", hex(hash))
}
fn state_name(hash: &[u8; 32]) -> String {
    format!("state-{}.json", hex(hash))
}
fn parse_pointer(bytes: &[u8]) -> Result<[u8; 32], StoreError> {
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err(StoreError::InvalidFile);
    }
    decode_hex(std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidFile)?)
        .map_err(|_| StoreError::InvalidFile)
}
fn validate_path(root: &Path) -> Result<(), StoreError> {
    if !root.is_absolute()
        || root.file_name().is_none()
        || root
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(StoreError::InvalidFile);
    }
    Ok(())
}
fn check_directory(path: &Path) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(StoreError::InvalidFile);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(StoreError::InvalidFile);
        }
    }
    Ok(())
}
fn create_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
fn open_file(path: &Path, write: bool) -> Result<File, StoreError> {
    let mut options = OpenOptions::new();
    options.read(true).write(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
        // LockFileEx arbitrates cooperating writer handles; immutable readers
        // reject concurrent writes but permit CURRENT replacement.
        options.share_mode(if write {
            (FILE_SHARE_READ | FILE_SHARE_WRITE).0
        } else {
            (FILE_SHARE_READ | FILE_SHARE_DELETE).0
        });
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(StoreError::InvalidFile);
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_TYPE_DISK, GetFileType},
        };
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            || unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK
        {
            return Err(StoreError::InvalidFile);
        }
    }
    Ok(file)
}
fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, StoreError> {
    let file = open_file(path, false)?;
    if file.metadata()?.len() > limit {
        return Err(StoreError::InvalidFile);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(StoreError::InvalidFile);
    }
    Ok(bytes)
}
#[cfg(not(windows))]
fn replace_pointer(source: &Path, destination: &Path) -> Result<(), StoreError> {
    fs::rename(source, destination).map_err(StoreError::Io)
}
#[cfg(windows)]
fn replace_pointer(source: &Path, destination: &Path) -> Result<(), StoreError> {
    use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_TYPE_DISK, FileRenameInfoEx, GetFileType, SetFileInformationByHandle,
        },
    };
    if source.parent() != destination.parent() || !destination.is_absolute() {
        return Err(StoreError::InvalidFile);
    }
    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    if name.is_empty() || name.len() > 32767 || name.contains(&0) {
        return Err(StoreError::InvalidFile);
    }
    let name_bytes = name.len() * 2;
    let size = std::mem::size_of::<FILE_RENAME_INFO>() + name_bytes + 2;
    // usize storage guarantees native pointer alignment, unlike Vec<u8>.
    let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
    let buffer = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    let file = OpenOptions::new()
        .access_mode((DELETE | FILE_READ_ATTRIBUTES).0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(source)?;
    let metadata = file.metadata()?;
    use std::os::windows::fs::MetadataExt;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK
    {
        return Err(StoreError::InvalidFile);
    }
    unsafe {
        // REPLACE_IF_EXISTS | POSIX_SEMANTICS preserves existing target handles.
        // No copy/delete, ignore-readonly, ACL-ignore or legacy rename fallback.
        (*buffer).Anonymous.Flags = 1 | 2;
        (*buffer).RootDirectory = HANDLE::default();
        (*buffer).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(std::mem::offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfoEx,
            buffer.cast(),
            size as u32,
        )
    }
    // Once submitted, callers reload rather than automatically retry a commit.
    .map_err(|_| StoreError::CommitUncertain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;
    use std::collections::BTreeSet;

    // Public synthetic fixtures only. These keys are never release anchors.
    fn key() -> SigningKey {
        SigningKey::from_bytes(&[42; 32])
    }
    fn trust() -> PackageTrust {
        PackageTrust::from_public_keys([("test-only".into(), key().verifying_key().to_bytes())])
            .unwrap()
    }
    fn artifact(revision: u64) -> Vec<u8> {
        let components = json!({
            "words":"hello\nworld\n", "short_words":"hi\n",
            "scoring":include_str!("../data/scoring/en-US.json"),
            "input":r#"{"format":1,"pack_id":"test-ru","windows_keyboard_profiles":[{"profile":"0409:00000409","required_capabilities":["physical-key-v1"]}]}"#,
            "ui":r#"{"format":1,"locale":"ru","direction":"ltr","messages":{"locale.self_name":"Русский"}}"#,
            "license":"Synthetic test license only.","notice":"Synthetic package fixture."
        });
        let mut integrity = json!({});
        for (role, content) in components.as_object().unwrap() {
            let content = content.as_str().unwrap();
            integrity[role] =
                json!({"bytes":content.len(),"sha256":hex(&digest(content.as_bytes()))});
        }
        let manifest = serde_json::to_string(&json!({"format":1,"package_id":"test-ru","revision":revision,"runtime_api":1,"input_pack":"test-ru","ui_locale":"ru","components":integrity})).unwrap();
        let mut signed = b"AutoKeyboardLayot.language-package.v1\0".to_vec();
        signed.extend_from_slice(manifest.as_bytes());
        serde_json::to_vec(&json!({"format":1,"signer":"test-only","manifest":manifest,"signature":hex(&key().sign(&signed).to_bytes()),"components":components})).unwrap()
    }
    fn stage(snapshot: &StoreSnapshot, bytes: &[u8]) -> PackageInventory {
        let package = Arc::new(VerifiedLanguagePackage::verify(bytes, &trust()).unwrap());
        snapshot.inventory().stage_import(&[package]).unwrap()
    }
    fn setup() -> (tempfile::TempDir, PackageStore, StoreSnapshot) {
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("Языки with spaces")).unwrap();
        let snapshot = store.load(&trust()).unwrap();
        (directory, store, snapshot)
    }
    fn names(store: &PackageStore) -> BTreeSet<PathBuf> {
        fs::read_dir(&store.root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect()
    }

    #[test]
    fn rollback_preview_is_read_only_then_commits_fresh_state_and_rejects_stale_confirm() {
        let (_directory, store, empty) = setup();
        let id = crate::PackId::parse("test-ru").unwrap();
        let old = artifact(1);
        let new = artifact(2);
        let first = store
            .commit(&empty, &stage(&empty, &old), &[&old], &trust())
            .unwrap();
        let updated = store
            .commit(&first, &stage(&first, &new), &[&new], &trust())
            .unwrap();
        let before = names(&store);
        let prepare = || PreparedRollback::from_store(&store, id, &trust()).unwrap();
        let cancelled = prepare();
        assert_eq!(cancelled.from_revision(), 2);
        assert_eq!(cancelled.package().revision(), 1);
        drop(cancelled);
        assert_eq!(names(&store), before);
        assert_eq!(store.load(&trust()).unwrap().state_hash, updated.state_hash);
        let stale = prepare();
        let rolled = prepare().confirm(&store, &trust()).unwrap();
        assert_eq!(
            rolled.inventory().generation(),
            updated.inventory().generation() + 1
        );
        assert_eq!(rolled.inventory().receipts().next().unwrap().revision(), 1);
        assert_eq!(rolled.inventory().highest_seen(&id).unwrap().revision(), 2);
        assert_eq!(store.load(&trust()).unwrap().state_hash, rolled.state_hash);
        assert!(matches!(
            stale.confirm(&store, &trust()),
            Err(StoreError::StaleSnapshot)
        ));
        assert!(PreparedRollback::from_store(&store, id, &trust()).is_err());
        assert!(before.is_subset(&names(&store)));
    }

    #[test]
    fn rollback_interruptions_never_rewind_highwater_or_publish_a_mixed_state() {
        for point in [
            CommitPoint::Validated,
            CommitPoint::BlobsPublished,
            CommitPoint::StatePublished,
            CommitPoint::BeforePointer,
            CommitPoint::PointerPublished,
        ] {
            let (_directory, store, empty) = setup();
            let id = crate::PackId::parse("test-ru").unwrap();
            let old = artifact(1);
            let new = artifact(2);
            let first = store
                .commit(&empty, &stage(&empty, &old), &[&old], &trust())
                .unwrap();
            let updated = store
                .commit(&first, &stage(&first, &new), &[&new], &trust())
                .unwrap();
            let prepared = PreparedRollback::from_store(&store, id, &trust()).unwrap();
            let result = store.commit_inner(
                &prepared.expected,
                &prepared.candidate,
                &[&prepared.bytes],
                &trust(),
                |at| {
                    if at == point {
                        Err(io::Error::other("injected rollback interruption"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            let loaded = store.load(&trust()).unwrap();
            assert_eq!(
                loaded.inventory().highest_seen(&id),
                updated.inventory().highest_seen(&id)
            );
            if point == CommitPoint::PointerPublished {
                assert!(matches!(result, Err(StoreError::CommitUncertain)));
                assert_eq!(
                    loaded.inventory().generation(),
                    updated.inventory().generation() + 1
                );
                assert_eq!(loaded.inventory().receipts().next().unwrap().revision(), 1);
                assert!(matches!(
                    prepared.confirm(&store, &trust()),
                    Err(StoreError::StaleSnapshot)
                ));
            } else {
                assert_eq!(loaded.state_hash, updated.state_hash);
                prepared.confirm(&store, &trust()).unwrap();
            }
        }
    }

    #[test]
    fn rollback_missing_corrupt_previous_or_revoked_trust_never_changes_current() {
        let (_directory, store, empty) = setup();
        let id = crate::PackId::parse("test-ru").unwrap();
        let old = artifact(1);
        let new = artifact(2);
        let first = store
            .commit(&empty, &stage(&empty, &old), &[&old], &trust())
            .unwrap();
        let updated = store
            .commit(&first, &stage(&first, &new), &[&new], &trust())
            .unwrap();
        let prepared = PreparedRollback::from_store(&store, id, &trust()).unwrap();
        assert!(prepared.confirm(&store, &PackageTrust::default()).is_err());
        let old_path = store.root.join(blob_name(&digest(&old)));
        fs::write(&old_path, b"corrupt previous cache").unwrap();
        assert!(PreparedRollback::from_store(&store, id, &trust()).is_err());
        assert_eq!(store.load(&trust()).unwrap().state_hash, updated.state_hash);
        fs::remove_file(old_path).unwrap();
        assert!(PreparedRollback::from_store(&store, id, &trust()).is_err());
        assert_eq!(store.load(&trust()).unwrap().state_hash, updated.state_hash);
    }

    #[test]
    fn configuration_activation_pins_current_store_and_rejects_stale_state() {
        let (_directory, store, empty) = setup();
        let bytes = artifact(1);
        let installed = store
            .commit(&empty, &stage(&empty, &bytes), &[&bytes], &trust())
            .unwrap();
        let called = std::cell::Cell::new(false);
        assert!(matches!(
            store.with_current_snapshot(&empty, &trust(), || {
                called.set(true);
                Ok(())
            }),
            Err(StoreError::StaleSnapshot)
        ));
        assert!(!called.get());
        let before = names(&store);
        store
            .with_current_snapshot(&installed, &trust(), || {
                called.set(true);
                // A competing writer cannot publish between verification and save.
                assert!(matches!(
                    store.commit(&installed, installed.inventory(), &[], &trust()),
                    Err(StoreError::Busy)
                ));
                Ok(())
            })
            .unwrap();
        assert!(called.get());
        assert_eq!(names(&store), before);
        assert!(matches!(
            store.with_current_snapshot(&installed, &trust(), || Err(io::Error::other(
                "simulated config save failure"
            ))),
            Err(StoreError::Io(_))
        ));
        assert_eq!(
            store.load(&trust()).unwrap().state_hash,
            installed.state_hash
        );
        store
            .commit(&installed, installed.inventory(), &[], &trust())
            .unwrap();
    }

    #[test]
    fn store_initialization_preview_preserves_required_inputs_and_exact_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("selected.aklp");
        let root = directory.path().join("packages");
        let bytes = artifact(1);
        fs::write(&path, &bytes).unwrap();
        let required = BTreeSet::from([
            crate::Language::English,
            crate::PackId::parse("test-ru").unwrap(),
        ]);
        let prepare = || {
            PreparedStoreInitialization::from_files(
                std::slice::from_ref(&path),
                &required,
                &trust(),
            )
            .unwrap()
        };
        let prepared = prepare();
        assert!(!root.exists());
        assert_eq!(prepared.total_bytes(), bytes.len() as u64);
        assert_eq!(prepared.packages()[0].revision(), 1);
        fs::write(&path, artifact(2)).unwrap();
        let installed = prepared.confirm(&root, &trust()).unwrap();
        assert_eq!(
            installed.inventory().receipts().next().unwrap().revision(),
            1
        );
        // Recover an exact previously confirmed store after a failed config save.
        fs::write(&path, &bytes).unwrap();
        let before = names(&PackageStore::open(&root).unwrap());
        let retried = prepare().confirm(&root, &trust()).unwrap();
        assert_eq!(retried.state_hash, installed.state_hash);
        assert_eq!(before, names(&PackageStore::open(&root).unwrap()));
    }

    #[test]
    fn store_initialization_rejects_missing_inputs_duplicates_and_revoked_trust() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("selected.aklp");
        let root = directory.path().join("packages");
        fs::write(&path, artifact(1)).unwrap();
        assert!(matches!(
            PreparedStoreInitialization::from_files(
                std::slice::from_ref(&path),
                &BTreeSet::from([crate::Language::Russian]),
                &trust()
            ),
            Err(StoreError::MissingMigrationInput)
        ));
        assert!(matches!(
            PreparedStoreInitialization::from_files(
                &[path.clone(), path.clone()],
                &BTreeSet::new(),
                &trust()
            ),
            Err(StoreError::Inventory(InventoryError::DuplicatePackage))
        ));
        let prepared =
            PreparedStoreInitialization::from_files(&[path], &BTreeSet::new(), &trust()).unwrap();
        assert!(prepared.confirm(&root, &PackageTrust::default()).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn store_initialization_refuses_unrelated_history_and_does_not_repair_partial_roots() {
        let (directory, store, empty) = setup();
        let bytes = artifact(2);
        let installed = store
            .commit(&empty, &stage(&empty, &bytes), &[&bytes], &trust())
            .unwrap();
        let path = directory.path().join("selected.aklp");
        fs::write(&path, artifact(1)).unwrap();
        let prepare = || {
            PreparedStoreInitialization::from_files(
                std::slice::from_ref(&path),
                &BTreeSet::new(),
                &trust(),
            )
            .unwrap()
        };
        let before = names(&store);
        assert!(matches!(
            prepare().confirm(&store.root, &trust()),
            Err(StoreError::ExistingMigrationState)
        ));
        assert_eq!(
            store.load(&trust()).unwrap().state_hash,
            installed.state_hash
        );
        assert_eq!(names(&store), before);
        let partial = directory.path().join("partial");
        fs::create_dir(&partial).unwrap();
        assert!(prepare().confirm(&partial, &trust()).is_err());
        assert_eq!(fs::read_dir(partial).unwrap().count(), 0);
    }

    #[test]
    fn empty_english_initialization_is_explicit_and_does_not_need_optional_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("packages");
        let prepared = PreparedStoreInitialization::from_files(
            &[],
            &BTreeSet::from([crate::Language::English]),
            &PackageTrust::default(),
        )
        .unwrap();
        assert_eq!(prepared.total_bytes(), 0);
        assert!(!root.exists());
        let installed = prepared.confirm(&root, &PackageTrust::default()).unwrap();
        assert_eq!(installed.inventory().packages().count(), 0);
        assert_eq!(installed.inventory().generation(), 0);
    }

    #[test]
    fn prepared_import_is_read_only_and_confirms_exact_previewed_bytes() {
        let (directory, store, empty) = setup();
        let path = directory.path().join("import.aklp");
        let bytes = artifact(1);
        fs::write(&path, &bytes).unwrap();
        let before = names(&store);
        let cancelled = PreparedImport::from_file(&path, &empty, &trust()).unwrap();
        assert_eq!(cancelled.package().revision(), 1);
        drop(cancelled);
        assert_eq!(names(&store), before);
        assert_eq!(store.load(&trust()).unwrap().state_hash, empty.state_hash);

        let prepared = PreparedImport::from_file(&path, &empty, &trust()).unwrap();
        assert_eq!(prepared.package().envelope_sha256(), digest(&bytes));
        fs::write(&path, artifact(2)).unwrap();
        fs::remove_file(&path).unwrap();
        let committed = prepared.confirm(&store, &trust()).unwrap();
        assert_eq!(
            committed.inventory().receipts().next().unwrap().revision(),
            1
        );
        assert_eq!(
            store.load(&trust()).unwrap().state_hash,
            committed.state_hash
        );
    }

    #[test]
    fn prepared_import_rechecks_trust_and_refuses_stale_confirmation() {
        let (directory, store, empty) = setup();
        let path = directory.path().join("import.aklp");
        fs::write(&path, artifact(1)).unwrap();
        assert!(PreparedImport::from_file(&path, &empty, &PackageTrust::default()).is_err());
        let prepared = PreparedImport::from_file(&path, &empty, &trust()).unwrap();
        let before = names(&store);
        assert!(matches!(
            prepared.confirm(&store, &PackageTrust::default()),
            Err(StoreError::Package(_))
        ));
        assert_eq!(names(&store), before);

        let stale = PreparedImport::from_file(&path, &empty, &trust()).unwrap();
        let bytes = artifact(2);
        store
            .commit(&empty, &stage(&empty, &bytes), &[&bytes], &trust())
            .unwrap();
        let before = names(&store);
        assert!(matches!(
            stale.confirm(&store, &trust()),
            Err(StoreError::StaleSnapshot)
        ));
        assert_eq!(names(&store), before);
    }

    #[test]
    fn prepared_import_rejects_invalid_and_oversized_files_without_writes() {
        let (directory, store, empty) = setup();
        let path = directory.path().join("import.aklp");
        let before = names(&store);
        assert!(PreparedImport::from_file(directory.path(), &empty, &trust()).is_err());
        fs::write(&path, b"invalid envelope").unwrap();
        assert!(PreparedImport::from_file(&path, &empty, &trust()).is_err());
        File::create(&path)
            .unwrap()
            .set_len(crate::language_package::MAX_PACKAGE_BYTES as u64 + 1)
            .unwrap();
        assert!(matches!(
            PreparedImport::from_file(&path, &empty, &trust()),
            Err(StoreError::InvalidFile)
        ));
        assert_eq!(names(&store), before);
    }

    #[cfg(unix)]
    #[test]
    fn prepared_import_rejects_symlinks() {
        let (directory, store, empty) = setup();
        let path = directory.path().join("import.aklp");
        let link = directory.path().join("linked.aklp");
        fs::write(&path, artifact(1)).unwrap();
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let before = names(&store);
        assert!(PreparedImport::from_file(&link, &empty, &trust()).is_err());
        assert_eq!(names(&store), before);
    }

    #[test]
    fn explicit_initialization_and_missing_metadata_never_reset_state() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("store");
        assert!(PackageStore::open(&root).is_err());
        assert!(!root.exists());
        assert!(PackageStore::initialize(Path::new("relative/store")).is_err());
        let store = PackageStore::initialize(&root).unwrap();
        assert_eq!(store.load(&trust()).unwrap().inventory().generation(), 0);
        assert!(PackageStore::initialize(&root).is_err());
        let before = names(&store);
        let pointer = fs::read(root.join("CURRENT")).unwrap();
        fs::remove_file(root.join("CURRENT")).unwrap();
        assert!(store.load(&trust()).is_err());
        assert_eq!(names(&store).len(), before.len() - 1);
        fs::write(root.join("CURRENT"), pointer).unwrap();
        fs::remove_file(root.join("writer.lock")).unwrap();
        let snapshot = store.load(&trust()).unwrap();
        assert!(
            store
                .commit(&snapshot, snapshot.inventory(), &[], &trust())
                .is_err()
        );
        assert!(!root.join("writer.lock").exists());
    }

    #[test]
    fn import_reload_remove_reinstall_preserves_high_water_and_old_artifacts() {
        let (_directory, store, empty) = setup();
        let bytes = artifact(2);
        let candidate = stage(&empty, &bytes);
        let mut old_reader = open_file(&store.root.join("CURRENT"), false).unwrap();
        let committed = store
            .commit(&empty, &candidate, &[&bytes], &trust())
            .unwrap();
        let mut old_pointer = Vec::new();
        old_reader.read_to_end(&mut old_pointer).unwrap();
        assert_eq!(parse_pointer(&old_pointer).unwrap(), empty.state_hash);
        drop(old_reader);
        assert_eq!(empty.inventory().generation(), 0);
        assert_eq!(committed.inventory().generation(), 1);
        let reloaded = PackageStore::open(&store.root)
            .unwrap()
            .load(&trust())
            .unwrap();
        assert_eq!(reloaded.state_hash, committed.state_hash);
        let id = crate::PackId::parse("test-ru").unwrap();
        let projected =
            crate::installed_packages::InstalledPackages::from_store(&reloaded, &[id].into())
                .unwrap();
        assert!(
            projected
                .dictionaries
                .active(&id)
                .unwrap()
                .contains("hello")
        );
        assert_eq!(projected.dictionaries.installed_ids().count(), 2);
        assert!(
            projected
                .dictionaries
                .active(&crate::Language::English)
                .is_none()
        );
        assert_eq!(
            projected
                .catalogs
                .as_ref()
                .unwrap()
                .select("ru")
                .text("locale.self_name"),
            "Русский"
        );
        let before = names(&store);
        store
            .commit(&reloaded, reloaded.inventory(), &[], &trust())
            .unwrap();
        assert_eq!(before, names(&store));
        let ids = reloaded.inventory().receipts().map(|r| r.id()).collect();
        let removed = reloaded.inventory().stage_remove(&ids).unwrap();
        store.commit(&reloaded, &removed, &[], &trust()).unwrap();
        let removed = store.load(&trust()).unwrap();
        assert_eq!(removed.inventory().packages().count(), 0);
        let without =
            crate::installed_packages::InstalledPackages::from_store(&removed, &[id].into())
                .unwrap();
        assert!(without.dictionaries.active(&id).is_none());
        assert!(
            without
                .dictionaries
                .enabled_ids()
                .any(|enabled| *enabled == id)
        );
        assert!(
            projected
                .dictionaries
                .active(&id)
                .unwrap()
                .contains("hello")
        ); // prior immutable snapshot
        assert!(before.is_subset(&names(&store))); // CURRENT is replaced at the same name
        let older = Arc::new(VerifiedLanguagePackage::verify(&artifact(1), &trust()).unwrap());
        assert_eq!(
            removed.inventory().stage_import(&[older]).unwrap_err(),
            InventoryError::Rollback
        );
        let reinstalled = stage(&removed, &bytes);
        store.commit(&removed, &reinstalled, &[], &trust()).unwrap(); // reuse exact cached bytes
        assert_eq!(store.load(&trust()).unwrap().inventory().generation(), 3);
    }

    #[test]
    fn stale_candidates_and_dropped_history_are_rejected_before_publication() {
        let (_directory, store, empty) = setup();
        let bytes = artifact(2);
        let candidate = stage(&empty, &bytes);
        let first = store
            .commit(&empty, &candidate, &[&bytes], &trust())
            .unwrap();
        let before = names(&store);
        assert!(matches!(
            store.commit(&empty, &candidate, &[&bytes], &trust()),
            Err(StoreError::StaleSnapshot)
        ));
        assert_eq!(before, names(&store));
        // A valid isolated serialized inventory at the next generation is not
        // authority to erase the current store's history.
        let mut unrelated: serde_json::Value =
            serde_json::from_slice(&PackageInventory::default().state_bytes().unwrap()).unwrap();
        unrelated["generation"] = json!(2);
        let unrelated =
            PackageInventory::restore(&serde_json::to_vec(&unrelated).unwrap(), &[]).unwrap();
        assert!(matches!(
            store.commit(&first, &unrelated, &[], &trust()),
            Err(StoreError::Inventory(InventoryError::Rollback))
        ));
        assert_eq!(before, names(&store));
        assert_eq!(store.load(&trust()).unwrap().state_hash, first.state_hash);
    }

    #[test]
    fn writer_lock_is_nonblocking_and_released_on_drop() {
        const CHILD_ROOT: &str = "AUTOKEY_TEST_PACKAGE_LOCK_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let store = PackageStore::open(Path::new(&root)).unwrap();
            assert!(matches!(store.writer_lock(), Err(StoreError::Busy)));
            return;
        }
        let (_directory, store, empty) = setup();
        let lock = store.writer_lock().unwrap();
        assert!(matches!(store.writer_lock(), Err(StoreError::Busy)));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "package_store::tests::writer_lock_is_nonblocking_and_released_on_drop",
            ])
            .env(CHILD_ROOT, &store.root)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(matches!(
            store.commit(&empty, empty.inventory(), &[], &trust()),
            Err(StoreError::Busy)
        ));
        // Readers do not contend for the independent writer lock.
        assert_eq!(store.load(&trust()).unwrap().state_hash, empty.state_hash);
        drop(lock);
        store
            .commit(&empty, empty.inventory(), &[], &trust())
            .unwrap();
    }

    #[test]
    fn injected_interruptions_leave_whole_old_or_new_state_and_retry_safely() {
        for point in [
            CommitPoint::Validated,
            CommitPoint::BlobsPublished,
            CommitPoint::StatePublished,
            CommitPoint::BeforePointer,
            CommitPoint::PointerPublished,
        ] {
            let (_directory, store, empty) = setup();
            let bytes = artifact(1);
            let candidate = stage(&empty, &bytes);
            let result = store.commit_inner(&empty, &candidate, &[&bytes], &trust(), |at| {
                if at == point {
                    Err(io::Error::other("injected interruption"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err());
            let loaded = store.load(&trust()).unwrap();
            if point == CommitPoint::PointerPublished {
                assert!(matches!(result, Err(StoreError::CommitUncertain)));
                assert_eq!(loaded.inventory().generation(), 1);
                assert!(matches!(
                    store.commit(&empty, &candidate, &[&bytes], &trust()),
                    Err(StoreError::StaleSnapshot)
                ));
            } else {
                assert_eq!(loaded.state_hash, empty.state_hash);
                store
                    .commit(&empty, &candidate, &[&bytes], &trust())
                    .unwrap();
            }
            assert_eq!(
                store.load(&trust()).unwrap().inventory().packages().count(),
                1
            );
            assert!(!names(&store).iter().any(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("tmp-")
            }));
        }
    }

    #[test]
    fn artifact_set_trust_and_immutable_collisions_fail_before_pointer_change() {
        let (_directory, store, empty) = setup();
        let bytes = artifact(2);
        let candidate = stage(&empty, &bytes);
        let before = names(&store);
        assert!(matches!(
            store.commit(&empty, &candidate, &[], &trust()),
            Err(StoreError::MissingArtifact)
        ));
        assert!(matches!(
            store.commit(&empty, &candidate, &[&artifact(1)], &trust()),
            Err(StoreError::UnexpectedArtifact)
        ));
        assert!(matches!(
            store.commit(&empty, &candidate, &[&bytes, &bytes], &trust()),
            Err(StoreError::UnexpectedArtifact)
        ));
        assert!(matches!(
            store.commit(&empty, &candidate, &[&bytes], &PackageTrust::default()),
            Err(StoreError::Package(_))
        ));
        assert_eq!(before, names(&store));
        let destination = store.root.join(blob_name(&digest(&bytes)));
        fs::write(&destination, b"wrong").unwrap();
        assert!(matches!(
            store.commit(&empty, &candidate, &[&bytes], &trust()),
            Err(StoreError::Integrity)
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"wrong");
        assert_eq!(store.load(&trust()).unwrap().state_hash, empty.state_hash);
    }

    #[test]
    fn load_rejects_pointer_state_blob_tampering_and_changed_trust() {
        let (_directory, store, empty) = setup();
        let bytes = artifact(1);
        store
            .commit(&empty, &stage(&empty, &bytes), &[&bytes], &trust())
            .unwrap();
        let pointer_path = store.root.join("CURRENT");
        let pointer = fs::read(&pointer_path).unwrap();
        for malformed in [
            vec![],
            vec![b'a'; 65],
            vec![b'A'; 64],
            b"../../outside".to_vec(),
            vec![b'0'; 64],
        ] {
            fs::write(&pointer_path, malformed).unwrap();
            assert!(store.load(&trust()).is_err());
        }
        fs::write(&pointer_path, &pointer).unwrap();
        assert!(matches!(
            store.load(&PackageTrust::default()),
            Err(StoreError::Package(_))
        ));
        let state_path = store
            .root
            .join(state_name(&parse_pointer(&pointer).unwrap()));
        let state = fs::read(&state_path).unwrap();
        fs::write(&state_path, b"{}").unwrap();
        assert!(matches!(store.load(&trust()), Err(StoreError::Integrity)));
        fs::write(&state_path, state).unwrap();
        let blob = store.root.join(blob_name(&digest(&bytes)));
        fs::write(&blob, b"truncated").unwrap();
        assert!(matches!(store.load(&trust()), Err(StoreError::Integrity)));
        fs::remove_file(&blob).unwrap();
        assert!(store.load(&trust()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn nofollow_regular_file_checks_reject_symlinks_directories_and_fifos() {
        use std::os::unix::fs::symlink;
        let (_directory, store, empty) = setup();
        let pointer = store.root.join("CURRENT");
        let saved = fs::read(&pointer).unwrap();
        fs::remove_file(&pointer).unwrap();
        let target = store.root.join("unrelated");
        fs::write(&target, &saved).unwrap();
        symlink(&target, &pointer).unwrap();
        assert!(store.load(&trust()).is_err());
        fs::remove_file(&pointer).unwrap();
        symlink(store.root.join("absent"), &pointer).unwrap();
        assert!(store.load(&trust()).is_err());
        fs::remove_file(&pointer).unwrap();
        fs::create_dir(&pointer).unwrap();
        assert!(store.load(&trust()).is_err());
        fs::remove_dir(&pointer).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let cpath = std::ffi::CString::new(pointer.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(matches!(store.load(&trust()), Err(StoreError::InvalidFile)));
        fs::remove_file(&pointer).unwrap();
        fs::write(&pointer, saved).unwrap();
        let bytes = artifact(1);
        let blob = store.root.join(blob_name(&digest(&bytes)));
        symlink(&target, &blob).unwrap();
        assert!(
            store
                .commit(&empty, &stage(&empty, &bytes), &[&bytes], &trust())
                .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), fs::read(&pointer).unwrap());
        let alias = store.root.parent().unwrap().join("alias");
        symlink(&store.root, &alias).unwrap();
        assert!(PackageStore::open(&alias).is_err());
    }
}
