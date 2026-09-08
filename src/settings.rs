//! Persistent, non-secret user preferences.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub automatic_conversion_on_startup: bool,
    pub pause_break_undo: bool,
    pub offer_word_exclusion_after_undo: bool,
    pub offer_dictionary_after_forced_conversion: bool,
    pub force_hotkey_virtual_key: u16,
    pub force_hotkey_modifiers: u8,
    pub recheck_first_word_after_erasing: bool,
    pub physical_fallback_for_unsupported_apps: bool,
    pub diagnostics_enabled: bool,
    pub suppress_after_backspace: bool,
    pub suppress_after_delete: bool,
    pub suppress_after_left: bool,
    pub suppress_after_right: bool,
    pub suppress_after_up: bool,
    pub suppress_after_down: bool,
    pub suppress_after_home_end: bool,
    pub suppress_after_manual_layout_change: bool,
    pub enable_english: bool,
    pub enable_russian: bool,
    pub enable_estonian: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let hotkey = crate::Hotkey::default();
        Self {
            automatic_conversion_on_startup: false,
            pause_break_undo: true,
            offer_word_exclusion_after_undo: true,
            offer_dictionary_after_forced_conversion: true,
            force_hotkey_virtual_key: hotkey.virtual_key,
            force_hotkey_modifiers: hotkey.modifiers,
            recheck_first_word_after_erasing: true,
            physical_fallback_for_unsupported_apps: true,
            diagnostics_enabled: false,
            suppress_after_backspace: true,
            suppress_after_delete: true,
            suppress_after_left: true,
            suppress_after_right: true,
            suppress_after_up: true,
            suppress_after_down: true,
            suppress_after_home_end: true,
            suppress_after_manual_layout_change: true,
            enable_english: true,
            enable_russian: true,
            enable_estonian: true,
        }
    }
}

