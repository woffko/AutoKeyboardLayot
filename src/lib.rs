//! Privacy-first core for the AutoKeyboardLayot Windows user agent.
//!
//! The portable input-processing core has no Win32 dependencies; conditional
//! Windows integration helpers are separate modules. Platform adapters feed normalized input
//! events into [`InputSession`], while [`Detector`] evaluates completed words.

pub mod backend_rules;
pub mod bounded_probe;
pub mod configuration;
pub mod conversion;
pub mod detector;
pub mod dictionary_registry;
pub mod hotkey;
pub mod input_capabilities;
pub mod input_pack_selection;
pub mod input_profile;
pub mod input_profile_selection;
#[cfg(windows)]
pub mod installation_fence;
pub mod installed_packages;
pub mod installer_packages;
pub mod installer_profile;
pub mod installer_protocol;
pub mod installer_session;
pub mod installer_worker;
pub mod language;
pub mod language_package;
pub mod localization;
pub mod package_catalog;
pub mod package_download;
pub mod package_install;
pub mod package_inventory;
#[cfg(feature = "signing-tools")]
pub mod package_signing;
pub mod package_store;
pub mod privacy;
pub mod profile_resolver;
pub mod scoring_model;
pub mod session;
pub mod settings;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tray_visual;
pub mod user_lexicon;
#[cfg(windows)]
pub mod windows_input_profiles;

pub use backend_rules::{BackendRuleError, BackendRules, BackendStrategy};
pub use configuration::{CONFIGURATION_SCHEMA_VERSION, ConfigurationDocument, ConfigurationError};
pub use conversion::{ConversionTransaction, TextEdit};
pub use detector::{Detection, Detector, DetectorConfig};
pub use dictionary_registry::{DictionaryError, DictionaryPack, DictionaryRegistry, PackId};
pub use hotkey::{
    HOTKEY_MOD_ALT, HOTKEY_MOD_CONTROL, HOTKEY_MOD_SHIFT, HOTKEY_MOD_WIN, Hotkey, HotkeyError,
};
pub use input_profile::{
    InputDescriptorError, InputPackDescriptor, InputProfileRequirements, WindowsKeyboardProfile,
};
pub use language::{
    AutomaticCorrection, Language, LanguagePack, PackKind, language_packs, transpose_word,
};
pub use privacy::{ExclusionPolicy, PrivacyBlockReason};
pub use scoring_model::{ScoringModel, ScoringModelError};
pub use session::{InputEvent, InputSession, ResetReason, SessionAction};
pub use settings::Settings;
pub use user_lexicon::UserLexicon;
