//! Display-language selection for both the tray process and the settings UI.
//! Initialization is startup/UI work; never call it from a low-level hook.

use std::sync::{Arc, OnceLock, RwLock};

use autokeyboardlayot::localization::{
    CatalogRegistry, Localizer, TextDirection, UiLanguageChoice, UiLanguagePreference,
};
use windows::{
    Win32::{
        Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME},
        UI::WindowsAndMessaging::{
            MB_RIGHT, MB_RTLREADING, MESSAGEBOX_STYLE, TPM_LAYOUTRTL, TRACK_POPUP_MENU_FLAGS,
        },
    },
    core::PWSTR,
};

struct UiState {
    registry: CatalogRegistry,
    selected: Arc<Localizer>,
    revision: u64,
}

fn state() -> &'static RwLock<UiState> {
    static STATE: OnceLock<RwLock<UiState>> = OnceLock::new();
    STATE.get_or_init(|| {
        RwLock::new(UiState {
            registry: CatalogRegistry::default(),
            selected: Arc::new(Localizer::default()),
            revision: 0,
        })
    })
}

fn snapshot() -> Arc<Localizer> {
    Arc::clone(
        &state()
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .selected,
    )
}

pub(super) fn initialize(preference: &UiLanguagePreference, installed: Option<&CatalogRegistry>) {
    if let Some(installed) = installed {
        apply_installed(preference, Some(installed));
        return;
    }
    // Unit tests do not inspect the real user's locale directories or depend on
    // their display language. Core tests exercise catalogs with explicit data.
    if cfg!(test) {
        apply_preference(preference);
        return;
    }
    let mut registry = CatalogRegistry::default();
    // An installed local catalog takes precedence over a bundled locale. Both
    // paths contain translation data only, never profiles or user dictionaries.
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let _ = registry.load_directory(
            &std::path::PathBuf::from(local)
                .join("AutoKeyboardLayot")
                .join("locales"),
        );
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let _ = registry.load_directory(&directory.join("locales"));
    }
    let system_locale = system_ui_language();
    let selected = Arc::new(registry.select(preference.requested_locale(&system_locale)));
    let mut state = state().write().unwrap_or_else(|error| error.into_inner());
    let revision = state.revision.wrapping_add(1);
    *state = UiState {
        registry,
        selected,
        revision,
    };
}

fn system_ui_language() -> String {
    if cfg!(test) {
        return "en".to_owned();
    }
    preferred_windows_ui_language().unwrap_or_else(|| "en".to_owned())
}

/// Changes only the in-memory UI snapshot; no catalog file reads or HKL changes.
pub(super) fn apply_preference(preference: &UiLanguagePreference) {
    apply_installed(preference, None);
}

/// Adopt the catalog snapshot acknowledged with the runtime configuration.
/// None changes only preference (legacy catalogs were loaded at startup).
pub(super) fn apply_installed(
    preference: &UiLanguagePreference,
    installed: Option<&CatalogRegistry>,
) {
    let system_locale = system_ui_language();
    let mut state = state().write().unwrap_or_else(|error| error.into_inner());
    if let Some(installed) = installed {
        state.registry = installed.clone();
    }
    state.selected = Arc::new(
        state
            .registry
            .select(preference.requested_locale(&system_locale)),
    );
    state.revision = state.revision.wrapping_add(1);
}

pub(super) fn revision() -> u64 {
    state()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .revision
}

pub(super) fn picker_choices(preference: &UiLanguagePreference) -> (Vec<UiLanguageChoice>, usize) {
    let state = state().read().unwrap_or_else(|error| error.into_inner());
    state
        .registry
        .picker_choices(preference, state.selected.text("ui_language.system"))
}

pub(super) fn active_language_name() -> String {
    snapshot().language_name().to_owned()
}

pub(super) fn is_rtl() -> bool {
    snapshot().direction() == TextDirection::Rtl
}

