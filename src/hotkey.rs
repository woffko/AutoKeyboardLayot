//! Platform-neutral representation of the contextual force/undo hotkey.

use std::fmt;

pub const HOTKEY_MOD_CONTROL: u8 = 1 << 0;
pub const HOTKEY_MOD_ALT: u8 = 1 << 1;
pub const HOTKEY_MOD_SHIFT: u8 = 1 << 2;
pub const HOTKEY_MOD_WIN: u8 = 1 << 3;
pub const HOTKEY_MOD_MASK: u8 =
    HOTKEY_MOD_CONTROL | HOTKEY_MOD_ALT | HOTKEY_MOD_SHIFT | HOTKEY_MOD_WIN;

const VK_CANCEL: u16 = 0x03;
const VK_PAUSE: u16 = 0x13;
const VK_INSERT: u16 = 0x2d;
const VK_F1: u16 = 0x70;
const VK_F24: u16 = 0x87;
const VK_SCROLL: u16 = 0x91;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub virtual_key: u16,
    pub modifiers: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyError(pub String);

impl fmt::Display for HotkeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for HotkeyError {}

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            virtual_key: VK_PAUSE,
            modifiers: 0,
        }
    }
}

impl Hotkey {
    pub fn new(mut virtual_key: u16, mut modifiers: u8) -> Result<Self, HotkeyError> {
        // Windows reports Ctrl+Pause as VK_CANCEL (Break). Keep one canonical
        // configuration instead of an unreachable standalone "Break" choice.
        if virtual_key == VK_CANCEL {
            virtual_key = VK_PAUSE;
            modifiers |= HOTKEY_MOD_CONTROL;
        }
        if modifiers & !HOTKEY_MOD_MASK != 0 {
            return Err(HotkeyError("unknown hotkey modifier bits".to_owned()));
        }
        let ordinary_key = (u16::from(b'0')..=u16::from(b'9')).contains(&virtual_key)
            || (u16::from(b'A')..=u16::from(b'Z')).contains(&virtual_key);
        let supported_key = matches!(virtual_key, VK_CANCEL | VK_PAUSE | VK_INSERT | VK_SCROLL)
            || (VK_F1..=VK_F24).contains(&virtual_key)
            || ordinary_key;
        if !supported_key {
            return Err(HotkeyError("unsupported hotkey key".to_owned()));
        }
        if ordinary_key && modifiers == 0 {
            return Err(HotkeyError(
                "letter and number hotkeys require at least one modifier".to_owned(),
            ));
        }
        Ok(Self {
            virtual_key,
            modifiers,
        })
    }

    pub fn from_parts(key: &str, modifiers: &str) -> Result<Self, HotkeyError> {
        let virtual_key = parse_key(key)
            .ok_or_else(|| HotkeyError(format!("unknown hotkey key: {}", key.trim())))?;
        let modifiers = parse_modifiers(modifiers)?;
        Self::new(virtual_key, modifiers)
    }

    pub fn key_name(self) -> String {
        match self.virtual_key {
            VK_CANCEL => "Break".to_owned(),
            VK_PAUSE => "Pause/Break".to_owned(),
            VK_INSERT => "Insert".to_owned(),
            VK_SCROLL => "Scroll Lock".to_owned(),
            key if (VK_F1..=VK_F24).contains(&key) => format!("F{}", key - VK_F1 + 1),
            key if (u16::from(b'A')..=u16::from(b'Z')).contains(&key)
                || (u16::from(b'0')..=u16::from(b'9')).contains(&key) =>
            {
                char::from_u32(u32::from(key)).unwrap_or('?').to_string()
            }
            _ => "Pause/Break".to_owned(),
        }
    }

    pub fn modifiers_text(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers & HOTKEY_MOD_CONTROL != 0 {
            parts.push("Ctrl");
        }
        if self.modifiers & HOTKEY_MOD_ALT != 0 {
            parts.push("Alt");
        }
        if self.modifiers & HOTKEY_MOD_SHIFT != 0 {
            parts.push("Shift");
        }
        if self.modifiers & HOTKEY_MOD_WIN != 0 {
            parts.push("Win");
        }
        parts.join("+")
    }

    pub fn display_name(self) -> String {
        let modifiers = self.modifiers_text();
        if modifiers.is_empty() {
            self.key_name()
        } else {
            format!("{modifiers}+{}", self.key_name())
        }
    }

    pub const fn matches(self, virtual_key: u16, modifiers: u8) -> bool {
        self.matches_key(virtual_key) && self.modifiers == modifiers
    }

    pub const fn matches_key(self, virtual_key: u16) -> bool {
        self.virtual_key == virtual_key
            || (self.virtual_key == VK_PAUSE
                && virtual_key == VK_CANCEL
                && self.modifiers & HOTKEY_MOD_CONTROL != 0)
    }

