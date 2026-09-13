//! Authenticated release selection. Pure planning: this module never downloads.

use crate::{
    PackId,
    language_package::{
        MAX_PACKAGE_BYTES, PackageError, PackageTrust, VerifiedLanguagePackage, decode_hex,
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub(crate) const DOMAIN: &[u8] = b"AutoKeyboardLayot.release-catalog.v1\0";
/// User-approved publication source; does not provision a signing key.
pub const DEFAULT_PACKAGE_REPOSITORY: &str = "woffko/AutoKeyboardLayot";

/// An existing authenticated store binding takes precedence over the default.
/// An explicit request cannot silently move that store to another repository.
pub fn resolve_repository_source(
    requested: &str,
    bound: Option<&str>,
) -> Result<String, ReleaseError> {
    let requested = requested.trim();
    let repository = repository_id(if requested.is_empty() {
        bound.unwrap_or(DEFAULT_PACKAGE_REPOSITORY)
    } else {
        requested
    })?;
    if let Some(bound) = bound
        && repository_id(bound)? != repository
    {
        return Err(ReleaseError::WrongRepository);
    }
    Ok(repository)
}
pub(crate) const MAX_CATALOG_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const MAX_LIFETIME_SECONDS: u64 = 31 * 24 * 60 * 60;
pub const MAX_SELECTED_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseError {
    Authentication(PackageError),
    InvalidData,
    TooLarge,
    Incompatible,
    WrongRepository,
    NotCurrent,
    Rollback,
    RevisionConflict,
    MissingSelection,
    ArtifactMismatch,
}
impl fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "release_catalog_{self:?}")
    }
}
impl std::error::Error for ReleaseError {}
impl From<PackageError> for ReleaseError {
    fn from(value: PackageError) -> Self {
        Self::Authentication(value)
    }
}