impl Settings {
    pub fn from_text(text: &str) -> Self {
        let mut settings = Self::default();
        let mut hotkey_key = None;
        let mut hotkey_modifiers = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key == "force_hotkey_key" {
                hotkey_key = Some(value.trim().to_owned());
                continue;
            }
            if key == "force_hotkey_modifiers" {
                hotkey_modifiers = Some(value.trim().to_owned());
                continue;
            }
            let Some(value) = parse_bool(value) else {
                continue;
            };
            match key {
                "automatic_conversion_on_startup" => {
                    settings.automatic_conversion_on_startup = value;
                }
                "pause_break_undo" => settings.pause_break_undo = value,
                "offer_word_exclusion_after_undo" => {
                    settings.offer_word_exclusion_after_undo = value;
                }
                "offer_dictionary_after_forced_conversion" => {
                    settings.offer_dictionary_after_forced_conversion = value;
                }
                "recheck_first_word_after_erasing" => {
                    settings.recheck_first_word_after_erasing = value;
                }
                "physical_fallback_for_unsupported_apps" => {
                    settings.physical_fallback_for_unsupported_apps = value;
                }
                "diagnostics_enabled" => settings.diagnostics_enabled = value,
                "suppress_after_backspace" => settings.suppress_after_backspace = value,
                "suppress_after_delete" => settings.suppress_after_delete = value,
                "suppress_after_left" => settings.suppress_after_left = value,
                "suppress_after_right" => settings.suppress_after_right = value,
                "suppress_after_up" => settings.suppress_after_up = value,
                "suppress_after_down" => settings.suppress_after_down = value,
                "suppress_after_home_end" => settings.suppress_after_home_end = value,
                "suppress_after_manual_layout_change" => {
                    settings.suppress_after_manual_layout_change = value;
                }
                "enable_english" => settings.enable_english = value,
                "enable_russian" => settings.enable_russian = value,
                "enable_estonian" => settings.enable_estonian = value,
                _ => {}
            }
        }
        if hotkey_key.is_some() || hotkey_modifiers.is_some() {
            let current = settings.force_hotkey();
            let key = hotkey_key.unwrap_or_else(|| current.key_name());
            let modifiers = hotkey_modifiers.unwrap_or_else(|| current.modifiers_text());
            if let Ok(hotkey) = crate::Hotkey::from_parts(&key, &modifiers) {
                settings.force_hotkey_virtual_key = hotkey.virtual_key;
                settings.force_hotkey_modifiers = hotkey.modifiers;
            }
        }
        settings
    }

    pub fn to_text(self) -> String {
        format!(
            "# AutoKeyboardLayot settings v1\n\
             automatic_conversion_on_startup={}\n\
             pause_break_undo={}\n\
             offer_word_exclusion_after_undo={}\n\
             offer_dictionary_after_forced_conversion={}\n\
             force_hotkey_key={}\n\
             force_hotkey_modifiers={}\n\
             recheck_first_word_after_erasing={}\n\
             physical_fallback_for_unsupported_apps={}\n\
             diagnostics_enabled={}\n\
             suppress_after_backspace={}\n\
             suppress_after_delete={}\n\
             suppress_after_left={}\n\
             suppress_after_right={}\n\
             suppress_after_up={}\n\
             suppress_after_down={}\n\
             suppress_after_home_end={}\n\
             suppress_after_manual_layout_change={}\n\
             enable_english={}\n\
             enable_russian={}\n\
             enable_estonian={}\n",
            self.automatic_conversion_on_startup,
            self.pause_break_undo,
            self.offer_word_exclusion_after_undo,
            self.offer_dictionary_after_forced_conversion,
            self.force_hotkey().key_name(),
            self.force_hotkey().modifiers_text(),
            self.recheck_first_word_after_erasing,
            self.physical_fallback_for_unsupported_apps,
            self.diagnostics_enabled,
            self.suppress_after_backspace,
            self.suppress_after_delete,
            self.suppress_after_left,
            self.suppress_after_right,
            self.suppress_after_up,
            self.suppress_after_down,
            self.suppress_after_home_end,
            self.suppress_after_manual_layout_change,
            self.enable_english,
            self.enable_russian,
            self.enable_estonian,
        )
    }

    pub const fn language_enabled(self, language: crate::Language) -> bool {
        match language {
            crate::Language::English => self.enable_english,
            crate::Language::Russian => self.enable_russian,
            crate::Language::Estonian => self.enable_estonian,
            crate::Language::Japanese => false,
        }
    }

    pub fn force_hotkey(self) -> crate::Hotkey {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_remain_fail_closed_for_startup_conversion() {
        let settings = Settings::default();
        assert!(!settings.automatic_conversion_on_startup);
        assert!(settings.pause_break_undo);
        assert!(settings.offer_word_exclusion_after_undo);
        assert!(settings.offer_dictionary_after_forced_conversion);
        assert_eq!(settings.force_hotkey().display_name(), "Pause/Break");
        assert!(settings.recheck_first_word_after_erasing);
        assert!(settings.physical_fallback_for_unsupported_apps);
        assert!(!settings.diagnostics_enabled);
        assert!(settings.suppress_after_backspace);
        assert!(settings.suppress_after_delete);
        assert!(settings.suppress_after_left);
        assert!(settings.suppress_after_right);
        assert!(settings.suppress_after_up);
        assert!(settings.suppress_after_down);
        assert!(settings.suppress_after_home_end);
        assert!(settings.suppress_after_manual_layout_change);
        assert!(settings.enable_english);
        assert!(settings.enable_russian);
        assert!(settings.enable_estonian);
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
        assert!(parsed.diagnostics_enabled);
        assert!(!parsed.suppress_after_backspace);
        assert!(!parsed.suppress_after_delete);
        assert!(!parsed.suppress_after_left);
        assert!(!parsed.suppress_after_manual_layout_change);
        assert!(!parsed.enable_estonian);
        assert_eq!(Settings::from_text(&parsed.to_text()), parsed);
    }
}
