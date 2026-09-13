//! Developer-only input validation before accessing the release private key.
//! No key storage, filesystem access, network requests, or publication here.
use crate::{
    language_package::{MAX_PACKAGE_BYTES, PackageError, PackageTrust, VerifiedLanguagePackage},
    package_catalog::{DEFAULT_PACKAGE_REPOSITORY, ReleaseError, VerifiedReleaseCatalog},
};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
pub enum SigningKind {
    Package,
    Catalog,
}

#[derive(Debug)]
pub enum SigningInputError {
    InvalidInput,
    TooLarge,
    Package(PackageError),
    Catalog(ReleaseError),
}
impl std::fmt::Display for SigningInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "signing_input_{self:?}")
    }
}
impl std::error::Error for SigningInputError {}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Components {
    words: Option<String>,
    short_words: Option<String>,
    scoring: Option<String>,
    input: Option<String>,
    ui: Option<String>,
    license: String,
    notice: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsignedPackage {
    format: u32,
    manifest: String,
    components: Components,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsignedCatalog {
    format: u32,
    catalog: String,
}

pub struct PreparedSigningInput {
    kind: SigningKind,
    envelope: Value,
    document: String,
    signer: String,
}
impl PreparedSigningInput {
    /// Accepts only unsigned envelopes. Unknown/duplicate fields and already
    /// signed input are refused. Existing runtime validators check all content.
    pub fn prepare(
        kind: SigningKind,
        signer: &str,
        bytes: &[u8],
        now: u64,
    ) -> Result<Self, SigningInputError> {
        let maximum = match kind {
            SigningKind::Package => MAX_PACKAGE_BYTES,
            SigningKind::Catalog => crate::package_catalog::MAX_CATALOG_BYTES,
        };
        if bytes.len() > maximum {
            return Err(SigningInputError::TooLarge);
        }
        let (document, envelope) = match kind {
            SigningKind::Package => {
                let input: UnsignedPackage =
                    serde_json::from_slice(bytes).map_err(|_| SigningInputError::InvalidInput)?;
                let envelope = json!({"format":input.format,"manifest":input.manifest,"components":input.components});
                (input.manifest, envelope)
            }
            SigningKind::Catalog => {
                let input: UnsignedCatalog =
                    serde_json::from_slice(bytes).map_err(|_| SigningInputError::InvalidInput)?;
                let envelope = json!({"format":input.format,"catalog":input.catalog});
                (input.catalog, envelope)
            }
        };
        let document_limit = match kind {
            SigningKind::Package => crate::language_package::MAX_MANIFEST_BYTES,
            SigningKind::Catalog => crate::package_catalog::MAX_DOCUMENT_BYTES,
        };
        if document.len() > document_limit {
            return Err(SigningInputError::TooLarge);
        }
        let prepared = Self {
            kind,
            envelope,
            document,
            signer: signer.to_owned(),
        };
        // Public synthetic validation key. Never a release anchor, never output.
        // This lets runtime parsers validate before the real key is decrypted.
        let fixture = SigningKey::from_bytes(&[42; 32]);
        let trust = PackageTrust::from_public_keys([(
            signer.to_owned(),
            fixture.verifying_key().to_bytes(),
        )])
        .map_err(SigningInputError::Package)?;
        prepared.finalize(
            signer,
            fixture.sign(&prepared.signing_message()).to_bytes(),
            &trust,
            now,
        )?;
        Ok(prepared)
    }

    pub fn signing_message(&self) -> Vec<u8> {
        let domain = match self.kind {
            SigningKind::Package => crate::language_package::SIGNATURE_DOMAIN,
            SigningKind::Catalog => crate::package_catalog::DOMAIN,
        };
        let mut result = domain.to_vec();
        result.extend_from_slice(self.document.as_bytes());
        result
    }

    /// Rechecks the resulting signature and full runtime schema. A later catalog
    /// expiry still refuses output, even when preparation previously succeeded.
    pub fn finalize(
        &self,
        signer: &str,
        signature: [u8; 64],
        trust: &PackageTrust,
        now: u64,
    ) -> Result<Vec<u8>, SigningInputError> {
        if signer != self.signer {
            return Err(SigningInputError::InvalidInput);
        }
        let mut envelope = self.envelope.clone();
        envelope["signer"] = json!(signer);
        envelope["signature"] = json!(
            signature
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let bytes = serde_json::to_vec(&envelope).map_err(|_| SigningInputError::InvalidInput)?;
        match self.kind {
            SigningKind::Package => {
                let package = VerifiedLanguagePackage::verify(&bytes, trust)
                    .map_err(SigningInputError::Package)?;
                if crate::package_inventory::is_base(package.id()) {
                    return Err(SigningInputError::InvalidInput);
                }
            }
            SigningKind::Catalog => {
                let catalog = VerifiedReleaseCatalog::verify(
                    &bytes,
                    trust,
                    DEFAULT_PACKAGE_REPOSITORY,
                    None,
                    now,
                )
                .map_err(SigningInputError::Catalog)?;
                if catalog
                    .packages()
                    .any(|package| crate::package_inventory::is_base(package.id()))
                {
                    return Err(SigningInputError::InvalidInput);
                }
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn draft() -> Value {
        let components = json!({"license":"synthetic test license","notice":"synthetic test notice",
            "ui":r#"{"format":1,"locale":"ru","direction":"ltr","messages":{"locale.self_name":"Русский"}}"#});
        let mut hashes = json!({});
        for (role, content) in components.as_object().unwrap() {
            let content = content.as_str().unwrap();
            let digest: String = Sha256::digest(content.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            hashes[role] = json!({"bytes":content.len(),"sha256":digest});
        }
        let manifest = json!({"format":1,"package_id":"ru-RU","revision":1,"runtime_api":1,"ui_locale":"ru","components":hashes}).to_string();
        json!({"format":1,"manifest":manifest,"components":components})
    }
    #[test]
    fn validates_package_before_signing_and_rechecks_final_signature() {
        let prepared = PreparedSigningInput::prepare(
            SigningKind::Package,
            "fixture",
            &serde_json::to_vec(&draft()).unwrap(),
            100,
        )
        .unwrap();
        let key = SigningKey::from_bytes(&[44; 32]);
        let trust =
            PackageTrust::from_public_keys([("fixture".into(), key.verifying_key().to_bytes())])
                .unwrap();
        let signature = key.sign(&prepared.signing_message()).to_bytes();
        let signed = prepared
            .finalize("fixture", signature, &trust, 100)
            .unwrap();
        assert!(VerifiedLanguagePackage::verify(&signed, &trust).is_ok());
        assert!(prepared.finalize("fixture", [0; 64], &trust, 100).is_err());
        assert!(
            PreparedSigningInput::prepare(SigningKind::Package, "fixture", &signed, 100).is_err()
        );
        let mut corrupt = draft();
        corrupt["components"]["license"] = json!("changed");
        assert!(
            PreparedSigningInput::prepare(
                SigningKind::Package,
                "fixture",
                &serde_json::to_vec(&corrupt).unwrap(),
                100
            )
            .is_err()
        );
    }
    #[test]
    fn duplicate_roles_and_unknown_fields_are_rejected() {
        let draft = serde_json::to_string(&draft()).unwrap();
        let duplicate = draft.replacen(
            "\"components\":{",
            "\"components\":{\"license\":\"duplicate\",",
            1,
        );
        assert!(
            PreparedSigningInput::prepare(
                SigningKind::Package,
                "fixture",
                duplicate.as_bytes(),
                100
            )
            .is_err()
        );
        let unknown = draft.replacen('{', "{\"unexpected\":true,", 1);
        assert!(
            PreparedSigningInput::prepare(SigningKind::Package, "fixture", unknown.as_bytes(), 100)
                .is_err()
        );
    }
    #[test]
    fn protected_base_is_not_an_external_signing_candidate() {
        let mut value = draft();
        let mut manifest: Value =
            serde_json::from_str(value["manifest"].as_str().unwrap()).unwrap();
        manifest["package_id"] = json!("en-US");
        value["manifest"] = json!(manifest.to_string());
        assert!(
            PreparedSigningInput::prepare(
                SigningKind::Package,
                "fixture",
                &serde_json::to_vec(&value).unwrap(),
                100
            )
            .is_err()
        );
    }
    #[test]
    fn oversized_signing_document_is_refused_before_validation_signature() {
        let mut value = draft();
        value["manifest"] = json!("x".repeat(crate::language_package::MAX_MANIFEST_BYTES + 1));
        assert!(matches!(
            PreparedSigningInput::prepare(
                SigningKind::Package,
                "fixture",
                &serde_json::to_vec(&value).unwrap(),
                100
            ),
            Err(SigningInputError::TooLarge)
        ));
    }
    #[test]
    fn catalog_scope_and_expiry_are_enforced() {
        let document = json!({"format":1,"repository":DEFAULT_PACKAGE_REPOSITORY,"revision":1,"issued_at":100,"expires_at":200,"packages":[]});
        let draft = json!({"format":1,"catalog":document.to_string()});
        let prepared = PreparedSigningInput::prepare(
            SigningKind::Catalog,
            "fixture",
            &serde_json::to_vec(&draft).unwrap(),
            100,
        )
        .unwrap();
        let key = SigningKey::from_bytes(&[44; 32]);
        let trust =
            PackageTrust::from_public_keys([("fixture".into(), key.verifying_key().to_bytes())])
                .unwrap();
        assert!(
            prepared
                .finalize(
                    "fixture",
                    key.sign(&prepared.signing_message()).to_bytes(),
                    &trust,
                    200
                )
                .is_err()
        );
        assert!(
            prepared
                .finalize(
                    "fixture",
                    key.sign(&prepared.signing_message()).to_bytes(),
                    &trust,
                    100
                )
                .is_ok()
        );
        let mut wrong = document;
        wrong["repository"] = json!("other/repository");
        assert!(
            PreparedSigningInput::prepare(
                SigningKind::Catalog,
                "fixture",
                &serde_json::to_vec(&json!({"format":1,"catalog":wrong.to_string()})).unwrap(),
                100
            )
            .is_err()
        );
    }
}
