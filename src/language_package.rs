//! Authenticated data-only package decoding. No filesystem, network or activation.
//!
//! Format 1 uses a fixed-role JSON envelope, not executable plugins or an archive
//! with attacker-selected extraction paths. The exact decoded manifest bytes are
//! signed with a domain prefix; its lengths and SHA-256 hashes bind all payloads.

use crate::{DictionaryPack, InputPackDescriptor, PackId, ScoringModel};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, sync::Arc};

pub const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_MANIFEST_BYTES: usize = 32 * 1024;
pub(crate) const SIGNATURE_DOMAIN: &[u8] = b"AutoKeyboardLayot.language-package.v1\0";
/// New producers use this floor; consumers retain support for API 1.
/// API 2 includes the expanded settings/error-message catalog contract.
pub const RUNTIME_API: u32 = 2;

pub const fn supports_runtime_api(api: u32) -> bool {
    api >= 1 && api <= RUNTIME_API
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageError {
    InvalidData,
    TooLarge,
    UnsupportedFormat,
    UntrustedSigner,
    InvalidSignature,
    IntegrityMismatch,
    Incompatible,
    InvalidComponents,
}
impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "language_package_{self:?}")
    }
}
impl std::error::Error for PackageError {}

/// Most keys the release metadata may list, and the most a trust can hold.
const MAX_TRUSTED_KEYS: usize = 8;
const MAX_RELEASE_METADATA_BYTES: usize = 4096;

/// What a key in the release metadata is for. Both roles verify signatures; the
/// release tools sign with the release key only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyRole {
    /// The key the release pipeline signs with.
    Release,
    /// An offline key, kept apart from the release machine, that can sign a
    /// replacement catalog or package if the release key is lost.
    Recovery,
}

struct MetadataKey {
    signer: String,
    public: [u8; 32],
    role: KeyRole,
}

/// The key the release tools sign with, as recorded in the embedded trust
/// metadata. A recovery key is trusted for verification but never chosen here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSigner {
    pub signer: String,
    pub public_key: [u8; 32],
    /// The key in lowercase hex, which also names it in the key-protection entropy.
    pub public_key_hex: String,
}

