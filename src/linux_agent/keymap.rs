//! The keyboard layouts the engine types in, compiled by libxkbcommon. IBus hands the engine the
//! physical key (an evdev code) and the modifier state; this module says what that key gives in the
//! layout of a language, and replays the keys of a word in every other layout to produce the texts
//! the detector chooses between (the counterpart of `ToUnicodeEx` in the Windows agent).

use autokeyboardlayot::Language;
use xkbcommon::xkb;

/// libxkbcommon keycodes are evdev codes plus eight.
const EVDEV_OFFSET: u32 = 8;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_CAPSLOCK: u32 = 58;
pub const KEY_RIGHTALT: u32 = 100;
const KEYSYM_ISO_LEVEL3_SHIFT: u32 = 0xfe03;
/// `XK_dead_grave` through `XK_dead_longsolidusoverlay`.
const DEAD_KEYSYMS: std::ops::RangeInclusive<u32> = 0xfe50..=0xfe93;

/// The XKB layout of each language the engine can type in.
pub fn xkb_layout(language: Language) -> Option<&'static str> {
    match language.id() {
        "en-US" => Some("us"),
        "ru-RU" => Some("ru"),
        "et-EE" => Some("ee"),
        _ => None,
    }
}

/// The modifiers that choose a key's level, as they were when the key went down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Levels {
    pub shift: bool,
    pub caps_lock: bool,
    /// AltGr, in layouts whose right Alt key selects the third level.
    pub level3: bool,
}

/// One physical key of a word with the levels it was typed with. A word's keys are kept, never
/// its text, so that the same keys can be read in another layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedKey {
    pub keycode: u32,
    pub levels: Levels,
}

/// What a key gives in a layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyOutput {
    /// One printable character, with the keysym that a compose sequence continues with.
    Text { text: String, keysym: u32 },
    /// A dead key that starts a compose sequence.
    Dead { keysym: u32 },
    /// Anything else: a function, navigation or modifier key, or a key without a symbol.
    Other,
}

#[derive(Debug)]
pub struct KeymapError(pub String);

impl std::fmt::Display for KeymapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for KeymapError {}

/// The compiled layout of every language the engine types in.
pub struct Keymaps {
    context: xkb::Context,
    layouts: Vec<(Language, xkb::Keymap)>,
}

impl Keymaps {
    /// Compiles the layout of each language that has one. Options are left empty on purpose, so
    /// that a group-switching option of the desktop never turns a layout into several groups.
    pub fn new(languages: &[Language]) -> Result<Self, KeymapError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let mut layouts = Vec::with_capacity(languages.len());
        for &language in languages {
            let Some(layout) = xkb_layout(language) else {
                continue;
            };
            let keymap = xkb::Keymap::new_from_names(
                &context,
                "evdev",
                "pc105",
                layout,
                "",
                Some(String::new()),
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
            .ok_or_else(|| KeymapError(format!("the XKB layout '{layout}' cannot be compiled")))?;
            layouts.push((language, keymap));
        }
        Ok(Self { context, layouts })
    }

    pub fn context(&self) -> &xkb::Context {
        &self.context
    }

