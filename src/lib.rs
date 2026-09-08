//! Privacy-first core for the AutoKeyboardLayot Windows user agent.
//!
//! The core has no Win32 dependencies. Platform adapters feed normalized input
//! events into [`InputSession`], while [`Detector`] evaluates completed words.

pub mod backend_rules;
pub mod bounded_probe;
pub mod configuration;
pub mod conversion;
pub mod detector;
mod dictionary;
pub mod hotkey;
pub mod language;
pub mod privacy;
pub mod session;
pub mod settings;
pub mod tray_visual;
pub mod user_lexicon;

pub use backend_rules::{BackendRuleError, BackendRules, BackendStrategy};
pub use configuration::{CONFIGURATION_SCHEMA_VERSION, ConfigurationDocument, ConfigurationError};
pub use conversion::{ConversionTransaction, TextEdit};
pub use detector::{Detection, Detector, DetectorConfig};
pub use hotkey::{
    HOTKEY_MOD_ALT, HOTKEY_MOD_CONTROL, HOTKEY_MOD_SHIFT, HOTKEY_MOD_WIN, Hotkey, HotkeyError,
};
pub use language::{
    AutomaticCorrection, Language, LanguagePack, PackKind, language_packs, transpose_word,
};
pub use privacy::{ExclusionPolicy, PrivacyBlockReason};
pub use session::{InputEvent, InputSession, ResetReason, SessionAction};
pub use settings::Settings;
pub use user_lexicon::UserLexicon;
