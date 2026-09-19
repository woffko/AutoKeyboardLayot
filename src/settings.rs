//! Persistent, non-secret user preferences.

use crate::PackId;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    pub line: usize,
    pub message: &'static str,
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for SettingsError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub automatic_conversion_on_startup: bool,
    pub pause_break_undo: bool,
    pub offer_word_exclusion_after_undo: bool,
    pub offer_dictionary_after_forced_conversion: bool,
    pub force_hotkey_virtual_key: u16,
    pub force_hotkey_modifiers: u8,
    pub recheck_first_word_after_erasing: bool,
    pub physical_fallback_for_unsupported_apps: bool,
    pub single_letter_words: bool,
    /// Explicitly permit manual conversion in verified terminals when only the
    /// focused field's UIA inspection is unavailable.
    pub manual_terminal_uia_fallback: bool,
    pub diagnostics_enabled: bool,
    pub suppress_after_backspace: bool,
    pub suppress_after_delete: bool,
    pub suppress_after_left: bool,
    pub suppress_after_right: bool,
    pub suppress_after_up: bool,
    pub suppress_after_down: bool,
    pub suppress_after_home_end: bool,
    pub suppress_after_manual_layout_change: bool,
    /// Selected IDs survive disabled/missing data and unsupported input adapters.
    pub enabled_input_packs: BTreeSet<PackId>,
}

impl Default for Settings {
    fn default() -> Self {
        let hotkey = crate::Hotkey::default();
        Self {
            automatic_conversion_on_startup: true,
            pause_break_undo: true,
            offer_word_exclusion_after_undo: true,
            offer_dictionary_after_forced_conversion: true,
            force_hotkey_virtual_key: hotkey.virtual_key,
            force_hotkey_modifiers: hotkey.modifiers,
            recheck_first_word_after_erasing: true,
            physical_fallback_for_unsupported_apps: true,
            single_letter_words: false,
            manual_terminal_uia_fallback: false,
            diagnostics_enabled: false,
            suppress_after_backspace: true,
            suppress_after_delete: true,
            suppress_after_left: true,
            suppress_after_right: true,
            suppress_after_up: true,
            suppress_after_down: true,
            suppress_after_home_end: true,
            suppress_after_manual_layout_change: true,
            enabled_input_packs: ["en-US", "ru-RU", "et-EE"]
                .map(|id| PackId::parse(id).expect("built-in ID is valid"))
                .into_iter()
                .collect(),
        }
    }
}

impl Settings {
    pub fn from_text(text: &str) -> Self {
        Self::parse_text(text, false).expect("tolerant settings parsing is infallible")
    }

    /// Reject malformed known values without rejecting forward-compatible keys.
    pub fn try_from_text(text: &str) -> Result<Self, SettingsError> {
        Self::parse_text(text, true)
    }