    /// The languages with a compiled layout, in the order they were given.
    pub fn languages(&self) -> impl Iterator<Item = Language> + '_ {
        self.layouts.iter().map(|(language, _)| *language)
    }

    pub fn supports(&self, language: Language) -> bool {
        self.keymap(language).is_some()
    }

    fn keymap(&self, language: Language) -> Option<&xkb::Keymap> {
        self.layouts
            .iter()
            .find(|(candidate, _)| *candidate == language)
            .map(|(_, keymap)| keymap)
    }

    /// Whether the right Alt key of this layout selects the third level (AltGr) rather than Alt.
    pub fn right_alt_is_level3(&self, language: Language) -> bool {
        self.keymap(language).is_some_and(|keymap| {
            let state = xkb::State::new(keymap);
            state
                .key_get_one_sym(xkb::Keycode::new(KEY_RIGHTALT + EVDEV_OFFSET))
                .raw()
                == KEYSYM_ISO_LEVEL3_SHIFT
        })
    }

    /// What `key` gives in the layout of `language`.
    pub fn key(&self, language: Language, key: TypedKey) -> KeyOutput {
        let Some(keymap) = self.keymap(language) else {
            return KeyOutput::Other;
        };
        let mut state = xkb::State::new(keymap);
        let press = |state: &mut xkb::State, code: u32| {
            let code = xkb::Keycode::new(code + EVDEV_OFFSET);
            state.update_key(code, xkb::KeyDirection::Down);
            code
        };
        if key.levels.shift {
            press(&mut state, KEY_LEFTSHIFT);
        }
        if key.levels.caps_lock {
            let code = press(&mut state, KEY_CAPSLOCK);
            state.update_key(code, xkb::KeyDirection::Up);
        }
        if key.levels.level3 {
            press(&mut state, KEY_RIGHTALT);
        }
        let code = xkb::Keycode::new(key.keycode + EVDEV_OFFSET);
        let keysym = state.key_get_one_sym(code).raw();
        if DEAD_KEYSYMS.contains(&keysym) {
            return KeyOutput::Dead { keysym };
        }
        let text = state.key_get_utf8(code);
        let mut characters = text.chars();
        match (characters.next(), characters.next()) {
            (Some(character), None) if !character.is_control() => KeyOutput::Text { text, keysym },
            _ => KeyOutput::Other,
        }
    }

    /// The word that `keys` give in the layout of `language`, when every key gives exactly one
    /// printable character there; a partial reading is never returned.
    pub fn replay(&self, language: Language, keys: &[TypedKey]) -> Option<String> {
        if keys.is_empty() || !self.supports(language) {
            return None;
        }
        let mut word = String::with_capacity(keys.len() * 2);
        for &key in keys {
            match self.key(language, key) {
                KeyOutput::Text { text, .. } => word.push_str(&text),
                KeyOutput::Dead { .. } | KeyOutput::Other => return None,
            }
        }
        Some(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_G: u32 = 34;
    const KEY_H: u32 = 35;
    const KEY_B: u32 = 48;
    const KEY_D: u32 = 32;
    const KEY_T: u32 = 20;
    const KEY_N: u32 = 49;
    const KEY_S: u32 = 31;
    const KEY_1: u32 = 2;
    const KEY_LEFTBRACE: u32 = 26;
    const KEY_GRAVE: u32 = 41;
    const KEY_ESC: u32 = 1;
    const KEY_LEFT: u32 = 105;

    fn keymaps() -> Keymaps {
        Keymaps::new(&[Language::English, Language::Russian, Language::Estonian])
            .expect("the system XKB data compiles us, ru and ee")
    }

    fn plain(keycode: u32) -> TypedKey {
        TypedKey {
            keycode,
            levels: Levels::default(),
        }
    }

    fn text(output: KeyOutput) -> Option<String> {
        match output {
            KeyOutput::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    #[test]
    fn a_word_typed_in_one_layout_reads_in_the_others() {
        let keymaps = keymaps();
        let word: Vec<TypedKey> = [KEY_G, KEY_H, KEY_B, KEY_D, KEY_T, KEY_N]
            .map(plain)
            .to_vec();
        assert_eq!(
            keymaps.replay(Language::English, &word).as_deref(),
            Some("ghbdtn")
        );
        assert_eq!(
            keymaps.replay(Language::Russian, &word).as_deref(),
            Some("привет")
        );
        assert_eq!(
            keymaps.replay(Language::Estonian, &word).as_deref(),
            Some("ghbdtn")
        );
        let bracket = [plain(KEY_LEFTBRACE)];
        assert_eq!(
            keymaps.replay(Language::English, &bracket).as_deref(),
            Some("[")
        );
        assert_eq!(
            keymaps.replay(Language::Russian, &bracket).as_deref(),
            Some("х")
        );
        assert_eq!(
            keymaps.replay(Language::Estonian, &bracket).as_deref(),
            Some("ü")
        );
        assert_eq!(keymaps.replay(Language::English, &[]), None);
    }

    #[test]
    fn shift_caps_lock_and_altgr_choose_the_level() {
        let keymaps = keymaps();
        let key = |keycode, shift, caps_lock, level3| TypedKey {
            keycode,
            levels: Levels {
                shift,
                caps_lock,
                level3,
            },
        };
        assert_eq!(
            text(keymaps.key(Language::Russian, key(KEY_G, true, false, false))).as_deref(),
            Some("П")
        );
        assert_eq!(
            text(keymaps.key(Language::Russian, key(KEY_G, false, true, false))).as_deref(),
            Some("П")
        );
        // Caps Lock leaves digits alone, Shift does not.
        assert_eq!(
            text(keymaps.key(Language::English, key(KEY_1, false, true, false))).as_deref(),
            Some("1")
        );
        assert_eq!(
            text(keymaps.key(Language::English, key(KEY_1, true, false, false))).as_deref(),
            Some("!")
        );
        // AltGr+s is š in the Estonian layout, whose right Alt is AltGr; in the US layout it is Alt.
        assert!(keymaps.right_alt_is_level3(Language::Estonian));
        assert!(!keymaps.right_alt_is_level3(Language::English));
        assert_eq!(
            text(keymaps.key(Language::Estonian, key(KEY_S, false, false, true))).as_deref(),
            Some("š")
        );
    }

    #[test]
    fn dead_keys_and_keys_without_a_character_are_told_apart() {
        let keymaps = keymaps();
        assert!(matches!(
            keymaps.key(Language::Estonian, plain(KEY_GRAVE)),
            KeyOutput::Dead { .. }
        ));
        assert_eq!(
            text(keymaps.key(Language::Russian, plain(KEY_GRAVE))).as_deref(),
            Some("ё")
        );
        assert_eq!(
            keymaps.key(Language::English, plain(KEY_ESC)),
            KeyOutput::Other
        );
        assert_eq!(
            keymaps.key(Language::English, plain(KEY_LEFT)),
            KeyOutput::Other
        );
        // A word with a dead key in the target layout has no reading there.
        assert_eq!(
            keymaps.replay(Language::Estonian, &[plain(KEY_G), plain(KEY_GRAVE)]),
            None
        );
    }

    #[test]
    fn only_languages_with_a_known_layout_are_compiled() {
        let keymaps = Keymaps::new(&[Language::English]).unwrap();
        assert!(keymaps.supports(Language::English));
        assert!(!keymaps.supports(Language::Russian));
        assert_eq!(
            keymaps.key(Language::Russian, plain(KEY_G)),
            KeyOutput::Other
        );
        assert_eq!(keymaps.languages().collect::<Vec<_>>(), [Language::English]);
    }
}
