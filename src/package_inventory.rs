//! Immutable package transactions and bounded durable-state representation.
//! No filesystem writes: committing the state atomically belongs to the store.

use crate::{
    DictionaryRegistry, PackId,
    language_package::{VerifiedLanguagePackage, decode_hex},
    localization::CatalogRegistry,
    package_catalog::{CatalogCheckpoint, DownloadPlan, ReleaseError, repository_id},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

pub const MAX_INVENTORY_STATE_BYTES: usize = 256 * 1024;
pub const MAX_OPTIONAL_PACKAGES: usize = 63; // plus the protected English base
const MAX_HISTORY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryError {
    InvalidState,
    TooLarge,
    ProtectedBase,
    DuplicatePackage,
    Rollback,
    RevisionConflict,
    OwnershipConflict,
    GenerationExhausted,
    Plan(ReleaseError),
}
impl fmt::Display for InventoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "package_inventory_{self:?}")
    }
}
impl std::error::Error for InventoryError {}
impl From<ReleaseError> for InventoryError {
    fn from(error: ReleaseError) -> Self {
        Self::Plan(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageReceipt {
    id: PackId,
    revision: u64,
    bytes: u64,
    sha256: [u8; 32],
}
impl PackageReceipt {
    fn from_package(package: &VerifiedLanguagePackage) -> Self {
        Self {
            id: package.id(),
            revision: package.revision(),
            bytes: package.envelope_bytes(),
            sha256: package.envelope_sha256(),
        }
    }
    pub const fn id(self) -> PackId {
        self.id
    }
    pub const fn revision(self) -> u64 {
        self.revision
    }
    pub const fn bytes(self) -> u64 {
        self.bytes
    }
    pub const fn sha256(self) -> [u8; 32] {
        self.sha256
    }
}

#[derive(Debug, Clone, Default)]
pub struct PackageInventory {
    generation: u64,
    packages: BTreeMap<PackId, Arc<VerifiedLanguagePackage>>,
    highest_seen: BTreeMap<PackId, PackageReceipt>,
    // Exact immediately preceding installed artifact, not a downloaded candidate.
    rollback_targets: BTreeMap<PackId, PackageReceipt>,
    repository: Option<String>,
    catalog: Option<CatalogCheckpoint>,
}
impl PackageInventory {
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    pub fn packages(&self) -> impl Iterator<Item = &Arc<VerifiedLanguagePackage>> {
        self.packages.values()
    }
    pub fn receipts(&self) -> impl Iterator<Item = PackageReceipt> + '_ {
        self.packages
            .values()
            .map(|p| PackageReceipt::from_package(p))
    }
    pub fn highest_seen(&self, id: &PackId) -> Option<PackageReceipt> {
        self.highest_seen.get(id).copied()
    }
    /// Only an installed newer package is eligible. After rollback this returns
    /// None until a subsequent explicit update installs a newer revision again.
    pub fn rollback_target(&self, id: &PackId) -> Option<PackageReceipt> {
        let target = *self.rollback_targets.get(id)?;
        (self.packages.get(id)?.revision() > target.revision).then_some(target)
    }

    /// Separate deliberate rollback, never an exception in normal import or
    /// catalog installation. The exact previously installed envelope is needed.
    /// High-water marks and catalog freshness are retained in a new generation.
    pub fn stage_rollback(
        &self,
        package: Arc<VerifiedLanguagePackage>,
    ) -> Result<Self, InventoryError> {
        let receipt = PackageReceipt::from_package(&package);
        if is_base(receipt.id) {
            return Err(InventoryError::ProtectedBase);
        }
        if self.rollback_target(&receipt.id) != Some(receipt) {
            return Err(InventoryError::Rollback);
        }
        let mut next = self.clone();
        next.packages.insert(receipt.id, package);
        next.validate()?;
        next.finish(self)
    }
    /// Source of the catalog checkpoint, not provenance of every local import.
    pub fn repository(&self) -> Option<&str> {
        self.repository.as_deref()
    }
    pub const fn catalog_checkpoint(&self) -> Option<CatalogCheckpoint> {
        self.catalog
    }

    /// Stage an explicitly approved local import of already authenticated data.
    /// No selection, user lexicon or running snapshot is changed by this method.
    pub fn stage_import(
        &self,
        packages: &[Arc<VerifiedLanguagePackage>],
    ) -> Result<Self, InventoryError> {
        let mut next = self.clone();
        next.add_packages(packages)?;
        next.finish(self)
    }

    /// Verify the entire supplied set against the exact selected plan before
    /// producing a candidate. The caller still must obtain user confirmation.
    pub fn stage_plan(
        &self,
        plan: &DownloadPlan,
        packages: &[Arc<VerifiedLanguagePackage>],
        now: u64,
    ) -> Result<Self, InventoryError> {
        plan.check_current(now)?;
        if packages.len() != plan.packages().len() {
            return Err(InventoryError::InvalidState);
        }
        if self
            .repository
            .as_deref()
            .is_some_and(|repo| repo != plan.repository())
        {
            return Err(InventoryError::Plan(ReleaseError::WrongRepository));
        }
        if let Some(previous) = self.catalog {
            if plan.checkpoint().revision() < previous.revision() {
                return Err(InventoryError::Plan(ReleaseError::Rollback));
            }
            if plan.checkpoint().revision() == previous.revision() && plan.checkpoint() != previous
            {
                return Err(InventoryError::Plan(ReleaseError::RevisionConflict));
            }
        }
        for pinned in plan.packages() {
            let package = packages
                .iter()
                .find(|p| p.id() == pinned.id())
                .ok_or(InventoryError::InvalidState)?;
            pinned.check_package(package, now)?;
        }
        let mut next = self.clone();
        next.add_packages(packages)?;
        next.repository = Some(plan.repository().to_owned());
        next.catalog = Some(plan.checkpoint());
        next.finish(self)
    }

    /// Removal preserves version high-water marks. User preferences, dictionary
    /// overlays and exclusions are outside this inventory and cannot be erased.
    pub fn stage_remove(&self, ids: &BTreeSet<PackId>) -> Result<Self, InventoryError> {
        if ids.len() > 64 {
            return Err(InventoryError::TooLarge);
        }
        let mut next = self.clone();
        for id in ids {
            if is_base(*id) {
                return Err(InventoryError::ProtectedBase);
            }
            next.packages.remove(id);
            next.rollback_targets.remove(id);
        }
        next.finish(self)
    }

    fn add_packages(
        &mut self,
        packages: &[Arc<VerifiedLanguagePackage>],
    ) -> Result<(), InventoryError> {
        if packages.len() > MAX_OPTIONAL_PACKAGES {
            return Err(InventoryError::TooLarge);
        }
        let mut seen = BTreeSet::new();
        for package in packages {
            let receipt = PackageReceipt::from_package(package);
            if is_base(receipt.id) {
                return Err(InventoryError::ProtectedBase);
            }
            if !seen.insert(receipt.id) {
                return Err(InventoryError::DuplicatePackage);
            }
            if let Some(previous) = self.highest_seen.get(&receipt.id) {
                if receipt.revision < previous.revision {
                    return Err(InventoryError::Rollback);
                }
                if receipt.revision == previous.revision && receipt != *previous {
                    return Err(InventoryError::RevisionConflict);
                }
            }
            self.highest_seen.insert(receipt.id, receipt);
            if let Some(previous) = self.packages.get(&receipt.id)
                && previous.revision() < receipt.revision
            {
                self.rollback_targets
                    .insert(receipt.id, PackageReceipt::from_package(previous));
            }
            self.packages.insert(receipt.id, package.clone());
        }
        self.validate()
    }

    fn validate(&self) -> Result<(), InventoryError> {
        if self.packages.len() > MAX_OPTIONAL_PACKAGES || self.highest_seen.len() > MAX_HISTORY {
            return Err(InventoryError::TooLarge);
        }
        if self.receipts().map(|receipt| receipt.bytes).sum::<u64>()
            > crate::package_catalog::MAX_SELECTED_DOWNLOAD_BYTES
        {
            return Err(InventoryError::TooLarge);
        }
        let installed = self.receipts().collect::<Vec<_>>();
        validate_receipt_history(&installed, &self.highest_seen, &self.rollback_targets)?;
        // Locale and alias ownership must be unambiguous even if no UI locale is
        // currently selected. Input-profile collisions remain detector policy.
        self.catalog_snapshot()?;
        Ok(())
    }
    fn finish(mut self, previous: &Self) -> Result<Self, InventoryError> {
        let changed = self.receipts().collect::<Vec<_>>()
            != previous.receipts().collect::<Vec<_>>()
            || self.highest_seen != previous.highest_seen
            || self.rollback_targets != previous.rollback_targets
            || self.repository != previous.repository
            || self.catalog != previous.catalog;
        if changed {
            self.generation = previous
                .generation
                .checked_add(1)
                .ok_or(InventoryError::GenerationExhausted)?;
        }
        Ok(self)
    }

    /// Build all installed UI data before publishing the candidate snapshot.
    pub fn catalog_snapshot(&self) -> Result<CatalogRegistry, InventoryError> {
        let mut catalogs = CatalogRegistry::default();
        for package in self.packages.values() {
            if let Some(ui) = package.ui() {
                catalogs
                    .add(ui.as_bytes())
                    .map_err(|_| InventoryError::OwnershipConflict)?;
            }
        }
        Ok(catalogs)
    }
    /// The caller supplies its immutable embedded base. Installed data never
    /// enables a selected ID or removes a retained missing selection implicitly.
    pub fn dictionary_snapshot(
        &self,
        base: &DictionaryRegistry,
        selected: &BTreeSet<PackId>,
    ) -> Result<DictionaryRegistry, InventoryError> {
        let mut registry = base.clone();
        for package in self.packages.values() {
            if let Some(input) = package.input() {
                registry
                    .insert_shared(input.clone())
                    .map_err(|_| InventoryError::OwnershipConflict)?;
            }
        }
        registry
            .set_enabled(selected.iter().copied())
            .map_err(|_| InventoryError::TooLarge)?;
        Ok(registry)
    }

    pub fn state_bytes(&self) -> Result<Vec<u8>, InventoryError> {
        let state = StateDocument {
            format: if self.rollback_targets.is_empty() {
                1
            } else {
                2
            },
            generation: self.generation,
            repository: self.repository.clone(),
            catalog: self.catalog.map(|c| CheckpointDocument {
                revision: c.revision(),
                sha256: hex(&c.document_sha256()),
            }),
            installed: self.receipts().map(ReceiptDocument::from_receipt).collect(),
            highest_seen: self
                .highest_seen
                .values()
                .copied()
                .map(ReceiptDocument::from_receipt)
                .collect(),
            rollback_targets: self
                .rollback_targets
                .values()
                .copied()
                .map(ReceiptDocument::from_receipt)
                .collect(),
        };
        let bytes = serde_json::to_vec(&state).map_err(|_| InventoryError::InvalidState)?;
        if bytes.len() > MAX_INVENTORY_STATE_BYTES {
            return Err(InventoryError::TooLarge);
        }
        Ok(bytes)
    }

    /// Restore only with all currently installed artifacts already authenticated
    /// under the current trust policy. Missing, extra or changed artifacts fail.
    pub fn restore(
        bytes: &[u8],
        packages: &[Arc<VerifiedLanguagePackage>],
    ) -> Result<Self, InventoryError> {
        let state = ParsedState::parse(bytes)?;
        if state.installed.len() != packages.len() {
            return Err(InventoryError::InvalidState);
        }
        let mut supplied = BTreeMap::new();
        for package in packages {
            if supplied.insert(package.id(), package.clone()).is_some() {
                return Err(InventoryError::DuplicatePackage);
            }
        }
        let mut installed = BTreeMap::new();
        for receipt in state.installed {
            let package = supplied
                .get(&receipt.id)
                .ok_or(InventoryError::InvalidState)?;
            if receipt != PackageReceipt::from_package(package) {
                return Err(InventoryError::InvalidState);
            }
            installed.insert(receipt.id, package.clone());
        }
        let inventory = Self {
            generation: state.generation,
            packages: installed,
            highest_seen: state.highest_seen,
            rollback_targets: state.rollback_targets,
            repository: state.repository,
            catalog: state.catalog,
        };
        inventory.validate()?;
        Ok(inventory)
    }

    /// Validate the entire state structure before the store reads any blobs.
    /// Artifact authentication and cross-package ownership checks follow in restore.
    pub fn required_artifacts(bytes: &[u8]) -> Result<Vec<PackageReceipt>, InventoryError> {
        Ok(ParsedState::parse(bytes)?.installed)
    }

    /// Store-side guard against committing a candidate based on another history.
    /// A no-op is permitted; a change must be exactly one generation ahead.
    pub(crate) fn check_successor(&self, next: &Self) -> Result<(), InventoryError> {
        if self.state_bytes()? == next.state_bytes()? {
            return Ok(());
        }
        if self.generation.checked_add(1) != Some(next.generation) {
            return Err(InventoryError::InvalidState);
        }
        // Public restore is not authority to invent rollback history. Derive
        // every next target from the exact current-to-candidate transition.
        let mut targets = self.rollback_targets.clone();
        for old in self.receipts() {
            match next.packages.get(&old.id) {
                None => {
                    targets.remove(&old.id);
                }
                Some(package) => {
                    let new = PackageReceipt::from_package(package);
                    if new.revision > old.revision {
                        targets.insert(old.id, old);
                    } else if new.revision < old.revision {
                        if self.rollback_target(&old.id) != Some(new) {
                            return Err(InventoryError::Rollback);
                        }
                    } else if new != old {
                        return Err(InventoryError::RevisionConflict);
                    }
                }
            }
        }
        if next.rollback_targets != targets {
            return Err(InventoryError::InvalidState);
        }
        for (id, previous) in &self.highest_seen {
            let current = next.highest_seen.get(id).ok_or(InventoryError::Rollback)?;
            if current.revision < previous.revision {
                return Err(InventoryError::Rollback);
            }
            if current.revision == previous.revision && current != previous {
                return Err(InventoryError::RevisionConflict);
            }
        }
        if let Some(repository) = &self.repository
            && next.repository.as_ref() != Some(repository)
        {
            return Err(InventoryError::Plan(ReleaseError::WrongRepository));
        }
        if let Some(previous) = self.catalog {
            let current = next.catalog.ok_or(InventoryError::Rollback)?;
            if current.revision() < previous.revision() {
                return Err(InventoryError::Rollback);
            }
            if current.revision() == previous.revision() && current != previous {
                return Err(InventoryError::RevisionConflict);
            }
        }
        next.validate()
    }
}

pub(crate) fn is_base(id: PackId) -> bool {
    id == crate::Language::English
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_receipt_history(
    installed: &[PackageReceipt],
    highest: &BTreeMap<PackId, PackageReceipt>,
    targets: &BTreeMap<PackId, PackageReceipt>,
) -> Result<(), InventoryError> {
    if targets.len() > MAX_OPTIONAL_PACKAGES {
        return Err(InventoryError::TooLarge);
    }
    for receipt in installed {
        let high = highest
            .get(&receipt.id)
            .ok_or(InventoryError::InvalidState)?;
        if receipt != high
            && !(receipt.revision < high.revision && targets.get(&receipt.id) == Some(receipt))
        {
            return Err(InventoryError::InvalidState);
        }
    }
    for (id, target) in targets {
        if target.id != *id
            || !installed.iter().any(|receipt| receipt.id == *id)
            || highest
                .get(id)
                .is_none_or(|high| target.revision >= high.revision)
        {
            return Err(InventoryError::InvalidState);
        }
    }
    Ok(())
}

struct ParsedState {
    generation: u64,
    installed: Vec<PackageReceipt>,
    highest_seen: BTreeMap<PackId, PackageReceipt>,
    rollback_targets: BTreeMap<PackId, PackageReceipt>,
    repository: Option<String>,
    catalog: Option<CatalogCheckpoint>,
}
impl ParsedState {
    fn parse(bytes: &[u8]) -> Result<Self, InventoryError> {
        if bytes.len() > MAX_INVENTORY_STATE_BYTES {
            return Err(InventoryError::TooLarge);
        }
        let state: StateDocument =
            serde_json::from_slice(bytes).map_err(|_| InventoryError::InvalidState)?;
        if !matches!(state.format, 1 | 2)
            || (state.format == 1 && !state.rollback_targets.is_empty())
            || (state.format == 2 && state.rollback_targets.is_empty())
        {
            return Err(InventoryError::InvalidState);
        }
        if state.installed.len() > MAX_OPTIONAL_PACKAGES
            || state.highest_seen.len() > MAX_HISTORY
            || state.rollback_targets.len() > MAX_OPTIONAL_PACKAGES
        {
            return Err(InventoryError::TooLarge);
        }
        let mut highest_seen = BTreeMap::new();
        for row in state.highest_seen {
            let receipt = row.parse()?;
            if highest_seen.insert(receipt.id, receipt).is_some() {
                return Err(InventoryError::InvalidState);
            }
        }
        let mut seen = BTreeSet::new();
        let mut rollback_targets = BTreeMap::new();
        for row in state.rollback_targets {
            let receipt = row.parse()?;
            if rollback_targets.insert(receipt.id, receipt).is_some() {
                return Err(InventoryError::InvalidState);
            }
        }
        let mut total_bytes = 0;
        let mut installed = Vec::new();
        for row in state.installed {
            let receipt = row.parse()?;
            if !seen.insert(receipt.id) {
                return Err(InventoryError::InvalidState);
            }
            total_bytes += receipt.bytes;
            if total_bytes > crate::package_catalog::MAX_SELECTED_DOWNLOAD_BYTES {
                return Err(InventoryError::TooLarge);
            }
            installed.push(receipt);
        }
        validate_receipt_history(&installed, &highest_seen, &rollback_targets)?;
        let (repository, catalog) = match (state.repository, state.catalog) {
            (Some(repo), Some(checkpoint)) => (
                Some(repository_id(&repo)?),
                Some(CatalogCheckpoint::from_persisted(
                    checkpoint.revision,
                    decode_hex(&checkpoint.sha256).map_err(|_| InventoryError::InvalidState)?,
                )?),
            ),
            (None, None) => (None, None),
            _ => return Err(InventoryError::InvalidState),
        };
        if state.generation == 0
            && (!installed.is_empty() || !highest_seen.is_empty() || catalog.is_some())
        {
            return Err(InventoryError::InvalidState);
        }
        Ok(Self {
            generation: state.generation,
            installed,
            highest_seen,
            rollback_targets,
            repository,
            catalog,
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StateDocument {
    format: u32,
    generation: u64,
    repository: Option<String>,
    catalog: Option<CheckpointDocument>,
    installed: Vec<ReceiptDocument>,
    highest_seen: Vec<ReceiptDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rollback_targets: Vec<ReceiptDocument>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckpointDocument {
    revision: u64,
    sha256: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptDocument {
    id: String,
    revision: u64,
    bytes: u64,
    sha256: String,
}
impl ReceiptDocument {
    fn from_receipt(receipt: PackageReceipt) -> Self {
        Self {
            id: receipt.id.to_string(),
            revision: receipt.revision,
            bytes: receipt.bytes,
            sha256: hex(&receipt.sha256),
        }
    }
    fn parse(self) -> Result<PackageReceipt, InventoryError> {
        let id = PackId::parse(&self.id).map_err(|_| InventoryError::InvalidState)?;
        if is_base(id)
            || self.revision == 0
            || self.bytes == 0
            || self.bytes > crate::language_package::MAX_PACKAGE_BYTES as u64
        {
            return Err(InventoryError::InvalidState);
        }
        Ok(PackageReceipt {
            id,
            revision: self.revision,
            bytes: self.bytes,
            sha256: decode_hex(&self.sha256).map_err(|_| InventoryError::InvalidState)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{language_package::PackageTrust, package_catalog::VerifiedReleaseCatalog};
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    // Public synthetic fixtures only; not release signing material.
    fn key() -> SigningKey {
        SigningKey::from_bytes(&[44; 32])
    }
    fn trust() -> PackageTrust {
        PackageTrust::from_public_keys([("test-only".into(), key().verifying_key().to_bytes())])
            .unwrap()
    }
    fn package(
        id: &str,
        revision: u64,
        locale: Option<&str>,
        aliases: &[&str],
        word: &str,
    ) -> Arc<VerifiedLanguagePackage> {
        let mut components = json!({"words":format!("{word}\n"),"short_words":"hi\n","scoring":include_str!("../data/scoring/en-US.json"),"input":serde_json::to_string(&json!({"format":1,"pack_id":id,"windows_keyboard_profiles":[{"profile":"0409:00000409","required_capabilities":["physical-key-v1"]}]})).unwrap(),"license":"Test fixture license.","notice":"Synthetic test data."});
        if let Some(locale) = locale {
            components["ui"] = json!(serde_json::to_string(&json!({"format":1,"locale":locale,"direction":"ltr","aliases":aliases,"messages":{"locale.self_name":"Test"}})).unwrap());
        }
        let mut integrity = json!({});
        for (role, data) in components.as_object().unwrap() {
            let data = data.as_str().unwrap();
            integrity[role] =
                json!({"bytes":data.len(),"sha256":hex(&Sha256::digest(data.as_bytes()))});
        }
        let manifest = serde_json::to_string(&json!({"format":1,"package_id":id,"revision":revision,"runtime_api":1,"input_pack":id,"ui_locale":locale,"components":integrity})).unwrap();
        let mut signed = b"AutoKeyboardLayot.language-package.v1\0".to_vec();
        signed.extend_from_slice(manifest.as_bytes());
        let bytes = serde_json::to_vec(&json!({"format":1,"signer":"test-only","manifest":manifest,"signature":hex(&key().sign(&signed).to_bytes()),"components":components})).unwrap();
        Arc::new(VerifiedLanguagePackage::verify(&bytes, &trust()).unwrap())
    }
    fn plan(
        packages: &[Arc<VerifiedLanguagePackage>],
        revision: u64,
        selected: bool,
    ) -> DownloadPlan {
        let entries: Vec<_> = packages.iter().map(|p| json!({"package_id":p.id().as_str(),"revision":p.revision(),"bytes":p.envelope_bytes(),"sha256":hex(&p.envelope_sha256()),"tag":"v1","asset":format!("{}.aklp",p.id()),"runtime_api":1,"input":p.input().is_some(),"ui_locale":p.ui_locale()})).collect();
        let document = serde_json::to_string(&json!({"format":1,"repository":"example/packs","revision":revision,"issued_at":100,"expires_at":200,"packages":entries})).unwrap();
        let mut signed = b"AutoKeyboardLayot.release-catalog.v1\0".to_vec();
        signed.extend_from_slice(document.as_bytes());
        let bytes = serde_json::to_vec(&json!({"format":1,"signer":"test-only","catalog":document,"signature":hex(&key().sign(&signed).to_bytes())})).unwrap();
        let catalog =
            VerifiedReleaseCatalog::verify(&bytes, &trust(), "example/packs", None, 100).unwrap();
        let ids = if selected {
            packages.iter().map(|p| p.id()).collect()
        } else {
            BTreeSet::new()
        };
        catalog.select(&ids, 100).unwrap()
    }

    #[test]
    fn imports_are_atomic_idempotent_and_do_not_mutate_previous_snapshots() {
        let old = package("custom-test", 1, None, &[], "hello");
        let original = PackageInventory::default();
        let first = original.stage_import(std::slice::from_ref(&old)).unwrap();
        assert_eq!(original.generation(), 0);
        assert_eq!(original.packages().count(), 0);
        assert_eq!(first.generation(), 1);
        assert_eq!(first.stage_import(&[old]).unwrap().generation(), 1);
        let newer = package("custom-test", 2, None, &[], "world");
        let forbidden = package("en-US", 1, None, &[], "hello");
        assert_eq!(
            first.stage_import(&[newer.clone(), forbidden]).unwrap_err(),
            InventoryError::ProtectedBase
        );
        assert_eq!(first.generation(), 1);
        assert_eq!(first.receipts().next().unwrap().revision(), 1);
        let updated = first.stage_import(&[newer]).unwrap();
        assert_eq!(updated.generation(), 2);
        assert_eq!(updated.receipts().next().unwrap().revision(), 2);
        assert_eq!(first.receipts().next().unwrap().revision(), 1);
    }
    #[test]
    fn explicit_rollback_preserves_highwater_and_exact_previous_receipt() {
        let old = package("custom-test", 1, None, &[], "hello");
        let new = package("custom-test", 3, None, &[], "world");
        let first = PackageInventory::default()
            .stage_import(std::slice::from_ref(&old))
            .unwrap();
        let accepted = plan(&[], 7, false);
        let first = first.stage_plan(&accepted, &[], 100).unwrap();
        assert_eq!(first.rollback_target(&old.id()), None);
        let upgraded = first.stage_import(std::slice::from_ref(&new)).unwrap();
        first.check_successor(&upgraded).unwrap();
        assert_eq!(upgraded.rollback_target(&old.id()), first.receipts().next());
        assert!(upgraded.stage_import(std::slice::from_ref(&old)).is_err());
        let rolled = upgraded.stage_rollback(old.clone()).unwrap();
        upgraded.check_successor(&rolled).unwrap();
        assert_eq!(rolled.generation(), upgraded.generation() + 1);
        assert_eq!(rolled.repository(), upgraded.repository());
        assert_eq!(rolled.catalog_checkpoint(), upgraded.catalog_checkpoint());
        assert!(rolled.stage_plan(&plan(&[], 6, false), &[], 100).is_err());
        assert_eq!(
            rolled.highest_seen(&old.id()),
            upgraded.highest_seen(&old.id())
        );
        assert_eq!(rolled.rollback_target(&old.id()), None);
        assert!(rolled.stage_rollback(old.clone()).is_err());
        assert!(rolled.stage_import(std::slice::from_ref(&old)).is_err());
        assert!(
            rolled
                .stage_import(&[package("custom-test", 2, None, &[], "intermediate")])
                .is_err()
        );
        assert!(
            upgraded
                .stage_rollback(package("custom-test", 1, None, &[], "changed"))
                .is_err()
        );
        assert!(
            upgraded
                .stage_rollback(package("custom-test", 2, None, &[], "unseen"))
                .is_err()
        );
        let restored =
            PackageInventory::restore(&rolled.state_bytes().unwrap(), std::slice::from_ref(&old))
                .unwrap();
        assert_eq!(
            restored.state_bytes().unwrap(),
            rolled.state_bytes().unwrap()
        );
        let reupdated = restored.stage_import(&[new]).unwrap();
        restored.check_successor(&reupdated).unwrap();
        assert_eq!(
            reupdated.rollback_target(&old.id()),
            first.receipts().next()
        );
        let removed = rolled.stage_remove(&BTreeSet::from([old.id()])).unwrap();
        rolled.check_successor(&removed).unwrap();
        assert!(removed.rollback_targets.is_empty());
        assert!(removed.stage_rollback(old).is_err());
    }

    #[test]
    fn successor_rejects_invented_rollback_history_and_ownership_conflicts() {
        let old = package("custom-test", 1, Some("ru"), &[], "hello");
        let new = package("custom-test", 2, Some("et"), &[], "world");
        let first = PackageInventory::default()
            .stage_import(std::slice::from_ref(&old))
            .unwrap();
        let updated = first.stage_import(std::slice::from_ref(&new)).unwrap();
        let collision = updated
            .stage_import(&[package("another", 1, Some("ru"), &[], "word")])
            .unwrap();
        assert_eq!(
            collision.stage_rollback(old.clone()).unwrap_err(),
            InventoryError::OwnershipConflict
        );
        // Structurally valid persisted data is not permission to add a target
        // never recorded by this current inventory's installation transition.
        let direct = PackageInventory::default().stage_import(&[new]).unwrap();
        let mut forged = updated.clone();
        forged.generation = direct.generation() + 1;
        assert_eq!(
            direct.check_successor(&forged),
            Err(InventoryError::InvalidState)
        );
        let forged = PackageInventory::restore(
            &forged.state_bytes().unwrap(),
            &forged.packages().cloned().collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            direct
                .check_successor(&forged.stage_rollback(old).unwrap())
                .is_err()
        );
    }

    #[test]
    fn removal_keeps_revision_history_and_refuses_reinstall_downgrades() {
        let old = package("custom-test", 1, None, &[], "hello");
        let newer = package("custom-test", 2, None, &[], "hello");
        let original = PackageInventory::default()
            .stage_import(std::slice::from_ref(&newer))
            .unwrap();
        let removed = original
            .stage_remove(&BTreeSet::from([newer.id()]))
            .unwrap();
        assert_eq!(removed.packages().count(), 0);
        assert_eq!(removed.highest_seen(&newer.id()).unwrap().revision(), 2);
        assert_eq!(
            removed.stage_import(&[old]).unwrap_err(),
            InventoryError::Rollback
        );
        let changed = package("custom-test", 2, None, &[], "world");
        assert_eq!(
            removed.stage_import(&[changed]).unwrap_err(),
            InventoryError::RevisionConflict
        );
        assert!(removed.stage_import(std::slice::from_ref(&newer)).is_ok());
        assert_eq!(
            removed
                .stage_remove(&BTreeSet::from([newer.id()]))
                .unwrap()
                .generation(),
            removed.generation()
        );
        assert_eq!(
            removed
                .stage_remove(&BTreeSet::from([crate::Language::English]))
                .unwrap_err(),
            InventoryError::ProtectedBase
        );
    }
    #[test]
    fn locale_and_alias_ownership_conflicts_reject_the_whole_candidate() {
        let first = package("custom-first", 1, Some("ru"), &["ru-RU"], "hello");
        let inventory = PackageInventory::default().stage_import(&[first]).unwrap();
        for locale in ["ru", "ru-RU"] {
            let collision = package("custom-second", 1, Some(locale), &[], "world");
            assert_eq!(
                inventory.stage_import(&[collision]).unwrap_err(),
                InventoryError::OwnershipConflict
            );
            assert_eq!(inventory.packages().count(), 1);
        }
    }
    #[test]
    fn restored_inventory_requires_exact_artifacts_and_preserves_tombstones() {
        let pack = package("custom-test", 2, None, &[], "hello");
        let inventory = PackageInventory::default()
            .stage_import(std::slice::from_ref(&pack))
            .unwrap();
        let bytes = inventory.state_bytes().unwrap();
        assert_eq!(
            PackageInventory::required_artifacts(&bytes).unwrap(),
            inventory.receipts().collect::<Vec<_>>()
        );
        let restored = PackageInventory::restore(&bytes, std::slice::from_ref(&pack)).unwrap();
        assert_eq!(restored.state_bytes().unwrap(), bytes);
        assert!(PackageInventory::restore(&bytes, &[]).is_err());
        assert!(
            PackageInventory::restore(&bytes, &[package("custom-test", 2, None, &[], "world")])
                .is_err()
        );
        let removed = inventory
            .stage_remove(&BTreeSet::from([pack.id()]))
            .unwrap();
        let restored = PackageInventory::restore(&removed.state_bytes().unwrap(), &[]).unwrap();
        assert_eq!(
            restored.highest_seen(&pack.id()),
            inventory.highest_seen(&pack.id())
        );
        assert_eq!(
            restored
                .stage_import(&[package("custom-test", 1, None, &[], "hello")])
                .unwrap_err(),
            InventoryError::Rollback
        );
    }
    #[test]
    fn malformed_state_and_unbounded_blob_requests_are_rejected_before_loading() {
        let pack = package("custom-test", 1, None, &[], "hello");
        let inventory = PackageInventory::default()
            .stage_import(std::slice::from_ref(&pack))
            .unwrap();
        let original: Value = serde_json::from_slice(&inventory.state_bytes().unwrap()).unwrap();
        for (field, value) in [
            ("format", json!(2)),
            ("generation", json!(0)),
            ("repository", json!("example/packs")),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert!(
                PackageInventory::required_artifacts(&serde_json::to_vec(&changed).unwrap())
                    .is_err()
            );
            assert!(
                PackageInventory::restore(
                    &serde_json::to_vec(&changed).unwrap(),
                    std::slice::from_ref(&pack)
                )
                .is_err()
            );
        }
        let mut changed = original.clone();
        changed["highest_seen"][0]["revision"] = json!(2);
        assert!(
            PackageInventory::required_artifacts(&serde_json::to_vec(&changed).unwrap()).is_err()
        );
        assert!(
            PackageInventory::restore(
                &serde_json::to_vec(&changed).unwrap(),
                std::slice::from_ref(&pack)
            )
            .is_err()
        );
        changed = original.clone();
        let duplicate = changed["installed"][0].clone();
        changed["installed"].as_array_mut().unwrap().push(duplicate);
        assert!(
            PackageInventory::required_artifacts(&serde_json::to_vec(&changed).unwrap()).is_err()
        );
        changed = original;
        changed["installed"][0]["id"] = json!("../outside");
        assert!(
            PackageInventory::required_artifacts(&serde_json::to_vec(&changed).unwrap()).is_err()
        );
        assert_eq!(
            PackageInventory::required_artifacts(&vec![b' '; MAX_INVENTORY_STATE_BYTES + 1])
                .unwrap_err(),
            InventoryError::TooLarge
        );
    }
    #[test]
    fn plans_are_checked_as_a_complete_set_with_attached_catalog_receipts() {
        let pack = package("custom-test", 1, None, &[], "hello");
        let chosen = plan(std::slice::from_ref(&pack), 2, true);
        let inventory = PackageInventory::default()
            .stage_plan(&chosen, std::slice::from_ref(&pack), 100)
            .unwrap();
        assert_eq!(inventory.repository(), Some("example/packs"));
        assert_eq!(inventory.catalog_checkpoint(), Some(chosen.checkpoint()));
        assert!(inventory.stage_plan(&chosen, &[], 100).is_err());
        assert!(
            inventory
                .stage_plan(&chosen, &[pack.clone(), pack.clone()], 100)
                .is_err()
        );
        assert_eq!(
            inventory
                .stage_plan(
                    &plan(std::slice::from_ref(&pack), 1, true),
                    std::slice::from_ref(&pack),
                    100
                )
                .unwrap_err(),
            InventoryError::Plan(ReleaseError::Rollback)
        );
        let empty = plan(&[pack], 3, false);
        assert_eq!(
            inventory.stage_plan(&empty, &[], 200).unwrap_err(),
            InventoryError::Plan(ReleaseError::NotCurrent)
        );
        let refreshed = inventory.stage_plan(&empty, &[], 100).unwrap();
        assert_eq!(refreshed.packages().count(), 1);
        let restored = PackageInventory::restore(
            &refreshed.state_bytes().unwrap(),
            &refreshed.packages().cloned().collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(restored.catalog_checkpoint(), Some(empty.checkpoint()));
    }
    #[test]
    fn dictionary_snapshots_keep_explicit_missing_selections_without_enabling_installs() {
        let pack = package("custom-test", 1, None, &[], "hello");
        let inventory = PackageInventory::default()
            .stage_import(std::slice::from_ref(&pack))
            .unwrap();
        let base = DictionaryRegistry::embedded();
        let missing = PackId::parse("missing-pack").unwrap();
        let selected = BTreeSet::from([pack.id(), crate::Language::Russian, missing]);
        let previous = inventory.dictionary_snapshot(&base, &selected).unwrap();
        assert!(previous.active(&pack.id()).unwrap().contains("hello"));
        assert!(previous.active(&crate::Language::English).is_none());
        assert_eq!(
            previous.enabled_ids().copied().collect::<BTreeSet<_>>(),
            selected
        );
        let removed = inventory
            .stage_remove(&BTreeSet::from([pack.id()]))
            .unwrap();
        let next = removed.dictionary_snapshot(&base, &selected).unwrap();
        assert!(next.active(&pack.id()).is_none());
        assert!(previous.active(&pack.id()).is_some());
        assert_eq!(
            next.enabled_ids().copied().collect::<BTreeSet<_>>(),
            selected
        );
    }
    #[test]
    fn capacity_and_generation_limits_leave_previous_state_unchanged() {
        let packages: Vec<_> = (0..64)
            .map(|i| package(&format!("pack-{i}"), 1, None, &[], "hello"))
            .collect();
        let inventory = PackageInventory::default()
            .stage_import(&packages[..63])
            .unwrap();
        assert_eq!(inventory.packages().count(), 63);
        assert_eq!(
            inventory.stage_import(&packages[63..]).unwrap_err(),
            InventoryError::TooLarge
        );
        assert_eq!(inventory.packages().count(), 63);
        let mut exhausted = inventory.clone();
        exhausted.generation = u64::MAX;
        assert_eq!(
            exhausted
                .stage_remove(&BTreeSet::from([packages[0].id()]))
                .unwrap_err(),
            InventoryError::GenerationExhausted
        );
        assert_eq!(exhausted.packages().count(), 63);
        assert_eq!(
            exhausted.stage_import(&packages[..1]).unwrap().generation(),
            u64::MAX
        );
        let rows: Vec<_> = (0..9).map(|i| json!({"id":format!("pack-{i}"),"revision":1,"bytes":crate::language_package::MAX_PACKAGE_BYTES,"sha256":"00".repeat(32)})).collect();
        let state = json!({"format":1,"generation":1,"repository":null,"catalog":null,"installed":rows,"highest_seen":rows});
        assert_eq!(
            PackageInventory::required_artifacts(&serde_json::to_vec(&state).unwrap()).unwrap_err(),
            InventoryError::TooLarge
        );
    }
}