/// Parses the release metadata. Format 1 lists one key and format 2 lists one to
/// eight, each with a role; exactly one key has the role "release". Unknown
/// fields, unknown formats, a wrong repository or algorithm, and a fingerprint
/// that does not match its key are all refused.
fn parse_release_metadata(bytes: &[u8]) -> Result<Vec<MetadataKey>, PackageError> {
    #[derive(Deserialize)]
    struct Header {
        format: u32,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Single {
        format: u32,
        algorithm: String,
        signer: String,
        public_key_hex: String,
        fingerprint_sha256: String,
        repository: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Listed {
        format: u32,
        algorithm: String,
        repository: String,
        keys: Vec<Listing>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Listing {
        signer: String,
        public_key_hex: String,
        fingerprint_sha256: String,
        role: String,
    }
    if bytes.len() > MAX_RELEASE_METADATA_BYTES {
        return Err(PackageError::TooLarge);
    }
    let header: Header = serde_json::from_slice(bytes).map_err(|_| PackageError::InvalidData)?;
    let (algorithm, repository, listings) = match header.format {
        1 => {
            let single: Single =
                serde_json::from_slice(bytes).map_err(|_| PackageError::InvalidData)?;
            if single.format != 1 {
                return Err(PackageError::InvalidData);
            }
            let only = Listing {
                signer: single.signer,
                public_key_hex: single.public_key_hex,
                fingerprint_sha256: single.fingerprint_sha256,
                role: "release".to_owned(),
            };
            (single.algorithm, single.repository, vec![only])
        }
        2 => {
            let listed: Listed =
                serde_json::from_slice(bytes).map_err(|_| PackageError::InvalidData)?;
            if listed.format != 2 {
                return Err(PackageError::InvalidData);
            }
            (listed.algorithm, listed.repository, listed.keys)
        }
        _ => return Err(PackageError::UnsupportedFormat),
    };
    let repository = crate::package_catalog::repository_id(&repository)
        .map_err(|_| PackageError::InvalidData)?;
    let expected_repository =
        crate::package_catalog::repository_id(crate::package_catalog::DEFAULT_PACKAGE_REPOSITORY)
            .map_err(|_| PackageError::InvalidData)?;
    if algorithm != "Ed25519" || repository != expected_repository || listings.is_empty() {
        return Err(PackageError::InvalidData);
    }
    if listings.len() > MAX_TRUSTED_KEYS {
        return Err(PackageError::TooLarge);
    }
    let mut keys = Vec::with_capacity(listings.len());
    for listing in listings {
        let public = decode_hex::<32>(&listing.public_key_hex)?;
        if <[u8; 32]>::from(Sha256::digest(public))
            != decode_hex::<32>(&listing.fingerprint_sha256)?
        {
            return Err(PackageError::IntegrityMismatch);
        }
        let role = match listing.role.as_str() {
            "release" => KeyRole::Release,
            "recovery" => KeyRole::Recovery,
            _ => return Err(PackageError::InvalidData),
        };
        keys.push(MetadataKey {
            signer: listing.signer,
            public,
            role,
        });
    }
    if keys
        .iter()
        .filter(|key| key.role == KeyRole::Release)
        .count()
        != 1
    {
        return Err(PackageError::InvalidData);
    }
    Ok(keys)
}

/// Trust anchors must be supplied by the application/release policy, NEVER by
/// the package being inspected. The empty default accepts no external packages.
#[derive(Debug, Clone, Default)]
pub struct PackageTrust(BTreeMap<String, VerifyingKey>);
impl PackageTrust {
    /// Deliberately provisioned application policy, embedded at build time.
    /// Never reads a key supplied by a downloaded package or a runtime file.
    pub fn release() -> Result<Self, PackageError> {
        Self::from_release_metadata(include_bytes!("../data/package-signing/public-key.json"))
    }

    /// The key the release tools sign with, from the same embedded metadata
    /// and under the same checks as `release()`.
    pub fn release_signer() -> Result<ReleaseSigner, PackageError> {
        Self::release_signer_from(include_bytes!("../data/package-signing/public-key.json"))
    }

    fn from_release_metadata(bytes: &[u8]) -> Result<Self, PackageError> {
        let keys = parse_release_metadata(bytes)?;
        Self::from_public_keys(keys.into_iter().map(|key| (key.signer, key.public)))
    }

    fn release_signer_from(bytes: &[u8]) -> Result<ReleaseSigner, PackageError> {
        let keys = parse_release_metadata(bytes)?;
        Self::from_public_keys(keys.iter().map(|key| (key.signer.clone(), key.public)))?;
        let release = keys
            .into_iter()
            .find(|key| key.role == KeyRole::Release)
            .ok_or(PackageError::InvalidData)?;
        Ok(ReleaseSigner {
            public_key_hex: release.public.iter().map(|b| format!("{b:02x}")).collect(),
            signer: release.signer,
            public_key: release.public,
        })
    }

    pub(crate) fn verify_document(
        &self,
        domain: &[u8],
        signer: &str,
        document: &str,
        signature: &str,
        maximum_bytes: usize,
    ) -> Result<(), PackageError> {
        if document.len() > maximum_bytes || signer.len() > 32 || signature.len() > 128 {
            return Err(PackageError::TooLarge);
        }
        let key = self.0.get(signer).ok_or(PackageError::UntrustedSigner)?;
        let signature = Signature::from_bytes(&decode_hex::<64>(signature)?);
        let mut signed = Vec::with_capacity(domain.len() + document.len());
        signed.extend_from_slice(domain);
        signed.extend_from_slice(document.as_bytes());
        key.verify_strict(&signed, &signature)
            .map_err(|_| PackageError::InvalidSignature)
    }

    pub fn from_public_keys(
        keys: impl IntoIterator<Item = (String, [u8; 32])>,
    ) -> Result<Self, PackageError> {
        let mut trusted = BTreeMap::new();
        let mut seen_keys = std::collections::BTreeSet::new();
        for (index, (id, bytes)) in keys.into_iter().enumerate() {
            if index >= MAX_TRUSTED_KEYS {
                return Err(PackageError::TooLarge);
            }
            if id.is_empty()
                || id.len() > 32
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err(PackageError::InvalidData);
            }
            let key = VerifyingKey::from_bytes(&bytes).map_err(|_| PackageError::InvalidData)?;
            // The same key under two names would make one signer count twice.
            if key.is_weak() || !seen_keys.insert(bytes) || trusted.insert(id, key).is_some() {
                return Err(PackageError::InvalidData);
            }
        }
        Ok(Self(trusted))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Roles<T> {
    words: Option<T>,
    short_words: Option<T>,
    scoring: Option<T>,
    input: Option<T>,
    ui: Option<T>,
    license: T,
    notice: T,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Integrity {
    bytes: usize,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    package_id: String,
    revision: u64,
    runtime_api: u32,
    input_pack: Option<String>,
    ui_locale: Option<String>,
    components: Roles<Integrity>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: u32,
    signer: String,
    manifest: String,
    signature: String,
    components: Roles<String>,
}

/// Successful authentication and data validation, not installation, licensing
/// approval, OS readiness, or permission to roll back an installed revision.
#[derive(Debug)]
pub struct VerifiedLanguagePackage {
    id: PackId,
    revision: u64,
    runtime_api: u32,
    envelope_sha256: [u8; 32],
    envelope_bytes: u64,
    input: Option<Arc<DictionaryPack>>,
    ui_locale: Option<String>,
    ui: Option<Arc<str>>,
    license: Arc<str>,
    notice: Arc<str>,
}

impl VerifiedLanguagePackage {
    pub fn verify(bytes: &[u8], trust: &PackageTrust) -> Result<Self, PackageError> {
        if bytes.len() > MAX_PACKAGE_BYTES {
            return Err(PackageError::TooLarge);
        }
        let envelope: Envelope =
            serde_json::from_slice(bytes).map_err(|_| PackageError::InvalidData)?;
        if envelope.format != 1 {
            return Err(PackageError::UnsupportedFormat);
        }
        trust.verify_document(
            SIGNATURE_DOMAIN,
            &envelope.signer,
            &envelope.manifest,
            &envelope.signature,
            MAX_MANIFEST_BYTES,
        )?;
        let manifest: Manifest =
            serde_json::from_str(&envelope.manifest).map_err(|_| PackageError::InvalidData)?;
        if manifest.format != 1 {
            return Err(PackageError::UnsupportedFormat);
        }
        if !supports_runtime_api(manifest.runtime_api) || manifest.revision == 0 {
            return Err(PackageError::Incompatible);
        }
        let id = PackId::parse(&manifest.package_id).map_err(|_| PackageError::InvalidData)?;
        let expected = &manifest.components;
        let contents = &envelope.components;
        for (data, integrity, limit) in [
            (
                &contents.words,
                &expected.words,
                crate::dictionary_registry::MAX_DICTIONARY_BYTES,
            ),
            (
                &contents.short_words,
                &expected.short_words,
                crate::dictionary_registry::MAX_SHORT_DICTIONARY_BYTES,
            ),
            (&contents.scoring, &expected.scoring, 64 * 1024),
            (&contents.input, &expected.input, 16 * 1024),
            (
                &contents.ui,
                &expected.ui,
                crate::localization::MAX_CATALOG_BYTES,
            ),
        ] {
            match (data, integrity) {
                (Some(data), Some(integrity)) => verify_content(data, integrity, limit)?,
                (None, None) => (),
                _ => return Err(PackageError::IntegrityMismatch),
            }
        }
        verify_content(&contents.license, &expected.license, 512 * 1024)?;
        verify_content(&contents.notice, &expected.notice, 512 * 1024)?;
        for text in [&contents.license, &contents.notice] {
            if text.trim().is_empty()
                || text
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(PackageError::InvalidComponents);
            }
        }
        let input = match (
            &manifest.input_pack,
            &contents.words,
            &contents.short_words,
            &contents.scoring,
            &contents.input,
        ) {
            (Some(raw_id), Some(words), Some(short), Some(scoring), Some(input)) => {
                let input_id =
                    PackId::parse(raw_id).map_err(|_| PackageError::InvalidComponents)?;
                if input_id != id {
                    return Err(PackageError::InvalidComponents);
                }
                let model = ScoringModel::from_json(scoring.as_bytes())
                    .map_err(|_| PackageError::InvalidComponents)?;
                let descriptor = InputPackDescriptor::from_json(input.as_bytes())
                    .map_err(|_| PackageError::InvalidComponents)?;
                let pack = DictionaryPack::from_words(input_id, words.lines(), short.lines())
                    .map_err(|_| PackageError::InvalidComponents)?
                    .with_scoring_model(model)
                    .with_input_descriptor(descriptor)
                    .map_err(|_| PackageError::InvalidComponents)?;
                Some(Arc::new(pack))
            }
            (None, None, None, None, None) => None,
            _ => return Err(PackageError::InvalidComponents),
        };
        let ui_locale = match (&manifest.ui_locale, &contents.ui) {
            (Some(locale), Some(ui)) => {
                let locale = crate::localization::normalize_locale(locale)
                    .map_err(|_| PackageError::InvalidComponents)?;
                let mut catalogs = crate::localization::CatalogRegistry::default();
                catalogs
                    .add(ui.as_bytes())
                    .map_err(|_| PackageError::InvalidComponents)?;
                // Canonical catalog identity must match; aliases are not identity.
                if !catalogs
                    .locales()
                    .into_iter()
                    .filter(|id| *id != "en")
                    .eq(std::iter::once(locale.as_str()))
                {
                    return Err(PackageError::InvalidComponents);
                }
                Some(locale)
            }
            (None, None) => None,
            _ => return Err(PackageError::InvalidComponents),
        };
        if input.is_none() && ui_locale.is_none() {
            return Err(PackageError::InvalidComponents);
        }
        Ok(Self {
            id,
            revision: manifest.revision,
            runtime_api: manifest.runtime_api,
            envelope_sha256: Sha256::digest(bytes).into(),
            envelope_bytes: bytes.len() as u64,
            input,
            ui_locale,
            ui: envelope.components.ui.map(Arc::from),
            license: Arc::from(envelope.components.license),
            notice: Arc::from(envelope.components.notice),
        })
    }

    pub const fn id(&self) -> PackId {
        self.id
    }
    pub const fn runtime_api(&self) -> u32 {
        self.runtime_api
    }
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub const fn envelope_sha256(&self) -> [u8; 32] {
        self.envelope_sha256
    }
    pub const fn envelope_bytes(&self) -> u64 {
        self.envelope_bytes
    }
    pub fn input(&self) -> Option<&Arc<DictionaryPack>> {
        self.input.as_ref()
    }
    pub fn ui_locale(&self) -> Option<&str> {
        self.ui_locale.as_deref()
    }
    pub fn ui(&self) -> Option<&str> {
        self.ui.as_deref()
    }
    pub fn license(&self) -> &str {
        &self.license
    }
    pub fn notice(&self) -> &str {
        &self.notice
    }
}

fn verify_content(data: &str, integrity: &Integrity, limit: usize) -> Result<(), PackageError> {
    if data.len() > limit || integrity.bytes > limit {
        return Err(PackageError::TooLarge);
    }
    let expected = decode_hex::<32>(&integrity.sha256)?;
    let actual: [u8; 32] = Sha256::digest(data.as_bytes()).into();
    if integrity.bytes != data.len() || actual != expected {
        return Err(PackageError::IntegrityMismatch);
    }
    Ok(())
}

pub(crate) fn decode_hex<const N: usize>(text: &str) -> Result<[u8; N], PackageError> {
    if text.len() != N * 2 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PackageError::InvalidData);
    }
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| PackageError::InvalidData)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_policy_provisions_only_the_approved_public_key() {
        let trust = super::PackageTrust::release().unwrap();
        assert_eq!(trust.0.len(), 1);
        assert!(trust.0.contains_key("pkg-20260912-01"));
        assert!(!trust.0.contains_key("test-only"));
        assert!(super::PackageTrust::default().0.is_empty());
        assert_eq!(
            trust.verify_document(b"test", "test-only", "{}", &"00".repeat(64), 128),
            Err(super::PackageError::UntrustedSigner)
        );
        assert_eq!(
            trust.verify_document(b"test", "pkg-20260912-01", "{}", &"00".repeat(64), 128),
            Err(super::PackageError::InvalidSignature)
        );
    }

    #[test]
    fn release_policy_rejects_corrupt_or_wrong_scope_metadata() {
        let original: serde_json::Value =
            serde_json::from_slice(include_bytes!("../data/package-signing/public-key.json"))
                .unwrap();
        for (field, value) in [
            ("repository", serde_json::json!("other/repository")),
            ("algorithm", serde_json::json!("RSA")),
            ("format", serde_json::json!(2)),
            ("signer", serde_json::json!("INVALID")),
            ("fingerprint_sha256", serde_json::json!("00".repeat(32))),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut metadata = original.clone();
            metadata[field] = value;
            assert!(
                super::PackageTrust::from_release_metadata(&serde_json::to_vec(&metadata).unwrap())
                    .is_err()
            );
        }
    }

    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};

    // Public deterministic test material only. Never a release signing key.
    fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[42; 32])
    }
    fn trust() -> PackageTrust {
        PackageTrust::from_public_keys([(
            "test-only".into(),
            test_key().verifying_key().to_bytes(),
        )])
        .unwrap()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    fn fixtures() -> (Value, Value) {
        let components = json!({
            "words":"hello\nworld\n", "short_words":"hi\n",
            "scoring": include_str!("../data/scoring/en-US.json"),
            "input": r#"{"format":1,"pack_id":"custom-test","windows_keyboard_profiles":[{"profile":"0409:00000409","required_capabilities":["physical-key-v1"]}]}"#,
            "ui": r#"{"format":1,"locale":"ru","direction":"ltr","messages":{"locale.self_name":"Русский","import.failed":"Legacy package failure message"}}"#,
            "license":"Test fixture license only; not a distribution license.\n",
            "notice":"Synthetic dictionary and catalog fixture.\n"
        });
        let mut manifest = json!({"format":1,"package_id":"custom-test","revision":1,"runtime_api":1,"input_pack":"custom-test","ui_locale":"ru","components":{}});
        for (name, content) in components.as_object().unwrap() {
            let content = content.as_str().unwrap();
            manifest["components"][name] =
                json!({"bytes":content.len(),"sha256":hex(&Sha256::digest(content.as_bytes()))});
        }
        (manifest, components)
    }
    fn envelope(manifest: &Value, components: &Value) -> Value {
        let manifest = serde_json::to_string(manifest).unwrap();
        let mut signed = SIGNATURE_DOMAIN.to_vec();
        signed.extend_from_slice(manifest.as_bytes());
        json!({"format":1,"signer":"test-only","manifest":manifest,"signature":hex(&test_key().sign(&signed).to_bytes()),"components":components})
    }
    fn verify(value: &Value) -> Result<VerifiedLanguagePackage, PackageError> {
        VerifiedLanguagePackage::verify(&serde_json::to_vec(value).unwrap(), &trust())
    }

    #[test]
    fn legacy_and_current_packages_load_but_future_consumers_are_required_explicitly() {
        let (mut manifest, components) = fixtures();
        for api in [1, RUNTIME_API] {
            manifest["runtime_api"] = json!(api);
            assert_eq!(
                verify(&envelope(&manifest, &components))
                    .unwrap()
                    .runtime_api(),
                api
            );
        }
        for api in [0, RUNTIME_API + 1] {
            manifest["runtime_api"] = json!(api);
            assert!(matches!(
                verify(&envelope(&manifest, &components)),
                Err(PackageError::Incompatible)
            ));
        }
    }
    #[test]
    #[ignore = "full real corpus; run explicitly in the bounded release validation job"]
    #[cfg(feature = "legacy-bundled-input")]
    fn full_russian_corpus_signed_package_round_trip() {
        use fst::Streamer;
        let original =
            fst::Set::new(&include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU.fst"))[..]).unwrap();
        let mut words = String::new();
        let mut stream = original.stream();
        while let Some(word) = stream.next() {
            words.push_str(std::str::from_utf8(word).unwrap());
            words.push('\n');
        }
        assert_eq!(original.len(), 1_436_545);
        assert_eq!(words.len(), 33_008_809);
        let components = json!({
            "words": words,
            "short_words": include_str!("../data/language-packs/ru-RU/common-short-words.txt"),
            "input": include_str!("../data/input/ru-RU.json"),
            "scoring": include_str!("../data/scoring/ru-RU.json"),
            "license": include_str!("../data/language-packs/ru-RU/LICENSE.words.txt"),
            "notice": "Local migration verification of the existing embedded Russian corpus; public synthetic test signer, NOT a release artifact."
        });
        let mut manifest = json!({"format":1,"package_id":"ru-RU","revision":1,"runtime_api":1,"input_pack":"ru-RU","components":{}});
        for (name, content) in components.as_object().unwrap() {
            let content = content.as_str().unwrap();
            manifest["components"][name] =
                json!({"bytes":content.len(),"sha256":hex(&Sha256::digest(content.as_bytes()))});
        }
        let raw = serde_json::to_vec(&envelope(&manifest, &components)).unwrap();
        // Drop generation temporaries to measure verifier rather than fixture
        // construction plus compilation. The raw signed envelope stays alive.
        drop(components);
        drop(words);
        assert!(raw.len() < MAX_PACKAGE_BYTES);
        let started = std::time::Instant::now();
        let package = VerifiedLanguagePackage::verify(&raw, &trust()).unwrap();
        let elapsed = started.elapsed();
        let dictionary = package.input().unwrap();
        let mut stream = original.stream();
        while let Some(word) = stream.next() {
            assert!(dictionary.contains(std::str::from_utf8(word).unwrap()));
        }
        assert!(dictionary.common_short_contains("не"));
        assert!(dictionary.input_descriptor().is_some());
        assert!(dictionary.scoring_model().is_some());
        assert_eq!(
            package.license(),
            include_str!("../data/language-packs/ru-RU/LICENSE.words.txt")
        );
        eprintln!(
            "full_ru_round_trip: words={} envelope_bytes={} verify_ms={}",
            original.len(),
            raw.len(),
            elapsed.as_millis()
        );
    }

    #[test]
    fn authenticated_components_are_validated_without_installing_or_enabling() {
        let (manifest, components) = fixtures();
        let envelope = envelope(&manifest, &components);
        let package = verify(&envelope).unwrap();
        assert_eq!(package.id().as_str(), "custom-test");
        assert_eq!(package.revision(), 1);
        assert_eq!(package.ui_locale(), Some("ru"));
        assert!(package.ui().unwrap().contains("Русский"));
        assert!(package.input().unwrap().contains("hello"));
        assert!(package.input().unwrap().common_short_contains("hi"));
        assert!(package.input().unwrap().scoring_model().is_some());
        assert!(package.input().unwrap().input_descriptor().is_some());
        assert!(package.license().contains("fixture"));
        assert!(package.notice().contains("Synthetic"));
        assert_eq!(
            package.envelope_sha256(),
            <[u8; 32]>::from(Sha256::digest(serde_json::to_vec(&envelope).unwrap()))
        );
        assert!(
            crate::DictionaryRegistry::embedded()
                .installed(&package.id())
                .is_none()
        );
    }
    #[test]
    fn signature_requires_trusted_key_exact_manifest_and_domain() {
        let (manifest, components) = fixtures();
        let good = envelope(&manifest, &components);
        assert_eq!(
            VerifiedLanguagePackage::verify(
                &serde_json::to_vec(&good).unwrap(),
                &PackageTrust::default()
            )
            .unwrap_err(),
            PackageError::UntrustedSigner
        );
        let mut changed = good.clone();
        changed["manifest"] = json!(format!("{} ", good["manifest"].as_str().unwrap()));
        assert_eq!(
            verify(&changed).unwrap_err(),
            PackageError::InvalidSignature
        );
        changed = good.clone();
        changed["signature"] = json!(hex(&test_key()
            .sign(good["manifest"].as_str().unwrap().as_bytes())
            .to_bytes()));
        assert_eq!(
            verify(&changed).unwrap_err(),
            PackageError::InvalidSignature
        );
        changed["signature"] = json!("00".repeat(64));
        assert_eq!(
            verify(&changed).unwrap_err(),
            PackageError::InvalidSignature
        );
        for signature in ["a".repeat(127), "g".repeat(128), "é".repeat(64)] {
            changed["signature"] = json!(signature);
            assert_eq!(verify(&changed).unwrap_err(), PackageError::InvalidData);
        }
        changed["signature"] = json!("a".repeat(129));
        assert_eq!(verify(&changed).unwrap_err(), PackageError::TooLarge);
        changed = good.clone();
        changed["signer"] = json!("a".repeat(33));
        assert_eq!(verify(&changed).unwrap_err(), PackageError::TooLarge);
    }
    #[test]
    fn raw_transport_digest_is_not_semantic_identity() {
        let (manifest, components) = fixtures();
        let envelope = envelope(&manifest, &components);
        let compact = serde_json::to_vec(&envelope).unwrap();
        let pretty = serde_json::to_vec_pretty(&envelope).unwrap();
        let first = VerifiedLanguagePackage::verify(&compact, &trust()).unwrap();
        let second = VerifiedLanguagePackage::verify(&pretty, &trust()).unwrap();
        assert_eq!(first.id(), second.id());
        assert_eq!(first.revision(), second.revision());
        assert_ne!(first.envelope_sha256(), second.envelope_sha256());
    }
    #[test]
    fn payload_bytes_and_presence_are_bound_to_signed_hashes_and_lengths() {
        let (manifest, components) = fixtures();
        let mut changed = envelope(&manifest, &components);
        changed["components"]["words"] = json!("other\nworld\n");
        assert_eq!(
            verify(&changed).unwrap_err(),
            PackageError::IntegrityMismatch
        );
        changed = envelope(&manifest, &components);
        changed["components"]
            .as_object_mut()
            .unwrap()
            .remove("short_words");
        assert_eq!(
            verify(&changed).unwrap_err(),
            PackageError::IntegrityMismatch
        );
        let mut changed_manifest = manifest.clone();
        changed_manifest["components"]["words"]["bytes"] = json!(1);
        assert_eq!(
            verify(&envelope(&changed_manifest, &components)).unwrap_err(),
            PackageError::IntegrityMismatch
        );
        changed_manifest["components"]["words"]["bytes"] =
            json!(crate::dictionary_registry::MAX_DICTIONARY_BYTES + 1);
        assert_eq!(
            verify(&envelope(&changed_manifest, &components)).unwrap_err(),
            PackageError::TooLarge
        );
    }
    #[test]
    fn signed_metadata_cannot_invent_compatibility_or_mix_partial_components() {
        let (manifest, components) = fixtures();
        for (field, value) in [("runtime_api", json!(3)), ("revision", json!(0))] {
            let mut changed = manifest.clone();
            changed[field] = value;
            assert_eq!(
                verify(&envelope(&changed, &components)).unwrap_err(),
                PackageError::Incompatible
            );
        }
        let mut changed = manifest.clone();
        changed["input_pack"] = json!("other-pack");
        assert_eq!(
            verify(&envelope(&changed, &components)).unwrap_err(),
            PackageError::InvalidComponents
        );
        changed = manifest.clone();
        changed["package_id"] = json!("other-pack");
        assert_eq!(
            verify(&envelope(&changed, &components)).unwrap_err(),
            PackageError::InvalidComponents
        );
        for locale in ["en", "et", "ru-RU"] {
            changed = manifest.clone();
            changed["ui_locale"] = json!(locale);
            assert_eq!(
                verify(&envelope(&changed, &components)).unwrap_err(),
                PackageError::InvalidComponents
            );
        }
        changed = manifest.clone();
        changed.as_object_mut().unwrap().remove("input_pack");
        assert_eq!(
            verify(&envelope(&changed, &components)).unwrap_err(),
            PackageError::InvalidComponents
        );
    }
    #[test]
    fn fixed_role_envelope_rejects_extra_paths_executables_and_duplicate_fields() {
        let (manifest, components) = fixtures();
        for field in ["../words", "C:\\evil.dll", "script", "words.txt"] {
            let mut changed = envelope(&manifest, &components);
            changed["components"][field] = json!("ignored?");
            assert_eq!(verify(&changed).unwrap_err(), PackageError::InvalidData);
        }
        let good = envelope(&manifest, &components);
        let text = serde_json::to_string(&good).unwrap();
        let duplicate = text.replacen("\"format\":1", "\"format\":1,\"format\":1", 1);
        assert_eq!(
            VerifiedLanguagePackage::verify(duplicate.as_bytes(), &trust()).unwrap_err(),
            PackageError::InvalidData
        );
        assert_eq!(
            VerifiedLanguagePackage::verify(&vec![b' '; MAX_PACKAGE_BYTES + 1], &trust())
                .unwrap_err(),
            PackageError::TooLarge
        );
    }
    #[test]
    fn ui_and_input_components_are_independent_but_not_partial() {
        let (manifest, components) = fixtures();
        let mut ui_manifest = manifest.clone();
        let mut ui_contents = components.clone();
        ui_manifest.as_object_mut().unwrap().remove("input_pack");
        for role in ["words", "short_words", "scoring", "input"] {
            ui_manifest["components"]
                .as_object_mut()
                .unwrap()
                .remove(role);
            ui_contents.as_object_mut().unwrap().remove(role);
        }
        let ui = verify(&envelope(&ui_manifest, &ui_contents)).unwrap();
        assert!(ui.input().is_none());
        assert_eq!(ui.ui_locale(), Some("ru"));
        let mut input_manifest = manifest.clone();
        let mut input_contents = components.clone();
        input_manifest.as_object_mut().unwrap().remove("ui_locale");
        input_manifest["components"]
            .as_object_mut()
            .unwrap()
            .remove("ui");
        input_contents.as_object_mut().unwrap().remove("ui");
        let input = verify(&envelope(&input_manifest, &input_contents)).unwrap();
        assert!(input.input().is_some());
        assert!(input.ui().is_none());
        for role in ["short_words", "scoring", "input"] {
            let mut incomplete = manifest.clone();
            let mut contents = components.clone();
            incomplete["components"]
                .as_object_mut()
                .unwrap()
                .remove(role);
            contents.as_object_mut().unwrap().remove(role);
            assert_eq!(
                verify(&envelope(&incomplete, &contents)).unwrap_err(),
                PackageError::InvalidComponents
            );
        }
        for role in ["license", "notice"] {
            let mut missing = components.clone();
            missing.as_object_mut().unwrap().remove(role);
            assert_eq!(
                verify(&envelope(&manifest, &missing)).unwrap_err(),
                PackageError::InvalidData
            );
            missing[role] = json!("");
            let mut blank = manifest.clone();
            blank["components"][role] = json!({"bytes":0,"sha256":hex(&Sha256::digest(b""))});
            assert_eq!(
                verify(&envelope(&blank, &missing)).unwrap_err(),
                PackageError::InvalidComponents
            );
        }
    }
    #[test]
    fn trust_registry_rejects_weak_duplicate_and_unbounded_anchors() {
        let public = test_key().verifying_key().to_bytes();
        let mut weak = [0; 32];
        weak[0] = 1;
        assert!(PackageTrust::from_public_keys([("weak".into(), weak)]).is_err());
        assert!(
            PackageTrust::from_public_keys([("test".into(), public), ("test".into(), public)])
                .is_err()
        );
        // The same key under two names would count one signer twice.
        assert_eq!(
            PackageTrust::from_public_keys([("one".into(), public), ("two".into(), public)])
                .unwrap_err(),
            PackageError::InvalidData
        );
        assert!(PackageTrust::from_public_keys([("../key".into(), public)]).is_err());
        let distinct = |count: u8| {
            (0..count).map(|i| {
                (
                    format!("key-{i}"),
                    SigningKey::from_bytes(&[i + 1; 32])
                        .verifying_key()
                        .to_bytes(),
                )
            })
        };
        assert_eq!(
            PackageTrust::from_public_keys(distinct(8)).unwrap().0.len(),
            8
        );
        assert_eq!(
            PackageTrust::from_public_keys(distinct(9)).unwrap_err(),
            PackageError::TooLarge
        );
    }

    // The trust metadata as it is published today (format 1).
    const FORMAT_1: &str = r#"{"format":1,"algorithm":"Ed25519","signer":"pkg-20260912-01","public_key_hex":"ed32cc8fc0341647aab1b7b2b691056f831b512529f4344497cacc4e05aca77d","fingerprint_sha256":"5700a261650fc5edd9dc18b5e3e3dd3ecea3aae6614f997b43679ff0f259ca11","repository":"woffko/AutoKeyboardLayot"}"#;

    // Public deterministic seeds: these keys sign nothing real.
    fn listed(seed: u8, signer: &str, role: &str) -> Value {
        listed_public(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes(),
            signer,
            role,
        )
    }
    fn listed_public(public: [u8; 32], signer: &str, role: &str) -> Value {
        json!({
            "signer": signer,
            "public_key_hex": hex(&public),
            "fingerprint_sha256": hex(&Sha256::digest(public)),
            "role": role,
        })
    }
    fn format_2(keys: Vec<Value>) -> Value {
        json!({
            "format": 2,
            "algorithm": "Ed25519",
            "repository": crate::package_catalog::DEFAULT_PACKAGE_REPOSITORY,
            "keys": keys,
        })
    }
    fn trust_from(metadata: &Value) -> Result<PackageTrust, PackageError> {
        PackageTrust::from_release_metadata(&serde_json::to_vec(metadata).unwrap())
    }
    fn signer_from(metadata: &Value) -> Result<ReleaseSigner, PackageError> {
        PackageTrust::release_signer_from(&serde_json::to_vec(metadata).unwrap())
    }
    fn signature_by(seed: u8, domain: &[u8], document: &str) -> String {
        let mut signed = domain.to_vec();
        signed.extend_from_slice(document.as_bytes());
        hex(&SigningKey::from_bytes(&[seed; 32]).sign(&signed).to_bytes())
    }

    #[test]
    fn release_metadata_format_1_is_still_accepted() {
        let trust = PackageTrust::from_release_metadata(FORMAT_1.as_bytes()).unwrap();
        assert_eq!(
            trust.0.keys().map(String::as_str).collect::<Vec<_>>(),
            ["pkg-20260912-01"]
        );
        let signer = PackageTrust::release_signer_from(FORMAT_1.as_bytes()).unwrap();
        assert_eq!(signer.signer, "pkg-20260912-01");
        assert_eq!(
            signer.public_key_hex,
            "ed32cc8fc0341647aab1b7b2b691056f831b512529f4344497cacc4e05aca77d"
        );
        assert_eq!(hex(&signer.public_key), signer.public_key_hex);
        // The embedded file works through the public entry points.
        let embedded = PackageTrust::release_signer().unwrap();
        assert!(
            PackageTrust::release()
                .unwrap()
                .0
                .contains_key(&embedded.signer)
        );
    }

    #[test]
    fn release_metadata_format_2_trusts_every_listed_key_and_signs_with_the_release_key() {
        let metadata = format_2(vec![
            listed(1, "release-one", "release"),
            listed(2, "recovery-one", "recovery"),
        ]);
        let trust = trust_from(&metadata).unwrap();
        assert_eq!(trust.0.len(), 2);
        // Both keys verify their own signatures and nobody else's.
        for (seed, signer) in [(1, "release-one"), (2, "recovery-one")] {
            assert_eq!(
                trust.verify_document(
                    b"domain",
                    signer,
                    "{}",
                    &signature_by(seed, b"domain", "{}"),
                    128
                ),
                Ok(())
            );
        }
        assert_eq!(
            trust.verify_document(
                b"domain",
                "release-one",
                "{}",
                &signature_by(2, b"domain", "{}"),
                128
            ),
            Err(PackageError::InvalidSignature)
        );
        // Only the release key is offered for signing, wherever it is listed.
        let signer = signer_from(&metadata).unwrap();
        assert_eq!(signer.signer, "release-one");
        assert_eq!(
            signer.public_key,
            SigningKey::from_bytes(&[1; 32]).verifying_key().to_bytes()
        );
        assert_eq!(hex(&signer.public_key), signer.public_key_hex);
        let reversed = format_2(vec![
            listed(2, "recovery-one", "recovery"),
            listed(1, "release-one", "release"),
        ]);
        assert_eq!(signer_from(&reversed).unwrap(), signer);
        // Eight keys are the limit.
        let eight: Vec<Value> = std::iter::once(listed(1, "release-one", "release"))
            .chain((2..=8).map(|seed| listed(seed, &format!("recovery-{seed}"), "recovery")))
            .collect();
        assert_eq!(trust_from(&format_2(eight)).unwrap().0.len(), 8);
    }

    #[test]
    fn swapping_the_roles_changes_the_signing_key_and_not_who_is_trusted() {
        // The recovery procedure builds the signing tool from a local copy of the
        // metadata in which the recovery key is the release key; the shipped
        // applications already trust both keys, so nothing has to be republished.
        let shipped = format_2(vec![
            listed(1, "release-one", "release"),
            listed(2, "recovery-one", "recovery"),
        ]);
        let swapped = format_2(vec![
            listed(1, "release-one", "recovery"),
            listed(2, "recovery-one", "release"),
        ]);
        let trusted = |metadata: &Value| {
            trust_from(metadata)
                .unwrap()
                .0
                .into_iter()
                .map(|(name, key)| (name, key.to_bytes()))
                .collect::<Vec<_>>()
        };
        assert_eq!(trusted(&shipped), trusted(&swapped));
        assert_eq!(signer_from(&shipped).unwrap().signer, "release-one");
        assert_eq!(signer_from(&swapped).unwrap().signer, "recovery-one");
    }

    #[test]
    fn release_metadata_rejects_duplicates_bad_roles_and_wrong_counts() {
        let release = || listed(1, "release-one", "release");
        let mut weak = [0; 32];
        weak[0] = 1;
        let nine: Vec<Value> = (1..=9)
            .map(|seed| {
                let role = if seed == 1 { "release" } else { "recovery" };
                listed(seed, &format!("key-{seed}"), role)
            })
            .collect();
        let cases: Vec<(&str, Vec<Value>, PackageError)> = vec![
            (
                "the same signer twice",
                vec![release(), listed(2, "release-one", "recovery")],
                PackageError::InvalidData,
            ),
            (
                "the same key under two signers",
                vec![release(), listed(1, "recovery-one", "recovery")],
                PackageError::InvalidData,
            ),
            (
                "no release key",
                vec![listed(2, "recovery-one", "recovery")],
                PackageError::InvalidData,
            ),
            (
                "two release keys",
                vec![release(), listed(2, "release-two", "release")],
                PackageError::InvalidData,
            ),
            ("no keys", vec![], PackageError::InvalidData),
            ("nine keys", nine, PackageError::TooLarge),
            (
                "an unknown role",
                vec![release(), listed(2, "recovery-one", "backup")],
                PackageError::InvalidData,
            ),
            (
                "a role in capitals",
                vec![release(), listed(2, "recovery-one", "Recovery")],
                PackageError::InvalidData,
            ),
            (
                "a signer name in capitals",
                vec![release(), listed(2, "RECOVERY", "recovery")],
                PackageError::InvalidData,
            ),
            (
                "an empty signer name",
                vec![release(), listed(2, "", "recovery")],
                PackageError::InvalidData,
            ),
            (
                "a weak key",
                vec![release(), listed_public(weak, "recovery-one", "recovery")],
                PackageError::InvalidData,
            ),
        ];
        for (name, keys, expected) in cases {
            let metadata = format_2(keys);
            assert_eq!(trust_from(&metadata).unwrap_err(), expected, "{name}");
            assert_eq!(signer_from(&metadata).unwrap_err(), expected, "{name}");
        }
    }

    #[test]
    fn release_metadata_rejects_unknown_fields_formats_and_wrong_scope() {
        let base = format_2(vec![
            listed(1, "release-one", "release"),
            listed(2, "recovery-one", "recovery"),
        ]);
        assert!(trust_from(&base).is_ok());
        let single: Value = serde_json::from_str(FORMAT_1).unwrap();
        let changed = |edit: &dyn Fn(&mut Value)| {
            let mut metadata = base.clone();
            edit(&mut metadata);
            metadata
        };
        // Unknown fields, at the top, in a key, and format 1 fields in a format 2 file.
        for (name, metadata) in [
            (
                "an extra top-level field",
                changed(&|m| m["note"] = json!(true)),
            ),
            (
                "an extra field in a key",
                changed(&|m| m["keys"][1]["expires"] = json!("2030-01-01")),
            ),
            (
                "format 1 fields in a format 2 file",
                changed(&|m| m["signer"] = json!("release-one")),
            ),
            (
                "a missing key list",
                changed(&|m| {
                    m.as_object_mut().unwrap().remove("keys");
                }),
            ),
            (
                "a missing role",
                changed(&|m| {
                    m["keys"][1].as_object_mut().unwrap().remove("role");
                }),
            ),
        ] {
            assert_eq!(
                trust_from(&metadata).unwrap_err(),
                PackageError::InvalidData,
                "{name}"
            );
        }
        let mut with_keys = single.clone();
        with_keys["keys"] = base["keys"].clone();
        assert_eq!(
            trust_from(&with_keys).unwrap_err(),
            PackageError::InvalidData,
            "a key list in a format 1 file"
        );
        let mut with_role = single;
        with_role["role"] = json!("release");
        assert_eq!(
            trust_from(&with_role).unwrap_err(),
            PackageError::InvalidData,
            "a role in a format 1 file"
        );
        // Unknown or missing formats.
        for format in [0, 3, 99] {
            let metadata = changed(&|m| m["format"] = json!(format));
            assert_eq!(
                trust_from(&metadata).unwrap_err(),
                PackageError::UnsupportedFormat,
                "format {format}"
            );
        }
        let metadata = changed(&|m| {
            m.as_object_mut().unwrap().remove("format");
        });
        assert_eq!(
            trust_from(&metadata).unwrap_err(),
            PackageError::InvalidData
        );
        // Wrong scope.
        for (name, metadata) in [
            (
                "another repository",
                changed(&|m| m["repository"] = json!("other/repository")),
            ),
            (
                "another algorithm",
                changed(&|m| m["algorithm"] = json!("RSA")),
            ),
        ] {
            assert_eq!(
                trust_from(&metadata).unwrap_err(),
                PackageError::InvalidData,
                "{name}"
            );
        }
        // A fingerprint that belongs to another key, or to none.
        let swapped = changed(&|m| {
            let first = m["keys"][0]["fingerprint_sha256"].clone();
            m["keys"][1]["fingerprint_sha256"] = first;
        });
        assert_eq!(
            trust_from(&swapped).unwrap_err(),
            PackageError::IntegrityMismatch
        );
        let zeros = changed(&|m| m["keys"][1]["fingerprint_sha256"] = json!("00".repeat(32)));
        assert_eq!(
            trust_from(&zeros).unwrap_err(),
            PackageError::IntegrityMismatch
        );
        // Malformed hex, an oversized document and text that is not JSON.
        let short = changed(&|m| m["keys"][1]["public_key_hex"] = json!("abcd"));
        assert_eq!(trust_from(&short).unwrap_err(), PackageError::InvalidData);
        let padded = format!(
            "{}{}",
            serde_json::to_string(&base).unwrap(),
            " ".repeat(5000)
        );
        assert_eq!(
            PackageTrust::from_release_metadata(padded.as_bytes()).unwrap_err(),
            PackageError::TooLarge
        );
        for bytes in [&b""[..], b"not json", b"{", b"[]", b"null"] {
            assert!(PackageTrust::from_release_metadata(bytes).is_err());
        }
    }

    #[test]
    fn release_metadata_rejects_text_that_is_not_exactly_one_clean_document() {
        let base = serde_json::to_string(&format_2(vec![
            listed(1, "release-one", "release"),
            listed(2, "recovery-one", "recovery"),
        ]))
        .unwrap();
        let parse = |text: &str| PackageTrust::from_release_metadata(text.as_bytes());
        assert!(parse(&base).is_ok());
        // Anything after the document, a second document, or a byte-order mark.
        for text in [
            format!("{base} garbage"),
            format!("{base}{base}"),
            format!("{base}\n{{}}"),
            format!("\u{feff}{base}"),
        ] {
            assert!(parse(&text).is_err(), "{text:.60}");
        }
        assert!(
            parse(&format!("  \n{base}\n\n")).is_ok(),
            "whitespace around the document is fine"
        );
        // A field written twice is refused, never resolved in favour of one copy.
        let release = serde_json::to_string(&listed(1, "release-one", "release")).unwrap();
        let recovery = serde_json::to_string(&listed(2, "recovery-one", "recovery")).unwrap();
        let repository = crate::package_catalog::DEFAULT_PACKAGE_REPOSITORY;
        for (name, text) in [
            (
                "format twice",
                format!(
                    r#"{{"format":2,"format":2,"algorithm":"Ed25519","repository":"{repository}","keys":[{release}]}}"#
                ),
            ),
            (
                "format twice with different values",
                format!(
                    r#"{{"format":1,"format":2,"algorithm":"Ed25519","repository":"{repository}","keys":[{release}]}}"#
                ),
            ),
            (
                "keys twice",
                format!(
                    r#"{{"format":2,"algorithm":"Ed25519","repository":"{repository}","keys":[{release}],"keys":[{release},{recovery}]}}"#
                ),
            ),
            (
                "role twice in a key",
                format!(
                    r#"{{"format":2,"algorithm":"Ed25519","repository":"{repository}","keys":[{}]}}"#,
                    release.replace(
                        r#""role":"release""#,
                        r#""role":"recovery","role":"release""#
                    )
                ),
            ),
        ] {
            assert_eq!(
                parse(&text).unwrap_err(),
                PackageError::InvalidData,
                "{name}"
            );
        }
        // The format number must be a plain unsigned integer.
        for number in [
            "2.0",
            "\"2\"",
            "-1",
            "4294967296",
            "true",
            "null",
            "[2]",
            "1e0",
        ] {
            let text = base.replacen("\"format\":2", &format!("\"format\":{number}"), 1);
            assert_ne!(text, base, "the replacement must apply");
            assert_eq!(
                parse(&text).unwrap_err(),
                PackageError::InvalidData,
                "format {number}"
            );
        }
    }

    #[test]
    fn hex_may_be_written_in_either_case_but_the_signer_key_is_always_lowercase() {
        // The key-protection entropy of the signing tool contains this text, and the
        // key tool records lowercase. The published file is lowercase, so nothing
        // changes for it; an uppercase file is read as the same key, not a new one.
        let lower = listed(1, "release-one", "release");
        let mut upper = lower.clone();
        upper["public_key_hex"] = json!(lower["public_key_hex"].as_str().unwrap().to_uppercase());
        upper["fingerprint_sha256"] =
            json!(lower["fingerprint_sha256"].as_str().unwrap().to_uppercase());
        assert_ne!(lower, upper);
        let (lowercase, uppercase) = (format_2(vec![lower.clone()]), format_2(vec![upper.clone()]));
        assert_eq!(
            signer_from(&lowercase).unwrap(),
            signer_from(&uppercase).unwrap()
        );
        assert_eq!(
            signer_from(&uppercase).unwrap().public_key_hex,
            lower["public_key_hex"].as_str().unwrap()
        );
        assert_eq!(
            trust_from(&lowercase).unwrap().0.keys().collect::<Vec<_>>(),
            trust_from(&uppercase).unwrap().0.keys().collect::<Vec<_>>()
        );
        // Case does not hide a duplicate: the same key under two names is refused.
        let twin = {
            let mut twin = upper.clone();
            twin["signer"] = json!("recovery-one");
            twin["role"] = json!("recovery");
            twin
        };
        assert_eq!(
            trust_from(&format_2(vec![lower, twin])).unwrap_err(),
            PackageError::InvalidData
        );
        // The published key keeps its exact entropy text.
        assert_eq!(
            PackageTrust::release_signer_from(FORMAT_1.as_bytes())
                .unwrap()
                .public_key_hex,
            serde_json::from_str::<Value>(FORMAT_1).unwrap()["public_key_hex"]
                .as_str()
                .unwrap()
        );
    }
}