    pub fn selectable_keys() -> Vec<(String, u16)> {
        let mut keys = vec![
            ("Pause/Break".to_owned(), VK_PAUSE),
            ("Insert".to_owned(), VK_INSERT),
            ("Scroll Lock".to_owned(), VK_SCROLL),
        ];
        keys.extend((VK_F1..=VK_F24).map(|key| (format!("F{}", key - VK_F1 + 1), key)));
        keys.extend((b'A'..=b'Z').map(|key| ((key as char).to_string(), u16::from(key))));
        keys.extend((b'0'..=b'9').map(|key| ((key as char).to_string(), u16::from(key))));
        keys
    }
}

fn parse_key(key: &str) -> Option<u16> {
    let key = key.trim();
    if key.eq_ignore_ascii_case("pause") || key.eq_ignore_ascii_case("pause/break") {
        return Some(VK_PAUSE);
    }
    if key.eq_ignore_ascii_case("break") {
        return Some(VK_CANCEL);
    }
    if key.eq_ignore_ascii_case("insert") {
        return Some(VK_INSERT);
    }
    if key.eq_ignore_ascii_case("scroll") || key.eq_ignore_ascii_case("scroll lock") {
        return Some(VK_SCROLL);
    }
    let upper = key.to_ascii_uppercase();
    if let Some(number) = upper
        .strip_prefix('F')
        .and_then(|value| value.parse::<u16>().ok())
        && (1..=24).contains(&number)
    {
        return Some(VK_F1 + number - 1);
    }
    let bytes = upper.as_bytes();
    match bytes {
        [key] if key.is_ascii_uppercase() || key.is_ascii_digit() => Some(u16::from(*key)),
        _ => None,
    }
}

fn parse_modifiers(modifiers: &str) -> Result<u8, HotkeyError> {
    let mut result = 0;
    for modifier in modifiers
        .split(['+', ',', ' '])
        .map(str::trim)
        .filter(|modifier| !modifier.is_empty())
    {
        let bit = if modifier.eq_ignore_ascii_case("ctrl")
            || modifier.eq_ignore_ascii_case("control")
        {
            HOTKEY_MOD_CONTROL
        } else if modifier.eq_ignore_ascii_case("alt") {
            HOTKEY_MOD_ALT
        } else if modifier.eq_ignore_ascii_case("shift") {
            HOTKEY_MOD_SHIFT
        } else if modifier.eq_ignore_ascii_case("win") || modifier.eq_ignore_ascii_case("windows") {
            HOTKEY_MOD_WIN
        } else {
            return Err(HotkeyError(format!("unknown hotkey modifier: {modifier}")));
        };
        if result & bit != 0 {
            return Err(HotkeyError(format!(
                "duplicate hotkey modifier: {modifier}"
            )));
        }
        result |= bit;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_contextual_pause_break() {
        let hotkey = Hotkey::default();
        assert_eq!(hotkey.display_name(), "Pause/Break");
        assert!(hotkey.matches(VK_PAUSE, 0));
    }

    #[test]
    fn parses_and_formats_modifier_combinations() {
        let hotkey = Hotkey::from_parts("F12", "Ctrl+Shift").unwrap();
        assert_eq!(hotkey.virtual_key, 0x7b);
        assert_eq!(hotkey.display_name(), "Ctrl+Shift+F12");
        assert_eq!(
            Hotkey::new(hotkey.virtual_key, hotkey.modifiers).unwrap(),
            hotkey
        );
    }

    #[test]
    fn ordinary_keys_require_a_modifier() {
        assert!(Hotkey::from_parts("K", "").is_err());
        assert!(Hotkey::from_parts("K", "Ctrl+Alt").is_ok());
        assert!(Hotkey::from_parts("Escape", "Ctrl").is_err());
    }

    #[test]
    fn malformed_key_names_return_errors_without_panicking() {
        for key in ["", " ", "F0", "F25", "\u{0436}"] {
            assert!(Hotkey::from_parts(key, "Ctrl").is_err());
        }
    }

    #[test]
    fn ctrl_pause_accepts_windows_break_code_and_legacy_break_is_canonical() {
        let hotkey = Hotkey::new(VK_PAUSE, HOTKEY_MOD_CONTROL).unwrap();
        assert!(hotkey.matches(VK_CANCEL, HOTKEY_MOD_CONTROL));
        assert!(!Hotkey::default().matches(VK_CANCEL, HOTKEY_MOD_CONTROL));
        assert_eq!(Hotkey::from_parts("Break", "").unwrap(), hotkey);
    }
}
