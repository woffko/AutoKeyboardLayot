//! Owned input-profile requirements, independent of runtime adapter readiness.
//!
//! These identifiers come from package data or an OS resolver. Never construct
//! them by reinterpreting the bits of an opaque Windows HKL.

use crate::PackId;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, OnceLock},
};

/// Exact Windows keyboard input profile: LANGID plus KLID. A language alone
/// does not identify its physical keyboard, and a KLID can serve many languages.
/// This type deliberately does not represent a TSF text-service profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowsKeyboardProfile {
    language_id: u16,
    klid: u32,
}

impl WindowsKeyboardProfile {
    pub fn parse(value: &str) -> Result<Self, InputDescriptorError> {
        let bytes = value.as_bytes();
        if bytes.len() != 13
            || bytes[4] != b':'
            || !bytes[..4].iter().all(u8::is_ascii_hexdigit)
            || !bytes[5..].iter().all(u8::is_ascii_hexdigit)
        {
            return Err(InputDescriptorError::InvalidData);
        }
        // The checks above guarantee ASCII boundaries and valid hexadecimal.
        let language_id =
            u16::from_str_radix(&value[..4], 16).map_err(|_| InputDescriptorError::InvalidData)?;
        let klid =
            u32::from_str_radix(&value[5..], 16).map_err(|_| InputDescriptorError::InvalidData)?;
        if language_id == 0 || klid == 0 {
            return Err(InputDescriptorError::InvalidData);
        }
        Ok(Self { language_id, klid })
    }

    pub const fn language_id(self) -> u16 {
        self.language_id
    }
    pub const fn klid(self) -> u32 {
        self.klid
    }
}

impl fmt::Display for WindowsKeyboardProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}:{:08X}", self.language_id, self.klid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDescriptorError {
    InvalidData,
    LimitExceeded,
    PackIdMismatch,
}
impl fmt::Display for InputDescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "input_descriptor_{self:?}")
    }
}
impl std::error::Error for InputDescriptorError {}