/// A receipt from previously authenticated metadata. Persistence and writer
/// locking belong to the manager; supplying None does not provide rollback defense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogCheckpoint {
    revision: u64,
    document_sha256: [u8; 32],
}
impl CatalogCheckpoint {
    pub fn from_persisted(revision: u64, document_sha256: [u8; 32]) -> Result<Self, ReleaseError> {
        if revision == 0 {
            return Err(ReleaseError::InvalidData);
        }
        Ok(Self {
            revision,
            document_sha256,
        })
    }
    pub const fn revision(self) -> u64 {
        self.revision
    }
    pub const fn document_sha256(self) -> [u8; 32] {
        self.document_sha256
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: u32,
    signer: String,
    catalog: String,
    signature: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: u32,
    repository: String,
    revision: u64,
    issued_at: u64,
    expires_at: u64,
    packages: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    package_id: String,
    revision: u64,
    bytes: u64,
    sha256: String,
    tag: String,
    asset: String,
    runtime_api: u32,
    input: bool,
    ui_locale: Option<String>,
}

/// All fields originate in authenticated metadata, not a caller-supplied URL.
#[derive(Debug, Clone)]
pub struct PinnedPackage {
    id: PackId,
    revision: u64,
    bytes: u64,
    sha256: [u8; 32],
    url: String,
    asset: String,
    runtime_api: u32,
    input: bool,
    ui_locale: Option<String>,
    issued_at: u64,
    expires_at: u64,
}
impl PinnedPackage {
    pub const fn id(&self) -> PackId {
        self.id
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
    pub const fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    /// Exact authenticated asset file name from the release catalog. It is a
    /// single safe path segment validated during catalog verification.
    pub fn asset(&self) -> &str {
        &self.asset
    }
    pub const fn includes_input(&self) -> bool {
        self.input
    }
    pub fn ui_locale(&self) -> Option<&str> {
        self.ui_locale.as_deref()
    }
    pub const fn compatible(&self) -> bool {
        self.runtime_api == 1
    }

    /// Reject stale/incompatible selections before starting a network request.
    pub fn check_current(&self, now: u64) -> Result<(), ReleaseError> {
        if now < self.issued_at || now >= self.expires_at {
            return Err(ReleaseError::NotCurrent);
        }
        if !self.compatible() {
            return Err(ReleaseError::Incompatible);
        }
        Ok(())
    }

    /// Recheck freshness and exact transport bytes before parsing package data.
    /// The download/commit layer must also apply its persisted revision policy.
    pub fn verify_download(
        &self,
        bytes: &[u8],
        trust: &PackageTrust,
        now: u64,
    ) -> Result<VerifiedLanguagePackage, ReleaseError> {
        self.check_current(now)?;
        if bytes.len() as u64 != self.bytes
            || <[u8; 32]>::from(Sha256::digest(bytes)) != self.sha256
        {
            return Err(ReleaseError::ArtifactMismatch);
        }
        let package = VerifiedLanguagePackage::verify(bytes, trust)?;
        self.check_package(&package, now)?;
        Ok(package)
    }

    pub(crate) fn check_package(
        &self,
        package: &VerifiedLanguagePackage,
        now: u64,
    ) -> Result<(), ReleaseError> {
        self.check_current(now)?;
        if package.id() != self.id
            || package.revision() != self.revision
            || package.envelope_bytes() != self.bytes
            || package.envelope_sha256() != self.sha256
            || package.input().is_some() != self.input
            || package.ui_locale() != self.ui_locale.as_deref()
        {
            return Err(ReleaseError::ArtifactMismatch);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct DownloadPlan {
    packages: Vec<PinnedPackage>,
    total_bytes: u64,
    checkpoint: CatalogCheckpoint,
    repository: String,
    issued_at: u64,
    expires_at: u64,
}
impl DownloadPlan {
    pub fn packages(&self) -> &[PinnedPackage] {
        &self.packages
    }
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
    pub const fn checkpoint(&self) -> CatalogCheckpoint {
        self.checkpoint
    }
    pub fn repository(&self) -> &str {
        &self.repository
    }
    pub(crate) fn check_current(&self, now: u64) -> Result<(), ReleaseError> {
        if now < self.issued_at || now >= self.expires_at {
            Err(ReleaseError::NotCurrent)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
pub struct VerifiedReleaseCatalog {
    checkpoint: CatalogCheckpoint,
    repository: String,
    packages: BTreeMap<PackId, PinnedPackage>,
    issued_at: u64,
    expires_at: u64,
}
impl VerifiedReleaseCatalog {
    /// expected_repository and previous must come from trusted application/store
    /// policy. A package/catalog must never choose its own repository or receipt.
    pub fn verify(
        bytes: &[u8],
        trust: &PackageTrust,
        expected_repository: &str,
        previous: Option<CatalogCheckpoint>,
        now: u64,
    ) -> Result<Self, ReleaseError> {
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err(ReleaseError::TooLarge);
        }
        let envelope: Envelope =
            serde_json::from_slice(bytes).map_err(|_| ReleaseError::InvalidData)?;
        if envelope.format != 1 {
            return Err(ReleaseError::Incompatible);
        }
        trust.verify_document(
            DOMAIN,
            &envelope.signer,
            &envelope.catalog,
            &envelope.signature,
            MAX_DOCUMENT_BYTES,
        )?;
        let document: Document =
            serde_json::from_str(&envelope.catalog).map_err(|_| ReleaseError::InvalidData)?;
        if document.format != 1 || document.revision == 0 {
            return Err(ReleaseError::Incompatible);
        }
        let repository = repository_id(&document.repository)?;
        if repository != repository_id(expected_repository)? {
            return Err(ReleaseError::WrongRepository);
        }
        if document.expires_at <= document.issued_at
            || document.expires_at - document.issued_at > MAX_LIFETIME_SECONDS
            || now < document.issued_at
            || now >= document.expires_at
        {
            return Err(ReleaseError::NotCurrent);
        }
        let checkpoint = CatalogCheckpoint {
            revision: document.revision,
            document_sha256: Sha256::digest(envelope.catalog.as_bytes()).into(),
        };
        if let Some(previous) = previous {
            if checkpoint.revision < previous.revision {
                return Err(ReleaseError::Rollback);
            }
            if checkpoint.revision == previous.revision
                && checkpoint.document_sha256 != previous.document_sha256
            {
                return Err(ReleaseError::RevisionConflict);
            }
        }
        if document.packages.len() > 64 {
            return Err(ReleaseError::TooLarge);
        }
        let mut packages = BTreeMap::new();
        let mut assets = BTreeSet::new();
        for entry in document.packages {
            let id = PackId::parse(&entry.package_id).map_err(|_| ReleaseError::InvalidData)?;
            if entry.revision == 0
                || entry.bytes == 0
                || entry.bytes > MAX_PACKAGE_BYTES as u64
                || !safe_segment(&entry.tag, 64)
                || entry.tag.eq_ignore_ascii_case("latest")
                || !safe_segment(&entry.asset, 128)
                || !entry.asset.ends_with(".aklp")
            {
                return Err(ReleaseError::InvalidData);
            }
            let ui_locale = entry
                .ui_locale
                .map(|locale| {
                    crate::localization::normalize_locale(&locale)
                        .map_err(|_| ReleaseError::InvalidData)
                })
                .transpose()?;
            if (!entry.input && ui_locale.is_none())
                || ui_locale.as_deref().is_some_and(|locale| {
                    locale == "system" || locale.split('-').next() == Some("en")
                })
            {
                return Err(ReleaseError::InvalidData);
            }
            if !assets.insert((
                entry.tag.to_ascii_lowercase(),
                entry.asset.to_ascii_lowercase(),
            )) {
                return Err(ReleaseError::InvalidData);
            }
            let package = PinnedPackage {
                id,
                revision: entry.revision,
                bytes: entry.bytes,
                sha256: decode_hex(&entry.sha256)?,
                url: format!(
                    "https://github.com/{repository}/releases/download/{}/{}",
                    entry.tag, entry.asset
                ),
                asset: entry.asset,
                runtime_api: entry.runtime_api,
                input: entry.input,
                ui_locale,
                issued_at: document.issued_at,
                expires_at: document.expires_at,
            };
            if packages.insert(id, package).is_some() {
                return Err(ReleaseError::InvalidData);
            }
        }
        Ok(Self {
            checkpoint,
            repository,
            packages,
            issued_at: document.issued_at,
            expires_at: document.expires_at,
        })
    }
    pub const fn checkpoint(&self) -> CatalogCheckpoint {
        self.checkpoint
    }
    pub fn packages(&self) -> impl Iterator<Item = &PinnedPackage> {
        self.packages.values()
    }

    /// An empty selection produces an empty plan. Missing/incompatible requested
    /// IDs fail the whole plan; no implicit dependencies or unselected downloads.
    pub fn select(&self, ids: &BTreeSet<PackId>, now: u64) -> Result<DownloadPlan, ReleaseError> {
        if now < self.issued_at || now >= self.expires_at {
            return Err(ReleaseError::NotCurrent);
        }
        if ids.len() > 64 {
            return Err(ReleaseError::TooLarge);
        }
        let mut packages = Vec::new();
        let mut total_bytes: u64 = 0;
        for id in ids {
            let package = self
                .packages
                .get(id)
                .ok_or(ReleaseError::MissingSelection)?;
            if !package.compatible() {
                return Err(ReleaseError::Incompatible);
            }
            total_bytes = total_bytes
                .checked_add(package.bytes)
                .ok_or(ReleaseError::TooLarge)?;
            if total_bytes > MAX_SELECTED_DOWNLOAD_BYTES {
                return Err(ReleaseError::TooLarge);
            }
            packages.push(package.clone());
        }
        Ok(DownloadPlan {
            packages,
            total_bytes,
            checkpoint: self.checkpoint,
            repository: self.repository.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
        })
    }
}

fn safe_segment(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
pub(crate) fn repository_id(value: &str) -> Result<String, ReleaseError> {
    let Some((owner, repo)) = value.split_once('/') else {
        return Err(ReleaseError::InvalidData);
    };
    if !safe_segment(owner, 100) || !safe_segment(repo, 100) {
        return Err(ReleaseError::InvalidData);
    }
    Ok(value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    #[test]
    fn approved_default_source_preserves_existing_bindings() {
        use super::{ReleaseError, resolve_repository_source};
        assert_eq!(
            resolve_repository_source("", None).unwrap(),
            "woffko/autokeyboardlayot"
        );
        assert_eq!(
            resolve_repository_source("  ", Some("example/packs")).unwrap(),
            "example/packs"
        );
        assert_eq!(
            resolve_repository_source("woffko/AutoKeyboardLayot", Some("woffko/autokeyboardlayot"))
                .unwrap(),
            "woffko/autokeyboardlayot"
        );
        assert_eq!(
            resolve_repository_source("woffko/AutoKeyboardLayot", Some("example/packs"))
                .unwrap_err(),
            ReleaseError::WrongRepository
        );
        assert!(resolve_repository_source("", Some("invalid")).is_err());
    }

    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};

    // Synthetic public test fixture, never used to sign a release.
    fn key() -> SigningKey {
        SigningKey::from_bytes(&[43; 32])
    }
    fn trust() -> PackageTrust {
        PackageTrust::from_public_keys([("test-only".into(), key().verifying_key().to_bytes())])
            .unwrap()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    fn entry(id: &str) -> Value {
        json!({"package_id":id,"revision":1,"bytes":100,"sha256":"00".repeat(32),"tag":"v1.0.0","asset":format!("{id}.aklp"),"runtime_api":1,"input":true})
    }
    fn document() -> Value {
        json!({"format":1,"repository":"example/language-packs","revision":1,"issued_at":100,"expires_at":200,"packages":[entry("ru-RU"),entry("et-EE")]})
    }
    fn signed(document: &Value) -> Vec<u8> {
        let text = serde_json::to_string(document).unwrap();
        let mut message = DOMAIN.to_vec();
        message.extend_from_slice(text.as_bytes());
        serde_json::to_vec(&json!({"format":1,"signer":"test-only","catalog":text,"signature":hex(&key().sign(&message).to_bytes())})).unwrap()
    }
    fn verify(
        document: &Value,
        previous: Option<CatalogCheckpoint>,
        now: u64,
    ) -> Result<VerifiedReleaseCatalog, ReleaseError> {
        VerifiedReleaseCatalog::verify(
            &signed(document),
            &trust(),
            "example/language-packs",
            previous,
            now,
        )
    }
    fn ids(names: &[&str]) -> BTreeSet<PackId> {
        names.iter().map(|id| PackId::parse(id).unwrap()).collect()
    }

    #[test]
    fn plans_only_explicit_selections_including_empty_selection() {
        let catalog = verify(&document(), None, 100).unwrap();
        let none = catalog.select(&BTreeSet::new(), 100).unwrap();
        assert!(none.packages().is_empty());
        assert_eq!(none.total_bytes(), 0);
        assert_eq!(none.checkpoint(), catalog.checkpoint());
        assert_eq!(none.repository(), "example/language-packs");
        let one = catalog.select(&ids(&["ru-RU"]), 100).unwrap();
        assert_eq!(one.packages().len(), 1);
        assert_eq!(one.total_bytes(), 100);
        assert_eq!(one.packages()[0].id().as_str(), "ru-ru");
        assert_eq!(
            one.packages()[0].url(),
            "https://github.com/example/language-packs/releases/download/v1.0.0/ru-RU.aklp"
        );
        assert_eq!(
            catalog
                .select(&ids(&["ru-RU", "et-EE"]), 100)
                .unwrap()
                .total_bytes(),
            200
        );
        assert_eq!(
            catalog
                .select(&ids(&["ru-RU", "missing-pack"]), 100)
                .unwrap_err(),
            ReleaseError::MissingSelection
        );
        let mut future = document();
        future["packages"][1]["runtime_api"] = json!(2);
        let catalog = verify(&future, None, 100).unwrap();
        assert!(catalog.select(&ids(&["ru-RU"]), 100).is_ok());
        assert_eq!(
            catalog.select(&ids(&["et-EE"]), 100).unwrap_err(),
            ReleaseError::Incompatible
        );
    }
    #[test]
    fn checkpoints_reject_rollback_and_same_revision_replacement() {
        let old = document();
        let first = verify(&old, None, 100).unwrap();
        let checkpoint = first.checkpoint();
        assert!(verify(&old, Some(checkpoint), 100).is_ok());
        let persisted =
            CatalogCheckpoint::from_persisted(checkpoint.revision(), checkpoint.document_sha256())
                .unwrap();
        assert_eq!(persisted, checkpoint);
        let mut changed = old.clone();
        changed["expires_at"] = json!(201);
        assert_eq!(
            verify(&changed, Some(checkpoint), 100).unwrap_err(),
            ReleaseError::RevisionConflict
        );
        changed["revision"] = json!(2);
        let next = verify(&changed, Some(checkpoint), 100).unwrap();
        assert_eq!(
            verify(&old, Some(next.checkpoint()), 100).unwrap_err(),
            ReleaseError::Rollback
        );
        assert!(CatalogCheckpoint::from_persisted(0, [0; 32]).is_err());
    }
    #[test]
    fn freshness_is_checked_at_inspection_and_selection_boundaries() {
        let doc = document();
        assert_eq!(
            verify(&doc, None, 99).unwrap_err(),
            ReleaseError::NotCurrent
        );
        let catalog = verify(&doc, None, 100).unwrap();
        assert!(catalog.select(&ids(&["ru-RU"]), 199).is_ok());
        assert_eq!(
            catalog.select(&ids(&["ru-RU"]), 200).unwrap_err(),
            ReleaseError::NotCurrent
        );
        assert_eq!(
            verify(&doc, None, 200).unwrap_err(),
            ReleaseError::NotCurrent
        );
        for expires in [99, 100, 100 + MAX_LIFETIME_SECONDS + 1] {
            let mut changed = doc.clone();
            changed["expires_at"] = json!(expires);
            assert_eq!(
                verify(&changed, None, 100).unwrap_err(),
                ReleaseError::NotCurrent
            );
        }
    }
    #[test]
    fn signatures_and_repository_policy_are_independent_of_untrusted_metadata() {
        let bytes = signed(&document());
        assert!(
            VerifiedReleaseCatalog::verify(
                &bytes,
                &PackageTrust::default(),
                "example/language-packs",
                None,
                100
            )
            .is_err()
        );
        assert_eq!(
            VerifiedReleaseCatalog::verify(&bytes, &trust(), "different/repository", None, 100)
                .unwrap_err(),
            ReleaseError::WrongRepository
        );
        let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
        envelope["catalog"] = json!(format!("{} ", envelope["catalog"].as_str().unwrap()));
        assert_eq!(
            VerifiedReleaseCatalog::verify(
                &serde_json::to_vec(&envelope).unwrap(),
                &trust(),
                "example/language-packs",
                None,
                100
            )
            .unwrap_err(),
            ReleaseError::Authentication(PackageError::InvalidSignature)
        );
        envelope["signature"] = json!(hex(&key()
            .sign(envelope["catalog"].as_str().unwrap().as_bytes())
            .to_bytes()));
        assert!(
            VerifiedReleaseCatalog::verify(
                &serde_json::to_vec(&envelope).unwrap(),
                &trust(),
                "example/language-packs",
                None,
                100
            )
            .is_err()
        );
    }
    #[test]
    fn unsafe_paths_duplicate_identities_and_ambiguous_assets_are_rejected() {
        for field in ["tag", "asset"] {
            for invalid in [
                "../evil",
                "https://evil.example/file.aklp",
                "x%2ffile.aklp",
                "a\\file.aklp",
                "a?b.aklp",
                "latest",
                "",
            ] {
                let mut doc = document();
                doc["packages"][0][field] = json!(invalid);
                assert!(verify(&doc, None, 100).is_err());
            }
        }
        let mut doc = document();
        doc["packages"][1]["package_id"] = json!("RU-ru");
        assert_eq!(
            verify(&doc, None, 100).unwrap_err(),
            ReleaseError::InvalidData
        );
        doc = document();
        doc["packages"][1]["asset"] = doc["packages"][0]["asset"].clone();
        assert_eq!(
            verify(&doc, None, 100).unwrap_err(),
            ReleaseError::InvalidData
        );
        for repo in [
            "owner/../repo",
            "https://github.com/owner/repo",
            "../repo",
            "owner/repo?x",
            "owner/",
            "owner\\repo",
        ] {
            assert!(repository_id(repo).is_err());
        }
        assert_eq!(
            repository_id("Example/Language-Packs").unwrap(),
            "example/language-packs"
        );
    }
    #[test]
    fn catalog_and_download_plan_limits_fail_without_partial_plans() {
        let mut doc = document();
        doc["packages"] = json!(
            (0..64)
                .map(|i| entry(&format!("pack-{i}")))
                .collect::<Vec<_>>()
        );
        assert_eq!(verify(&doc, None, 100).unwrap().packages().count(), 64);
        doc["packages"]
            .as_array_mut()
            .unwrap()
            .push(entry("pack-64"));
        assert_eq!(verify(&doc, None, 100).unwrap_err(), ReleaseError::TooLarge);
        doc["packages"] = json!(
            (0..9)
                .map(|i| {
                    let mut e = entry(&format!("pack-{i}"));
                    e["bytes"] = json!(MAX_PACKAGE_BYTES);
                    e
                })
                .collect::<Vec<_>>()
        );
        let catalog = verify(&doc, None, 100).unwrap();
        let selected = (0..8)
            .map(|i| PackId::parse(&format!("pack-{i}")).unwrap())
            .collect();
        assert_eq!(
            catalog.select(&selected, 100).unwrap().total_bytes(),
            MAX_SELECTED_DOWNLOAD_BYTES
        );
        let selected = catalog.packages().map(PinnedPackage::id).collect();
        assert_eq!(
            catalog.select(&selected, 100).unwrap_err(),
            ReleaseError::TooLarge
        );
    }
    #[test]
    fn malformed_or_oversized_metadata_cannot_supply_a_partial_catalog() {
        for (field, value) in [
            ("bytes", json!(0)),
            ("bytes", json!(MAX_PACKAGE_BYTES + 1)),
            ("revision", json!(0)),
            ("sha256", json!("bad")),
            ("ui_locale", json!("en-US")),
            ("input", json!(false)),
            ("dependencies", json!([])),
        ] {
            let mut doc = document();
            doc["packages"][0][field] = value;
            assert!(verify(&doc, None, 100).is_err());
        }
        assert_eq!(
            VerifiedReleaseCatalog::verify(
                &vec![b' '; MAX_CATALOG_BYTES + 1],
                &trust(),
                "example/language-packs",
                None,
                100
            )
            .unwrap_err(),
            ReleaseError::TooLarge
        );
        let mut envelope: Value = serde_json::from_slice(&signed(&document())).unwrap();
        let text = envelope["catalog"].as_str().unwrap().replacen(
            "\"revision\":1",
            "\"revision\":1,\"revision\":1",
            1,
        );
        let mut message = DOMAIN.to_vec();
        message.extend_from_slice(text.as_bytes());
        envelope["catalog"] = json!(text);
        envelope["signature"] = json!(hex(&key().sign(&message).to_bytes()));
        assert_eq!(
            VerifiedReleaseCatalog::verify(
                &serde_json::to_vec(&envelope).unwrap(),
                &trust(),
                "example/language-packs",
                None,
                100
            )
            .unwrap_err(),
            ReleaseError::InvalidData
        );
    }
    fn ui_package() -> Vec<u8> {
        let components = json!({"ui":r#"{"format":1,"locale":"ru","direction":"ltr","messages":{"locale.self_name":"Русский"}}"#,"license":"Test fixture only.","notice":"Synthetic test data."});
        let mut integrity = json!({});
        for (role, data) in components.as_object().unwrap() {
            let data = data.as_str().unwrap();
            integrity[role] =
                json!({"bytes":data.len(),"sha256":hex(&Sha256::digest(data.as_bytes()))});
        }
        let manifest = serde_json::to_string(&json!({"format":1,"package_id":"ru-RU","revision":1,"runtime_api":1,"ui_locale":"ru","components":integrity})).unwrap();
        let mut message = b"AutoKeyboardLayot.language-package.v1\0".to_vec();
        message.extend_from_slice(manifest.as_bytes());
        serde_json::to_vec(&json!({"format":1,"signer":"test-only","manifest":manifest,"signature":hex(&key().sign(&message).to_bytes()),"components":components})).unwrap()
    }

    #[test]
    fn installer_page_starts_clear_preserves_failed_choices_and_pins_confirmation() {
        use crate::{
            installer_packages::InstallerPackageSelection, package_install::PreparedCatalog,
            package_store::PackageStore,
        };
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let empty = store.load(&trust()).unwrap();
        let (doc, package) = downloadable_fixture();
        let make = || {
            InstallerPackageSelection::new(
                PreparedCatalog::from_bytes(
                    signed(&doc),
                    "example/language-packs",
                    &empty,
                    &trust(),
                    100,
                )
                .unwrap(),
            )
        };
        let mut page = make();
        assert_eq!(page.selected_count(), 0);
        assert_eq!(page.total_bytes(), 0);
        assert!(page.rows().iter().all(|row| !row.selected));
        let initial_display = page.page_data().unwrap();
        assert!(initial_display.is_ascii());
        assert!(
            initial_display
                .starts_with("[catalog]\r\nformat=1\r\ncount=2\r\nselected=0\r\ntotal_bytes=0\r\n")
        );
        assert!(
            initial_display.contains("ui_locale=ru\r\ninput=0\r\ncompatible=1\r\nselected=0\r\n")
        );
        assert!(make().confirm_download(&trust(), 100).unwrap().is_none());
        let ru = PackId::parse("ru-RU").unwrap();
        page.set_selected(ru, true, &trust(), 100).unwrap();
        assert_eq!(page.total_bytes(), package.len() as u64);
        assert!(
            page.page_data()
                .unwrap()
                .contains(&format!("selected=1\r\ntotal_bytes={}\r\n", package.len()))
        );
        assert!(
            page.set_selected(PackId::parse("missing").unwrap(), true, &trust(), 100)
                .is_err()
        );
        assert_eq!(page.selected_count(), 1);
        assert_eq!(page.total_bytes(), package.len() as u64);
        assert_eq!(
            store.load(&trust()).unwrap().state_sha256(),
            empty.state_sha256()
        );
        let transaction = page.confirm_download(&trust(), 100).unwrap().unwrap();
        assert_eq!(transaction.plan().packages().len(), 1);
        assert_eq!(transaction.plan().packages()[0].id(), ru);
        let mut revoked = make();
        revoked.set_selected(ru, true, &trust(), 100).unwrap();
        assert!(
            revoked
                .confirm_download(&PackageTrust::default(), 100)
                .is_err()
        );
    }

    #[test]
    fn installer_session_cancellation_revokes_late_review_and_install_approval_is_single_use() {
        use crate::{
            installer_session::InstallerSession, package_download::DownloadedPackage,
            package_install::PreparedCatalog, package_store::PackageStore,
        };
        use std::sync::atomic::AtomicBool;
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let (doc, package) = downloadable_fixture();
        let catalog = || {
            PreparedCatalog::from_bytes(
                signed(&doc),
                "example/language-packs",
                &store.load(&trust()).unwrap(),
                &trust(),
                100,
            )
            .unwrap()
        };
        let mut session = InstallerSession::default();
        assert!(session.begin_download(&trust(), 100).is_err());
        session.show_catalog(catalog()).unwrap();
        session
            .selection()
            .unwrap()
            .set_selected(PackId::parse("ru-RU").unwrap(), true, &trust(), 100)
            .unwrap();
        let (old_ticket, selected) = session.begin_download(&trust(), 100).unwrap().unwrap();
        assert!(session.selection().is_err());
        assert!(session.begin_download(&trust(), 100).is_err());
        let ready = selected
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |pin| DownloadedPackage::from_bytes(pin, package.clone(), &trust(), 100),
            )
            .unwrap();
        session.cancel().unwrap();
        assert!(session.finish_download(old_ticket, Ok(ready)).is_err());
        assert!(session.review().is_err());
        assert!(session.begin_install(old_ticket).is_err());
        assert_eq!(
            store.load(&trust()).unwrap().inventory().packages().count(),
            0
        );

        session.show_catalog(catalog()).unwrap();
        session
            .selection()
            .unwrap()
            .set_selected(PackId::parse("ru-RU").unwrap(), true, &trust(), 100)
            .unwrap();
        let (ticket, selected) = session.begin_download(&trust(), 100).unwrap().unwrap();
        assert_ne!(ticket, old_ticket);
        let ready = selected
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |pin| DownloadedPackage::from_bytes(pin, package.clone(), &trust(), 100),
            )
            .unwrap();
        session.finish_download(ticket, Ok(ready)).unwrap();
        assert_eq!(session.review().unwrap().0, ticket);
        assert!(session.begin_install(old_ticket).is_err());
        let _approved = session.begin_install(ticket).unwrap();
        assert!(session.begin_install(ticket).is_err());
        assert!(session.cancel().is_err());
        assert!(session.finish_install(old_ticket).is_err());
        session.finish_install(ticket).unwrap();
        assert!(session.finish_install(ticket).is_err());
        // Handing work to a worker is not a commit; the actual confirm call was
        // intentionally not made in this session-state test.
        assert_eq!(
            store.load(&trust()).unwrap().inventory().packages().count(),
            0
        );
    }

    #[test]
    fn installer_replacing_selection_invalidates_previous_view_without_partial_changes() {
        use crate::{
            installer_session::InstallerSession, package_install::PreparedCatalog,
            package_store::PackageStore,
        };
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let (doc, _) = downloadable_fixture();
        let catalog = PreparedCatalog::from_bytes(
            signed(&doc),
            "example/language-packs",
            &store.load(&trust()).unwrap(),
            &trust(),
            100,
        )
        .unwrap();
        let mut session = InstallerSession::default();
        session.show_catalog(catalog).unwrap();
        let old_view = session.view();
        session
            .replace_selection(old_view, ids(&["ru-RU"]), &trust(), 100)
            .unwrap();
        assert_ne!(session.view(), old_view);
        assert!(
            session
                .replace_selection(old_view, BTreeSet::new(), &trust(), 100)
                .is_err()
        );
        assert_eq!(session.selection().unwrap().selected_count(), 1);
        let current_view = session.view();
        assert!(
            session
                .replace_selection(current_view, ids(&["missing"]), &trust(), 100)
                .is_err()
        );
        assert_eq!(session.view(), current_view);
        assert_eq!(session.selection().unwrap().selected_count(), 1);
    }

    fn downloadable_fixture() -> (Value, Vec<u8>) {
        let package = ui_package();
        let mut doc = document(); // the unselected ET record must never be fetched
        doc["packages"][0]["bytes"] = json!(package.len());
        doc["packages"][0]["sha256"] = json!(hex(&Sha256::digest(&package)));
        doc["packages"][0]["input"] = json!(false);
        doc["packages"][0]["ui_locale"] = json!("ru");
        (doc, package)
    }

    #[test]
    fn online_selection_download_confirmation_and_exact_cache_reuse_are_separate() {
        use crate::{
            package_download::DownloadedPackage, package_install::PreparedCatalog,
            package_store::PackageStore,
        };
        use std::sync::atomic::AtomicBool;
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let empty = store.load(&trust()).unwrap();
        let (doc, package) = downloadable_fixture();
        let catalog = PreparedCatalog::from_bytes(
            signed(&doc),
            "example/language-packs",
            &empty,
            &trust(),
            100,
        )
        .unwrap();
        let selection = catalog.select(&ids(&["ru-RU"]), &trust(), 100).unwrap();
        assert_eq!(selection.plan().packages().len(), 1);
        assert_eq!(selection.plan().total_bytes(), package.len() as u64);
        assert_eq!(
            store.load(&trust()).unwrap().state_sha256(),
            empty.state_sha256()
        );
        let calls = std::cell::Cell::new(0);
        let ready = selection
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |pin| {
                    calls.set(calls.get() + 1);
                    assert_eq!(pin.id(), PackId::parse("ru-RU").unwrap());
                    DownloadedPackage::from_bytes(pin, package.clone(), &trust(), 100)
                },
            )
            .unwrap();
        let accepted = store.load(&trust()).unwrap();
        assert_eq!(accepted.inventory().packages().count(), 0);
        assert_eq!(
            accepted
                .inventory()
                .catalog_checkpoint()
                .unwrap()
                .revision(),
            1
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(ready.packages().len(), 1);
        let installed = ready.confirm(&store, &trust(), 100).unwrap();
        assert_eq!(installed.inventory().packages().count(), 1);
        let catalog = PreparedCatalog::from_bytes(
            signed(&doc),
            "example/language-packs",
            &installed,
            &trust(),
            100,
        )
        .unwrap();
        let ready = catalog
            .select(&ids(&["ru-RU"]), &trust(), 100)
            .unwrap()
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |_| panic!("exact cached artifact must not be downloaded"),
            )
            .unwrap();
        assert_eq!(
            ready.confirm(&store, &trust(), 100).unwrap().state_sha256(),
            installed.state_sha256()
        );
    }

    #[test]
    fn local_catalog_resolves_adjacent_artifact_without_network() {
        use crate::{package_install::PreparedCatalog, package_store::PackageStore};
        use std::sync::atomic::AtomicBool;
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let empty = store.load(&trust()).unwrap();
        let (doc, package) = downloadable_fixture();
        let catalog_dir = directory.path().join("catalogs with spaces");
        std::fs::create_dir(&catalog_dir).unwrap();
        std::fs::write(catalog_dir.join("ru-RU.aklp"), &package).unwrap();
        let catalog = PreparedCatalog::from_local_bytes(
            signed(&doc),
            "example/language-packs",
            catalog_dir,
            &empty,
            &trust(),
            100,
        )
        .unwrap();
        let ready = catalog
            .select(&ids(&["ru-RU"]), &trust(), 100)
            .unwrap()
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |_| panic!("a local catalog must never fetch over the network"),
            )
            .unwrap();
        assert_eq!(ready.packages().len(), 1);
        let installed = ready.confirm(&store, &trust(), 100).unwrap();
        assert_eq!(installed.inventory().packages().count(), 1);
        assert!(matches!(
            PreparedCatalog::from_local_bytes(
                signed(&doc),
                "example/language-packs",
                std::path::PathBuf::from("relative"),
                &empty,
                &trust(),
                100,
            ),
            Err(crate::package_install::InstallError::Catalog(
                ReleaseError::InvalidData
            ))
        ));
    }

    #[test]
    fn local_catalog_refuses_missing_tampered_or_substituted_artifacts() {
        use crate::package_install::PreparedCatalog;
        use crate::package_store::PackageStore;
        use std::sync::atomic::AtomicBool;
        let (doc, package) = downloadable_fixture();
        // Each attempt needs a fresh store: accepting a download persists the
        // catalog receipt even when artifact retrieval fails.
        let run = |contents: Option<&[u8]>| -> bool {
            let directory = tempfile::tempdir().unwrap();
            let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
            let empty = store.load(&trust()).unwrap();
            if let Some(contents) = contents {
                std::fs::write(directory.path().join("ru-RU.aklp"), contents).unwrap();
            }
            PreparedCatalog::from_local_bytes(
                signed(&doc),
                "example/language-packs",
                directory.path().to_path_buf(),
                &empty,
                &trust(),
                100,
            )
            .unwrap()
            .select(&ids(&["ru-RU"]), &trust(), 100)
            .unwrap()
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |_| panic!("local source must not fetch"),
            )
            .is_ok()
        };
        assert!(!run(None), "missing artifact must fail closed");
        let mut tampered = package.clone();
        tampered.push(0);
        assert!(!run(Some(&tampered)), "tampered bytes must fail the pin");
        assert!(run(Some(&package)), "correct bytes must install");
    }

    #[test]
    fn cancelled_online_download_keeps_accepted_receipt_but_installs_nothing() {
        use crate::{
            package_download::DownloadedPackage,
            package_install::{InstallError, PreparedCatalog},
            package_store::PackageStore,
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let empty = store.load(&trust()).unwrap();
        let (doc, package) = downloadable_fixture();
        let catalog = PreparedCatalog::from_bytes(
            signed(&doc),
            "example/language-packs",
            &empty,
            &trust(),
            100,
        )
        .unwrap();
        assert!(matches!(
            catalog.select(&BTreeSet::new(), &trust(), 100),
            Err(InstallError::EmptySelection)
        ));
        let cancelled = AtomicBool::new(true);
        let result = catalog
            .select(&ids(&["ru-RU"]), &trust(), 100)
            .unwrap()
            .accept_and_download_with(
                &store,
                &trust(),
                &cancelled,
                || Ok(100),
                |_| panic!("cancelled before acceptance"),
            );
        assert!(matches!(result, Err(InstallError::Cancelled)));
        assert_eq!(
            store.load(&trust()).unwrap().state_sha256(),
            empty.state_sha256()
        );
        cancelled.store(false, Ordering::Relaxed);
        let result = catalog
            .select(&ids(&["ru-RU"]), &trust(), 100)
            .unwrap()
            .accept_and_download_with(
                &store,
                &trust(),
                &cancelled,
                || Ok(100),
                |pin| {
                    cancelled.store(true, Ordering::Relaxed);
                    DownloadedPackage::from_bytes(pin, package.clone(), &trust(), 100)
                },
            );
        assert!(matches!(result, Err(InstallError::Cancelled)));
        let accepted = store.load(&trust()).unwrap();
        assert_eq!(accepted.inventory().packages().count(), 0);
        assert_eq!(
            accepted
                .inventory()
                .catalog_checkpoint()
                .unwrap()
                .revision(),
            1
        );
        assert!(
            std::fs::read_dir(directory.path().join("packages"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("blob-"))
        );
    }

    #[test]
    fn online_install_cannot_replace_a_newer_catalog_snapshot_or_revoked_trust() {
        use crate::{
            package_download::DownloadedPackage,
            package_install::{InstallError, PreparedCatalog},
            package_store::{PackageStore, StoreError},
        };
        use std::sync::atomic::AtomicBool;
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let empty = store.load(&trust()).unwrap();
        let (doc, package) = downloadable_fixture();
        let prepare = |snapshot: &crate::package_store::StoreSnapshot| {
            PreparedCatalog::from_bytes(
                signed(&doc),
                "example/language-packs",
                snapshot,
                &trust(),
                100,
            )
            .unwrap()
            .select(&ids(&["ru-RU"]), &trust(), 100)
            .unwrap()
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |pin| DownloadedPackage::from_bytes(pin, package.clone(), &trust(), 100),
            )
            .unwrap()
        };
        let ready = prepare(&empty);
        let accepted = store.load(&trust()).unwrap();
        assert!(
            ready
                .confirm(&store, &PackageTrust::default(), 100)
                .is_err()
        );
        assert_eq!(
            store.load(&trust()).unwrap().state_sha256(),
            accepted.state_sha256()
        );
        let ready = prepare(&accepted);
        let mut newer = doc.clone();
        newer["revision"] = json!(2);
        let newer_plan = verify(&newer, accepted.inventory().catalog_checkpoint(), 100)
            .unwrap()
            .select(&BTreeSet::new(), 100)
            .unwrap();
        let next = accepted
            .inventory()
            .stage_plan(&newer_plan, &[], 100)
            .unwrap();
        let current = store.commit(&accepted, &next, &[], &trust()).unwrap();
        assert!(matches!(
            ready.confirm(&store, &trust(), 100),
            Err(InstallError::Store(StoreError::StaleSnapshot))
        ));
        assert_eq!(
            store.load(&trust()).unwrap().state_sha256(),
            current.state_sha256()
        );
        assert_eq!(current.inventory().packages().count(), 0);
    }

    #[test]
    fn online_fetch_cannot_substitute_an_unselected_verified_package() {
        use crate::{
            package_download::DownloadedPackage, package_install::PreparedCatalog,
            package_store::PackageStore,
        };
        use std::sync::atomic::AtomicBool;
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let empty = store.load(&trust()).unwrap();
        let (doc, package) = downloadable_fixture();
        let catalog = PreparedCatalog::from_bytes(
            signed(&doc),
            "example/language-packs",
            &empty,
            &trust(),
            100,
        )
        .unwrap();
        let ru = catalog
            .packages()
            .find(|p| p.id() == PackId::parse("ru-RU").unwrap())
            .unwrap()
            .clone();
        let result = catalog
            .select(&ids(&["et-EE"]), &trust(), 100)
            .unwrap()
            .accept_and_download_with(
                &store,
                &trust(),
                &AtomicBool::new(false),
                || Ok(100),
                |_| DownloadedPackage::from_bytes(&ru, package.clone(), &trust(), 100),
            );
        assert!(result.is_err());
        assert_eq!(
            store.load(&trust()).unwrap().inventory().packages().count(),
            0
        );
    }
    #[test]
    fn downloaded_artifact_must_match_pinned_bytes_identity_revision_and_components() {
        let package = ui_package();
        let mut doc = document();
        doc["packages"] = json!([entry("ru-RU")]);
        doc["packages"][0]["bytes"] = json!(package.len());
        doc["packages"][0]["sha256"] = json!(hex(&Sha256::digest(&package)));
        doc["packages"][0]["input"] = json!(false);
        doc["packages"][0]["ui_locale"] = json!("ru");
        let catalog = verify(&doc, None, 100).unwrap();
        let plan = catalog.select(&ids(&["ru-RU"]), 100).unwrap();
        let pinned = &plan.packages()[0];
        assert_eq!(pinned.check_current(99), Err(ReleaseError::NotCurrent));
        assert_eq!(pinned.check_current(100), Ok(()));
        assert_eq!(pinned.check_current(200), Err(ReleaseError::NotCurrent));
        let downloaded = crate::package_download::DownloadedPackage::from_bytes(
            pinned,
            package.clone(),
            &trust(),
            100,
        )
        .unwrap();
        assert_eq!(downloaded.bytes(), package.as_slice());
        assert_eq!(downloaded.package().id(), pinned.id());
        assert!(matches!(
            crate::package_download::DownloadedPackage::from_bytes(
                pinned,
                package.clone(),
                &trust(),
                200,
            ),
            Err(crate::package_download::DownloadError::Verification(
                ReleaseError::NotCurrent
            ))
        ));
        assert!(pinned.verify_download(&package, &trust(), 199).is_ok());
        assert_eq!(
            pinned.verify_download(&package, &trust(), 200).unwrap_err(),
            ReleaseError::NotCurrent
        );
        assert_eq!(
            pinned
                .verify_download(&package[..package.len() - 1], &trust(), 100)
                .unwrap_err(),
            ReleaseError::ArtifactMismatch
        );
        let mut tampered = package.clone();
        tampered[0] = b' ';
        assert_eq!(
            pinned
                .verify_download(&tampered, &trust(), 100)
                .unwrap_err(),
            ReleaseError::ArtifactMismatch
        );
        for (field, value) in [
            ("package_id", json!("other-pack")),
            ("revision", json!(2)),
            ("input", json!(true)),
            ("ui_locale", json!("et")),
        ] {
            let mut changed = doc.clone();
            changed["packages"][0][field] = value;
            let catalog = verify(&changed, None, 100).unwrap();
            let pinned = catalog.packages().next().unwrap();
            assert_eq!(
                pinned.verify_download(&package, &trust(), 100).unwrap_err(),
                ReleaseError::ArtifactMismatch
            );
        }
    }
}