pub(super) fn message_box_style(base: MESSAGEBOX_STYLE) -> MESSAGEBOX_STYLE {
    directional_message_box_style(base, is_rtl())
}

fn directional_message_box_style(base: MESSAGEBOX_STYLE, rtl: bool) -> MESSAGEBOX_STYLE {
    if rtl {
        base | MB_RIGHT | MB_RTLREADING
    } else {
        base
    }
}

pub(super) fn popup_menu_style(base: TRACK_POPUP_MENU_FLAGS) -> TRACK_POPUP_MENU_FLAGS {
    directional_popup_menu_style(base, is_rtl())
}

fn directional_popup_menu_style(base: TRACK_POPUP_MENU_FLAGS, rtl: bool) -> TRACK_POPUP_MENU_FLAGS {
    if rtl { base | TPM_LAYOUTRTL } else { base }
}

fn preferred_windows_ui_language() -> Option<String> {
    let mut language_count = 0u32;
    let mut length = 0u32;
    unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut language_count, None, &mut length)
            .ok()?;
    }
    if language_count == 0 || length == 0 || length > 32_768 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut language_count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
        .ok()?;
    }
    first_ui_language(&buffer)
}

fn first_ui_language(buffer: &[u16]) -> Option<String> {
    let end = buffer.iter().position(|unit| *unit == 0)?;
    if end == 0 {
        return None;
    }
    String::from_utf16(&buffer[..end]).ok()
}

pub(super) fn tr(id: &str) -> String {
    snapshot().text(id).to_owned()
}

pub(super) fn tr_format(id: &str, arguments: &[(&str, &str)]) -> String {
    let localizer = snapshot();
    localizer
        .format(id, arguments)
        .unwrap_or_else(|_| localizer.text("text.unavailable").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_rtl_styles_preserve_buttons_defaults_and_menu_dispatch() {
        use windows::Win32::UI::WindowsAndMessaging::{
            MB_DEFBUTTON2, MB_ICONINFORMATION, MB_YESNO, TPM_RIGHTBUTTON,
        };
        let base = MB_YESNO | MB_DEFBUTTON2 | MB_ICONINFORMATION;
        assert_eq!(directional_message_box_style(base, false), base);
        let rtl = directional_message_box_style(base, true);
        assert_eq!(rtl, base | MB_RIGHT | MB_RTLREADING);
        assert_eq!(rtl.0 & !(MB_RIGHT | MB_RTLREADING).0, base.0);
        assert_eq!(directional_message_box_style(rtl, true), rtl);
        assert_eq!(
            directional_popup_menu_style(TPM_RIGHTBUTTON, false),
            TPM_RIGHTBUTTON
        );
        let rtl_menu = directional_popup_menu_style(TPM_RIGHTBUTTON, true);
        assert_eq!(rtl_menu, TPM_RIGHTBUTTON | TPM_LAYOUTRTL);
        assert_eq!(rtl_menu.0 & !TPM_LAYOUTRTL.0, TPM_RIGHTBUTTON.0);
    }

    #[test]
    fn display_language_list_uses_the_first_preference() {
        let buffer: Vec<_> = "ru-RU\0en-US\0\0".encode_utf16().collect();
        assert_eq!(first_ui_language(&buffer).as_deref(), Some("ru-RU"));
        assert_eq!(tr("button.apply"), "Apply");
    }

    #[test]
    fn empty_unterminated_or_invalid_utf16_language_is_unavailable() {
        assert_eq!(first_ui_language(&[]), None);
        assert_eq!(first_ui_language(&[0]), None);
        assert_eq!(first_ui_language(&[b'e' as u16, b'n' as u16]), None);
        assert_eq!(first_ui_language(&[0xd800, 0]), None);
    }

    #[test]
    fn an_unknown_formatted_key_still_produces_a_safe_english_ui_fallback() {
        assert_eq!(
            tr_format("unknown.key", &[("unused", "value")]),
            "Text unavailable"
        );
    }
}
