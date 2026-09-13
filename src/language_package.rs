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
const RUNTIME_API: u32 = 1;

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

    fn from_release_metadata(bytes: &[u8]) -> Result<Self, PackageError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Metadata {
            format: u32,
            algorithm: String,
            signer: String,
            public_key_hex: String,
            fingerprint_sha256: String,
            repository: String,
        }
        if bytes.len() > 4096 {
            return Err(PackageError::TooLarge);
        }
        let metadata: Metadata =
            serde_json::from_slice(bytes).map_err(|_| PackageError::InvalidData)?;
        let repository = crate::package_catalog::repository_id(&metadata.repository)
            .map_err(|_| PackageError::InvalidData)?;
        let expected_repository = crate::package_catalog::repository_id(
            crate::package_catalog::DEFAULT_PACKAGE_REPOSITORY,
        )
        .map_err(|_| PackageError::InvalidData)?;
        if metadata.format != 1
            || metadata.algorithm != "Ed25519"
            || repository != expected_repository
        {
            return Err(PackageError::InvalidData);
        }
        let public = decode_hex::<32>(&metadata.public_key_hex)?;
        if <[u8; 32]>::from(Sha256::digest(public))
            != decode_hex::<32>(&metadata.fingerprint_sha256)?
        {
            return Err(PackageError::IntegrityMismatch);
        }
        Self::from_public_keys([(metadata.signer, public)])
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
        for (index, (id, bytes)) in keys.into_iter().enumerate() {
            if index >= 8 {
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
            if key.is_weak() || trusted.insert(id, key).is_some() {
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
        if manifest.runtime_api != RUNTIME_API || manifest.revision == 0 {
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
            "ui": r#"{"format":1,"locale":"ru","direction":"ltr","messages":{"locale.self_name":"Русский"}}"#,
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
        for (field, value) in [("runtime_api", json!(2)), ("revision", json!(0))] {
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
        assert!(PackageTrust::from_public_keys([("../key".into(), public)]).is_err());
        assert!(
            PackageTrust::from_public_keys((0..9).map(|i| (format!("key-{i}"), public))).is_err()
        );
    }
}
