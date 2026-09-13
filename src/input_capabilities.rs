//! Code-owned implementation inventory, separate from package requirements,
//! installed OS profiles, per-event safety checks and native acceptance.
//!
//! A package cannot register code or make an unimplemented capability available.
//! In particular, the legacy conservative ET path is not full ET support.

use crate::{InputProfileRequirements, WindowsKeyboardProfile};
use std::collections::BTreeSet;

/// Every current binding is limited to ordinary physical keys with optional
/// Shift/Caps Lock and one Unicode scalar per key. AltGr, dead-key state and
/// composition are outside this event scope even for a recognized profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputScope {
    ConservativePhysicalKeys,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputBinding {
    profile: WindowsKeyboardProfile,
    scope: InputScope,
}

impl InputBinding {
    pub const fn profile(self) -> WindowsKeyboardProfile {
        self.profile
    }
    pub const fn scope(self) -> InputScope {
        self.scope
    }

    // The legacy offline table implements this exact keyboard pair only.
    // Package identity never selects its layout. ET requires native scan codes.
    fn offline_layout(self) -> Option<crate::Language> {
        match (self.profile.language_id(), self.profile.klid()) {
            (0x0409, 0x00000409) => Some(crate::Language::English),
            (0x0419, 0x00000419) => Some(crate::Language::Russian),
            _ => None,
        }
    }
    pub(crate) fn transpose_character(self, character: char, target: Self) -> Option<char> {
        crate::language::transpose_character(
            character,
            self.offline_layout()?,
            target.offline_layout()?,
        )
    }
    pub(crate) fn transpose_word(self, word: &str, target: Self) -> Option<String> {
        crate::language::transpose_word(word, self.offline_layout()?, target.offline_layout()?)
    }
}

/// Select a code-owned, conservative adapter binding. This is not full profile
/// readiness. The ET exception explicitly preserves only the physical subset
/// of its known full requirements; any additional unknown requirement blocks it.
pub(crate) fn bind_conservative_profile(
    profile: WindowsKeyboardProfile,
    requirements: &InputProfileRequirements,
) -> Option<InputBinding> {
    let supported = match assess_profile_implementation(profile, requirements) {
        ProfileImplementation::Implemented => true,
        ProfileImplementation::MissingCapabilities(missing)
            if (profile.language_id(), profile.klid()) == (0x0425, 0x00000425) =>
        {
            requirements
                .required_capabilities()
                .any(|cap| cap == "physical-key-v1")
                && missing == BTreeSet::from(["altgr-v1".to_owned(), "dead-key-v1".to_owned()])
        }
        _ => false,
    };
    supported.then_some(InputBinding {
        profile,
        scope: InputScope::ConservativePhysicalKeys,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileImplementation {
    /// No reviewed adapter binding for this exact LANGID/KLID pair.
    UnsupportedProfile,
    /// The exact profile is known, but some requested implementations are absent.
    MissingCapabilities(BTreeSet<String>),
    /// Only the implementation requirements match. This does not authorize input
    /// conversion or assert installed-profile, privacy, event or native readiness.
    Implemented,
}

/// Assess full requirements against implementations owned by this executable.
/// No same-language/default-keyboard inference and no data-defined grants.
pub fn assess_profile_implementation(
    profile: WindowsKeyboardProfile,
    requirements: &InputProfileRequirements,
) -> ProfileImplementation {
    let capabilities: &[&str] = match (profile.language_id(), profile.klid()) {
        (0x0409, 0x00000409) | (0x0419, 0x00000419) | (0x0425, 0x00000425) => &["physical-key-v1"],
        _ => return ProfileImplementation::UnsupportedProfile,
    };
    let missing: BTreeSet<String> = requirements
        .required_capabilities()
        .filter(|capability| !capabilities.contains(capability))
        .map(str::to_owned)
        .collect();
    if missing.is_empty() {
        ProfileImplementation::Implemented
    } else {
        ProfileImplementation::MissingCapabilities(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputPackDescriptor, PackId};

    #[test]
    fn conservative_et_binding_does_not_grant_full_or_unknown_capabilities() {
        let descriptor = crate::test_support::descriptor(&crate::Language::Estonian).unwrap();
        let (profile, requirements) = descriptor.profiles().next().unwrap();
        let binding = bind_conservative_profile(*profile, requirements).unwrap();
        assert_eq!(binding.scope(), InputScope::ConservativePhysicalKeys);
        assert!(matches!(
            assess_profile_implementation(*profile, requirements),
            ProfileImplementation::MissingCapabilities(_)
        ));
        assert_eq!(binding.transpose_word("tere", binding), None);
        for capabilities in [
            vec!["altgr-v1", "dead-key-v1"],
            vec!["physical-key-v1", "altgr-v1", "dead-key-v1", "future-v9"],
        ] {
            let changed = InputPackDescriptor::from_json(
                &serde_json::to_vec(&serde_json::json!({
                    "format":1,"pack_id":"et-EE","windows_keyboard_profiles":[{
                        "profile":"0425:00000425","required_capabilities":capabilities
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
            let (profile, requirements) = changed.profiles().next().unwrap();
            assert!(bind_conservative_profile(*profile, requirements).is_none());
        }
    }

    #[test]
    fn embedded_requirements_do_not_grant_missing_et_implementations() {
        for id in ["en-US", "ru-RU", "et-EE"] {
            let descriptor = crate::test_support::descriptor(&PackId::parse(id).unwrap()).unwrap();
            let (profile, requirements) = descriptor.profiles().next().unwrap();
            let expected = if id == "et-EE" {
                ProfileImplementation::MissingCapabilities(BTreeSet::from([
                    "altgr-v1".to_owned(),
                    "dead-key-v1".to_owned(),
                ]))
            } else {
                ProfileImplementation::Implemented
            };
            assert_eq!(
                assess_profile_implementation(*profile, requirements),
                expected
            );
        }
    }

    #[test]
    fn assessment_is_exact_profile_based_not_pack_id_based() {
        let descriptor = InputPackDescriptor::from_json(
            br#"{
            "format":1,"pack_id":"custom-pack","windows_keyboard_profiles":[
                {"profile":"0409:00000409","required_capabilities":["physical-key-v1"]},
                {"profile":"0409:00020409","required_capabilities":["physical-key-v1"]},
                {"profile":"0809:00000409","required_capabilities":["physical-key-v1"]}
            ]}"#,
        )
        .unwrap();
        for (profile, requirements) in descriptor.profiles() {
            let expected = if profile.to_string() == "0409:00000409" {
                ProfileImplementation::Implemented
            } else {
                ProfileImplementation::UnsupportedProfile
            };
            assert_eq!(
                assess_profile_implementation(*profile, requirements),
                expected
            );
        }
    }

    #[test]
    fn unknown_and_complex_capabilities_are_missing_not_ignored() {
        let descriptor = InputPackDescriptor::from_json(
            br#"{
            "format":1,"pack_id":"en-US","windows_keyboard_profiles":[
                {"profile":"0409:00000409","required_capabilities":[
                    "physical-key-v1","altgr-v1","dead-key-v1","ime-v1","composition-guard-v1","future-v9"
                ]}
            ]}"#,
        )
        .unwrap();
        let (profile, requirements) = descriptor.profiles().next().unwrap();
        assert_eq!(
            assess_profile_implementation(*profile, requirements),
            ProfileImplementation::MissingCapabilities(
                [
                    "altgr-v1",
                    "dead-key-v1",
                    "ime-v1",
                    "composition-guard-v1",
                    "future-v9"
                ]
                .map(str::to_owned)
                .into_iter()
                .collect()
            )
        );
    }
}