    fn parse_text(text: &str, strict: bool) -> Result<Self, SettingsError> {
        let mut settings = Self::default();
        let mut hotkey_key = None;
        let mut hotkey_modifiers = None;
        let mut hotkey_line = 1;
        let mut pack_selection_seen = false;
        let mut legacy_selection_seen = false;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                if strict {
                    return Err(SettingsError {
                        line: index + 1,
                        message: "expected a settings key=value pair",
                    });
                }
                continue;
            };
            let key = key.trim();
            if key == "enabled_input_packs" {
                if strict && (pack_selection_seen || legacy_selection_seen) {
                    return Err(SettingsError {
                        line: index + 1,
                        message: "conflicting input pack selection settings",
                    });
                }
                match parse_pack_selection(value) {
                    Ok(selection) => settings.enabled_input_packs = selection,
                    Err(message) if strict => {
                        return Err(SettingsError {
                            line: index + 1,
                            message,
                        });
                    }
                    Err(_) => continue,
                }
                pack_selection_seen = true;
                continue;
            }
            let legacy_id = match key {
                "enable_english" => Some("en-US"),
                "enable_russian" => Some("ru-RU"),
                "enable_estonian" => Some("et-EE"),
                _ => None,
            };
            if let Some(id) = legacy_id {
                if strict && pack_selection_seen {
                    return Err(SettingsError {
                        line: index + 1,
                        message: "conflicting input pack selection settings",
                    });
                }
                legacy_selection_seen = true;
                if let Some(value) = parse_bool(value) {
                    settings
                        .set_pack_enabled(PackId::parse(id).expect("legacy ID is valid"), value);
                } else if strict {
                    return Err(SettingsError {
                        line: index + 1,
                        message: "invalid boolean setting",
                    });
                }
                continue;
            }
            if key == "force_hotkey_key" {
                hotkey_key = Some(value.trim().to_owned());
                hotkey_line = index + 1;
                continue;
            }
            if key == "force_hotkey_modifiers" {
                hotkey_modifiers = Some(value.trim().to_owned());
                hotkey_line = index + 1;
                continue;
            }
            let destination = match key {
                "automatic_conversion_on_startup" => &mut settings.automatic_conversion_on_startup,
                "pause_break_undo" => &mut settings.pause_break_undo,
                "offer_word_exclusion_after_undo" => &mut settings.offer_word_exclusion_after_undo,
                "offer_dictionary_after_forced_conversion" => {
                    &mut settings.offer_dictionary_after_forced_conversion
                }
                "recheck_first_word_after_erasing" => {
                    &mut settings.recheck_first_word_after_erasing
                }
                "physical_fallback_for_unsupported_apps" => {
                    &mut settings.physical_fallback_for_unsupported_apps
                }
                "single_letter_words" => &mut settings.single_letter_words,
                "manual_terminal_uia_fallback" => &mut settings.manual_terminal_uia_fallback,
                "diagnostics_enabled" => &mut settings.diagnostics_enabled,
                "suppress_after_backspace" => &mut settings.suppress_after_backspace,
                "suppress_after_delete" => &mut settings.suppress_after_delete,
                "suppress_after_left" => &mut settings.suppress_after_left,
                "suppress_after_right" => &mut settings.suppress_after_right,
                "suppress_after_up" => &mut settings.suppress_after_up,
                "suppress_after_down" => &mut settings.suppress_after_down,
                "suppress_after_home_end" => &mut settings.suppress_after_home_end,
                "suppress_after_manual_layout_change" => {
                    &mut settings.suppress_after_manual_layout_change
                }
                _ => continue,
            };
            if let Some(value) = parse_bool(value) {
                *destination = value;
            } else if strict {
                return Err(SettingsError {
                    line: index + 1,
                    message: "invalid boolean setting",
                });
            }
        }
        if hotkey_key.is_some() || hotkey_modifiers.is_some() {
            let current = settings.force_hotkey();
            let key = hotkey_key.unwrap_or_else(|| current.key_name());
            let modifiers = hotkey_modifiers.unwrap_or_else(|| current.modifiers_text());
            if let Ok(hotkey) = crate::Hotkey::from_parts(&key, &modifiers) {
                settings.force_hotkey_virtual_key = hotkey.virtual_key;
                settings.force_hotkey_modifiers = hotkey.modifiers;
            } else if strict {
                return Err(SettingsError {
                    line: hotkey_line,
                    message: "invalid force hotkey",
                });
            }
        }
        Ok(settings)
    }

    pub fn to_text(&self) -> String {
        format!(
            "# AutoKeyboardLayot settings v2\n\
             automatic_conversion_on_startup={}\n\
             pause_break_undo={}\n\
             offer_word_exclusion_after_undo={}\n\
             offer_dictionary_after_forced_conversion={}\n\
             force_hotkey_key={}\n\
             force_hotkey_modifiers={}\n\
             recheck_first_word_after_erasing={}\n\
             physical_fallback_for_unsupported_apps={}\n\
             single_letter_words={}\n\
             manual_terminal_uia_fallback={}\n\
             diagnostics_enabled={}\n\
             suppress_after_backspace={}\n\
             suppress_after_delete={}\n\
             suppress_after_left={}\n\
             suppress_after_right={}\n\
             suppress_after_up={}\n\
             suppress_after_down={}\n\
             suppress_after_home_end={}\n\
             suppress_after_manual_layout_change={}\n\
             enabled_input_packs={}\n",
            self.automatic_conversion_on_startup,
            self.pause_break_undo,
            self.offer_word_exclusion_after_undo,
            self.offer_dictionary_after_forced_conversion,
            self.force_hotkey().key_name(),
            self.force_hotkey().modifiers_text(),
            self.recheck_first_word_after_erasing,
            self.physical_fallback_for_unsupported_apps,
            self.single_letter_words,
            self.manual_terminal_uia_fallback,
            self.diagnostics_enabled,
            self.suppress_after_backspace,
            self.suppress_after_delete,
            self.suppress_after_left,
            self.suppress_after_right,
            self.suppress_after_up,
            self.suppress_after_down,
            self.suppress_after_home_end,
            self.suppress_after_manual_layout_change,
            self.enabled_input_packs
                .iter()
                .map(PackId::as_str)
                .collect::<Vec<_>>()
                .join(","),
        )
    }

    pub fn language_enabled(&self, language: crate::Language) -> bool {
        self.enabled_input_packs.contains(&language)
    }

    pub fn set_pack_enabled(&mut self, id: PackId, enabled: bool) {
        if enabled {
            self.enabled_input_packs.insert(id);
        } else {
            self.enabled_input_packs.remove(&id);
        }
    }

    pub fn force_hotkey(&self) -> crate::Hotkey {
        crate::Hotkey::new(self.force_hotkey_virtual_key, self.force_hotkey_modifiers)
            .unwrap_or_default()
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_pack_selection(value: &str) -> Result<BTreeSet<PackId>, &'static str> {
    let mut selection = BTreeSet::new();
    if !value.trim().is_empty() {
        for (position, id) in value.trim().split(',').enumerate() {
            if position >= 64 {
                return Err("too many input pack selections");
            }
            selection.insert(PackId::parse(id.trim()).map_err(|_| "invalid input pack ID")?);
        }
    }
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_language_flags_migrate_to_ids_without_changing_other_settings() {
        for mask in 0u8..8 {
            let text = format!(
                "automatic_conversion_on_startup=true\npause_break_undo=false\ndiagnostics_enabled=true\nenable_english={}\nenable_russian={}\nenable_estonian={}\n",
                mask & 1 != 0,
                mask & 2 != 0,
                mask & 4 != 0
            );
            let settings = Settings::try_from_text(&text).unwrap();
            assert_eq!(
                settings.enabled_input_packs.len(),
                mask.count_ones() as usize
            );
            for (bit, language) in [
                (1, crate::Language::English),
                (2, crate::Language::Russian),
                (4, crate::Language::Estonian),
            ] {
                assert_eq!(settings.language_enabled(language), mask & bit != 0);
            }
            assert!(settings.automatic_conversion_on_startup);
            assert!(!settings.pause_break_undo);
            assert!(settings.diagnostics_enabled);
            let serialized = settings.to_text();
            assert!(!serialized.contains("enable_english="));
            assert_eq!(Settings::try_from_text(&serialized).unwrap(), settings);
        }
    }

    #[test]
    fn explicit_pack_selection_preserves_unknown_ids_and_rejects_ambiguity() {
        let mut settings =
            Settings::try_from_text("enabled_input_packs=EN-us,de-DE,zh-Hans").unwrap();
        settings.set_pack_enabled(PackId::parse("ru-RU").unwrap(), false);
        assert!(
            settings
                .enabled_input_packs
                .contains(&PackId::parse("de-DE").unwrap())
        );
        assert_eq!(
            Settings::try_from_text(&settings.to_text()).unwrap(),
            settings
        );
        for text in [
            "enabled_input_packs=en-US\nenable_russian=false",
            "enable_russian=false\nenabled_input_packs=en-US",
            "enabled_input_packs=en-US\nenabled_input_packs=ru-RU",
            "enabled_input_packs=../bad",
            "enabled_input_packs=en-US,",
            "enable_russian=invalid",
        ] {
            assert!(Settings::try_from_text(text).is_err(), "{text}");
        }
        let tolerant = Settings::from_text(
            "pause_break_undo=false\nenabled_input_packs=../bad\ndiagnostics_enabled=true",
        );
        assert!(!tolerant.pause_break_undo);
        assert!(tolerant.diagnostics_enabled);
        assert!(
            Settings::try_from_text("enabled_input_packs=")
                .unwrap()
                .enabled_input_packs
                .is_empty()
        );
        assert!(
            Settings::try_from_text(&format!(
                "enabled_input_packs={}",
                vec!["en-US"; 65].join(",")
            ))
            .is_err()
        );
    }

    #[test]
    fn strict_parser_validates_every_serialized_boolean_and_keeps_legacy_spellings() {
        for line in Settings::default().to_text().lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if !matches!(value, "true" | "false") {
                continue;
            }
            for invalid in ["", "maybe", "2", "tru"] {
                assert!(
                    Settings::try_from_text(&format!("{key}={invalid}")).is_err(),
                    "{key}"
                );
            }
            for valid in [
                "true", "false", "yes", "no", "on", "off", "1", "0", " TRUE ",
            ] {
                let text = format!("{key}={valid}");
                assert_eq!(
                    Settings::try_from_text(&text).unwrap(),
                    Settings::from_text(&text)
                );
            }
        }
    }

    #[test]
    fn strict_parser_rejects_malformed_lines_and_hotkeys_without_echoing_values() {
        for text in [
            "broken line",
            "force_hotkey_key=invalid-private-value",
            "force_hotkey_modifiers=invalid-private-value",
            "force_hotkey_key=A\nforce_hotkey_modifiers=None",
        ] {
            let error = Settings::try_from_text(text).unwrap_err();
            assert!(!error.to_string().contains("private-value"));
        }
        let error = Settings::try_from_text("# comment\n\npause_break_undo=bad").unwrap_err();
        assert_eq!(error.line, 3);
    }

    #[test]
    fn strict_parser_preserves_valid_roundtrips_partial_hotkeys_and_unknown_keys() {
        for text in [
            Settings::default().to_text(),
            "force_hotkey_key=F12".to_owned(),
            "force_hotkey_modifiers=Ctrl".to_owned(),
            "unknown=future-value\nui_language=ru\npause_break_undo=no".to_owned(),
        ] {
            assert_eq!(
                Settings::try_from_text(&text).unwrap(),
                Settings::from_text(&text)
            );
        }
    }

    #[test]
    fn defaults_enable_startup_conversion_but_keep_hotkey_and_safety_defaults() {
        let settings = Settings::default();
        assert!(settings.automatic_conversion_on_startup);
        assert!(settings.pause_break_undo);
        assert!(settings.offer_word_exclusion_after_undo);
        assert!(settings.offer_dictionary_after_forced_conversion);
        assert_eq!(settings.force_hotkey().display_name(), "Pause/Break");
        assert!(settings.recheck_first_word_after_erasing);
        assert!(settings.physical_fallback_for_unsupported_apps);
        assert!(!settings.single_letter_words);
        assert!(!settings.diagnostics_enabled);
        assert!(settings.suppress_after_backspace);
        assert!(settings.suppress_after_delete);
        assert!(settings.suppress_after_left);
        assert!(settings.suppress_after_right);
        assert!(settings.suppress_after_up);
        assert!(settings.suppress_after_down);
        assert!(settings.suppress_after_home_end);
        assert!(settings.suppress_after_manual_layout_change);
        assert!(settings.language_enabled(crate::Language::English));
        assert!(settings.language_enabled(crate::Language::Russian));
        assert!(settings.language_enabled(crate::Language::Estonian));
    }

    #[test]
    fn parser_is_forward_compatible_and_round_trips() {
        let parsed = Settings::from_text(
            "# comment\nautomatic_conversion_on_startup=yes\n\
             pause_break_undo=off\nunknown=true\n\
             offer_word_exclusion_after_undo=1\n\
             offer_dictionary_after_forced_conversion=no\n\
             force_hotkey_key=F12\n\
             force_hotkey_modifiers=Ctrl+Shift\n\
             recheck_first_word_after_erasing=no\n\
             physical_fallback_for_unsupported_apps=on\n\
             single_letter_words=yes\n\
             manual_terminal_uia_fallback=yes\n\
             diagnostics_enabled=true\n\
             suppress_after_backspace=off\n\
             suppress_after_delete=no\n\
             suppress_after_left=false\n\
             suppress_after_manual_layout_change=0\n\
             enable_estonian=off\n",
        );
        assert!(parsed.automatic_conversion_on_startup);
        assert!(!parsed.pause_break_undo);
        assert!(parsed.offer_word_exclusion_after_undo);
        assert!(!parsed.offer_dictionary_after_forced_conversion);
        assert_eq!(parsed.force_hotkey().display_name(), "Ctrl+Shift+F12");
        assert!(!parsed.recheck_first_word_after_erasing);
        assert!(parsed.physical_fallback_for_unsupported_apps);
        assert!(parsed.single_letter_words);
        assert!(parsed.manual_terminal_uia_fallback);
        assert!(parsed.diagnostics_enabled);
        assert!(!parsed.suppress_after_backspace);
        assert!(!parsed.suppress_after_delete);
        assert!(!parsed.suppress_after_left);
        assert!(!parsed.suppress_after_manual_layout_change);
        assert!(!parsed.language_enabled(crate::Language::Estonian));
        assert_eq!(Settings::from_text(&parsed.to_text()), parsed);
    }
}