/// A package requests these capabilities; it cannot assert that they exist,
/// select an implementation, or declare that a profile is safe for conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputProfileRequirements {
    required_capabilities: BTreeSet<String>,
}
impl InputProfileRequirements {
    pub fn required_capabilities(&self) -> impl Iterator<Item = &str> {
        self.required_capabilities.iter().map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPackDescriptor {
    pack_id: PackId,
    profiles: BTreeMap<WindowsKeyboardProfile, InputProfileRequirements>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: u32,
    pack_id: String,
    windows_keyboard_profiles: Vec<ProfileDocument>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    profile: String,
    required_capabilities: Vec<String>,
}

impl InputPackDescriptor {
    /// Bounded data parsing only. No filesystem, OS activation, code loading,
    /// capability registration, or conversion authorization happens here.
    pub fn from_json(bytes: &[u8]) -> Result<Self, InputDescriptorError> {
        use InputDescriptorError::{InvalidData, LimitExceeded};
        if bytes.len() > 16 * 1024 {
            return Err(LimitExceeded);
        }
        let document: Document = serde_json::from_slice(bytes).map_err(|_| InvalidData)?;
        if document.format != 1 || document.windows_keyboard_profiles.is_empty() {
            return Err(InvalidData);
        }
        if document.windows_keyboard_profiles.len() > 32 {
            return Err(LimitExceeded);
        }
        let pack_id = PackId::parse(&document.pack_id).map_err(|_| InvalidData)?;
        let mut profiles = BTreeMap::new();
        for profile in document.windows_keyboard_profiles {
            let id = WindowsKeyboardProfile::parse(&profile.profile)?;
            if profile.required_capabilities.is_empty() {
                return Err(InvalidData);
            }
            if profile.required_capabilities.len() > 8 {
                return Err(LimitExceeded);
            }
            let mut required_capabilities = BTreeSet::new();
            for capability in profile.required_capabilities {
                let bytes = capability.as_bytes();
                if bytes.is_empty()
                    || bytes.len() > 63
                    || !bytes[0].is_ascii_lowercase()
                    || !bytes
                        .iter()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
                    || !required_capabilities.insert(capability)
                {
                    return Err(InvalidData);
                }
            }
            if profiles
                .insert(
                    id,
                    InputProfileRequirements {
                        required_capabilities,
                    },
                )
                .is_some()
            {
                return Err(InvalidData);
            }
        }
        Ok(Self { pack_id, profiles })
    }

    pub const fn pack_id(&self) -> PackId {
        self.pack_id
    }
    pub fn profiles(
        &self,
    ) -> impl Iterator<Item = (&WindowsKeyboardProfile, &InputProfileRequirements)> {
        self.profiles.iter()
    }
    /// Exact lookup only: no same-language, default-layout or variant fallback.
    /// A match returns requirements, never proof of installed/ready capability.
    pub fn profile(&self, id: WindowsKeyboardProfile) -> Option<&InputProfileRequirements> {
        self.profiles.get(&id)
    }
}

pub(crate) fn embedded_descriptor(id: &PackId) -> Option<Arc<InputPackDescriptor>> {
    static DESCRIPTORS: OnceLock<BTreeMap<PackId, Arc<InputPackDescriptor>>> = OnceLock::new();
    DESCRIPTORS
        .get_or_init(|| {
            let mut descriptors = BTreeMap::new();
            let mut insert = |bytes: &[u8]| {
                let descriptor =
                    InputPackDescriptor::from_json(bytes).expect("valid embedded descriptor");
                assert!(
                    descriptors
                        .insert(descriptor.pack_id(), Arc::new(descriptor))
                        .is_none(),
                    "duplicate embedded input descriptor"
                );
            };
            insert(include_bytes!("../data/input/en-US.json"));
            #[cfg(feature = "legacy-bundled-input")]
            for bytes in [
                &include_bytes!("../data/input/ru-RU.json")[..],
                &include_bytes!("../data/input/et-EE.json")[..],
            ] {
                insert(bytes);
            }
            descriptors
        })
        .get(id)
        .cloned()
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "legacy-bundled-input"))]
    #[test]
    fn base_build_excludes_optional_descriptors() {
        for id in ["ru-RU", "et-EE"] {
            assert!(super::embedded_descriptor(&crate::PackId::parse(id).unwrap()).is_none());
        }
        assert!(super::embedded_descriptor(&crate::Language::English).is_some());
    }

    use super::*;
    use serde_json::{Value, json};

    fn valid() -> Value {
        json!({"format":1,"pack_id":"en-US","windows_keyboard_profiles":[
            {"profile":"0409:00000409","required_capabilities":["physical-key-v1"]}
        ]})
    }
    fn parse(value: &Value) -> Result<InputPackDescriptor, InputDescriptorError> {
        InputPackDescriptor::from_json(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn exact_profile_identity_preserves_language_and_keyboard_variant() {
        let descriptor = parse(&valid()).unwrap();
        let us = WindowsKeyboardProfile::parse("0409:00000409").unwrap();
        assert_eq!(us.language_id(), 0x0409);
        assert_eq!(us.klid(), 0x00000409);
        assert_eq!(us.to_string(), "0409:00000409");
        assert!(descriptor.profile(us).is_some());
        for other in ["0409:00020409", "0809:00000409", "0809:00000809"] {
            assert!(
                descriptor
                    .profile(WindowsKeyboardProfile::parse(other).unwrap())
                    .is_none()
            );
        }
        assert_eq!(
            WindowsKeyboardProfile::parse("040c:0000040c")
                .unwrap()
                .to_string(),
            "040C:0000040C"
        );
        for bad in [
            "",
            "04090409",
            "0409:409",
            " 0409:00000409",
            "0409:00000409 ",
            "0000:00000409",
            "0409:00000000",
            "0409:0000040G",
            "040é:00000409",
            "04é:00000409",
            "0409:{guid}",
        ] {
            assert!(WindowsKeyboardProfile::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn descriptor_rejects_unknown_fields_duplicates_and_invalid_requirements() {
        for bytes in [
            &br#"{"format":1,"format":1,"pack_id":"en-US","windows_keyboard_profiles":[{"profile":"0409:00000409","required_capabilities":["x"]}]}"#[..],
            &br#"{"format":1,"pack_id":"en-US","windows_keyboard_profiles":[{"profile":"0409:00000409","profile":"0409:00000409","required_capabilities":["x"]}]}"#[..],
        ] {
            assert!(InputPackDescriptor::from_json(bytes).is_err());
        }
        for field in ["format", "pack_id", "windows_keyboard_profiles"] {
            let mut value = valid();
            value.as_object_mut().unwrap().remove(field);
            assert!(parse(&value).is_err());
        }
        for replacement in [json!(0), json!(2), json!("1")] {
            let mut value = valid();
            value["format"] = replacement;
            assert!(parse(&value).is_err());
        }
        for bad in ["", "../code.dll", "Physical-key-v1", "x_y", "é", "-x"] {
            let mut value = valid();
            value["windows_keyboard_profiles"][0]["required_capabilities"] = json!([bad]);
            assert!(parse(&value).is_err());
        }
        let mut value = valid();
        value["ready"] = json!(true);
        assert!(parse(&value).is_err());
        let mut value = valid();
        value["windows_keyboard_profiles"][0]["ready"] = json!(true);
        assert!(parse(&value).is_err());
        let mut value = valid();
        value["windows_keyboard_profiles"][0]["required_capabilities"] = json!([]);
        assert!(parse(&value).is_err());
        let mut value = valid();
        value["windows_keyboard_profiles"][0]["required_capabilities"] = json!(["x", "x"]);
        assert!(parse(&value).is_err());
        let mut value = valid();
        let profile = value["windows_keyboard_profiles"][0].clone();
        value["windows_keyboard_profiles"] = json!([profile, profile]);
        assert!(parse(&value).is_err());
        value["windows_keyboard_profiles"] = json!([]);
        assert!(parse(&value).is_err());
    }

    #[test]
    fn descriptor_limits_count_raw_rows_and_bytes() {
        let mut boundary = serde_json::to_vec(&valid()).unwrap();
        boundary.resize(16 * 1024, b' ');
        assert!(InputPackDescriptor::from_json(&boundary).is_ok());
        boundary.push(b' ');
        assert_eq!(
            InputPackDescriptor::from_json(&boundary),
            Err(InputDescriptorError::LimitExceeded)
        );
        assert_eq!(
            InputPackDescriptor::from_json(&vec![b' '; 16 * 1024 + 1]),
            Err(InputDescriptorError::LimitExceeded)
        );
        let mut value = valid();
        let profile = value["windows_keyboard_profiles"][0].clone();
        value["windows_keyboard_profiles"] = json!(vec![profile; 33]);
        assert_eq!(parse(&value), Err(InputDescriptorError::LimitExceeded));
        let mut value = valid();
        value["windows_keyboard_profiles"][0]["required_capabilities"] = json!(vec!["x"; 9]);
        assert_eq!(parse(&value), Err(InputDescriptorError::LimitExceeded));
        value["windows_keyboard_profiles"][0]["required_capabilities"] = json!(["x".repeat(64)]);
        assert!(parse(&value).is_err());
        // Exact count/string limits remain accepted, not accidentally off by one.
        let mut value = valid();
        let mut profile = value["windows_keyboard_profiles"][0].clone();
        profile["required_capabilities"] = json!(
            (0..8)
                .map(|index| format!("{}{}", "x".repeat(62), index))
                .collect::<Vec<_>>()
        );
        value["windows_keyboard_profiles"] = json!(
            (1..=32)
                .map(|index| {
                    let mut row = profile.clone();
                    row["profile"] = json!(format!("0409:{index:08X}"));
                    row
                })
                .collect::<Vec<_>>()
        );
        // Full 63-byte names in 32 rows exceed the byte budget by design.
        assert_eq!(parse(&value), Err(InputDescriptorError::LimitExceeded));
        value["windows_keyboard_profiles"] = json!([profile]);
        assert!(parse(&value).is_ok());
        value["windows_keyboard_profiles"] = json!((1..=32).map(|index| json!({
            "profile":format!("0409:{index:08X}"),"required_capabilities":["physical-key-v1"]
        })).collect::<Vec<_>>());
        assert_eq!(parse(&value).unwrap().profiles().count(), 32);
    }

    #[test]
    fn embedded_profiles_are_owned_data_without_readiness_flags() {
        for (id, profile) in [
            ("en-US", "0409:00000409"),
            ("ru-RU", "0419:00000419"),
            ("et-EE", "0425:00000425"),
        ] {
            let id = PackId::parse(id).unwrap();
            if id != crate::Language::English && !cfg!(feature = "legacy-bundled-input") {
                assert!(embedded_descriptor(&id).is_none());
                continue;
            }
            let descriptor = embedded_descriptor(&id).unwrap();
            assert_eq!(descriptor.pack_id(), id);
            assert_eq!(descriptor.profiles().count(), 1);
            assert!(
                descriptor
                    .profile(WindowsKeyboardProfile::parse(profile).unwrap())
                    .is_some()
            );
            assert!(Arc::ptr_eq(&descriptor, &embedded_descriptor(&id).unwrap()));
        }
        assert!(embedded_descriptor(&PackId::parse("ja-JP").unwrap()).is_none());
        let mut value = valid();
        value["pack_id"] = json!("de-DE");
        value["windows_keyboard_profiles"][0]["required_capabilities"] =
            json!(["future-adapter-v9"]);
        let descriptor = parse(&value).unwrap();
        drop(value);
        assert_eq!(descriptor.pack_id(), PackId::parse("de-DE").unwrap());
        assert_eq!(
            descriptor
                .profiles()
                .next()
                .unwrap()
                .1
                .required_capabilities()
                .collect::<Vec<_>>(),
            ["future-adapter-v9"]
        );
    }
}
