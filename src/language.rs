//! Language-pack metadata and direct keyboard-layout mappings.

/// Stable language identity used by detector decisions and Windows routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    English,
    Russian,
    Estonian,
    Japanese,
}

/// Input model required by a language pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    DirectKeyboardLayout,
    ImeRomaji,
}

/// Current automatic-correction capability of a pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomaticCorrection {
    Enabled,
    NeedsPhysicalKeyMapper,
    NeedsImeAdapter,
}

/// Immutable metadata for one embedded language pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguagePack {
    pub language: Language,
    pub id: &'static str,
    pub windows_primary_language_id: u16,
    pub display_code: &'static str,
    pub tray_code: &'static str,
    pub kind: PackKind,
    pub automatic_correction: AutomaticCorrection,
    pub dictionary_embedded: bool,
}

const LANGUAGE_PACKS: &[LanguagePack] = &[
    LanguagePack {
        language: Language::English,
        id: "en-US",
        windows_primary_language_id: 0x09,
        display_code: "ENG",
        tray_code: "EN",
        kind: PackKind::DirectKeyboardLayout,
        automatic_correction: AutomaticCorrection::Enabled,
        dictionary_embedded: true,
    },
    LanguagePack {
        language: Language::Russian,
        id: "ru-RU",
        windows_primary_language_id: 0x19,
        display_code: "RUS",
        tray_code: "RU",
        kind: PackKind::DirectKeyboardLayout,
        automatic_correction: AutomaticCorrection::Enabled,
        dictionary_embedded: true,
    },
    LanguagePack {
        language: Language::Estonian,
        id: "et-EE",
        windows_primary_language_id: 0x25,
        display_code: "EST",
        tray_code: "ET",
        kind: PackKind::DirectKeyboardLayout,
        automatic_correction: AutomaticCorrection::Enabled,
        dictionary_embedded: true,
    },
    LanguagePack {
        language: Language::Japanese,
        id: "ja-JP",
        windows_primary_language_id: 0x11,
        display_code: "JPN",
        tray_code: "JA",
        kind: PackKind::ImeRomaji,
        automatic_correction: AutomaticCorrection::NeedsImeAdapter,
        dictionary_embedded: false,
    },
];

const ENGLISH_TARGETS: &[Language] = &[Language::Russian, Language::Estonian];
const RUSSIAN_TARGETS: &[Language] = &[Language::English, Language::Estonian];
const ESTONIAN_TARGETS: &[Language] = &[Language::English, Language::Russian];
const NO_AUTOMATIC_TARGETS: &[Language] = &[];

impl Language {
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "en" | "en-us" | "eng" => Some(Self::English),
            "ru" | "ru-ru" | "rus" => Some(Self::Russian),
            "et" | "et-ee" | "est" => Some(Self::Estonian),
            "ja" | "ja-jp" | "jpn" => Some(Self::Japanese),
            _ => None,
        }
    }

    pub const fn pack(self) -> &'static LanguagePack {
        match self {
            Self::English => &LANGUAGE_PACKS[0],
            Self::Russian => &LANGUAGE_PACKS[1],
            Self::Estonian => &LANGUAGE_PACKS[2],
            Self::Japanese => &LANGUAGE_PACKS[3],
        }
    }

    /// BCP-47 identifier used by settings and diagnostics.
    pub const fn id(self) -> &'static str {
        self.pack().id
    }

    /// Short label suitable for a tooltip or larger tray icon.
    pub const fn display_code(self) -> &'static str {
        self.pack().display_code
    }

    /// Compact label that remains legible in a 16 by 16 tray icon.
    pub const fn tray_code(self) -> &'static str {
        self.pack().tray_code
    }

    /// Direct-layout targets that may be evaluated for this current language.
    pub const fn automatic_targets(self) -> &'static [Language] {
        match self {
            Self::English => ENGLISH_TARGETS,
            Self::Russian => RUSSIAN_TARGETS,
            Self::Estonian => ESTONIAN_TARGETS,
            Self::Japanese => NO_AUTOMATIC_TARGETS,
        }
    }

    pub(crate) fn accepts_character(self, character: char) -> bool {
        match self {
            Self::English => character.is_ascii_alphabetic(),
            Self::Russian => matches!(character, 'А'..='я' | 'Ё' | 'ё'),
            Self::Estonian => {
                character.is_ascii_alphabetic()
                    || matches!(
                        character,
                        'Õ' | 'õ' | 'Ä' | 'ä' | 'Ö' | 'ö' | 'Ü' | 'ü' | 'Š' | 'š' | 'Ž' | 'ž'
                    )
            }
            Self::Japanese => {
                matches!(character, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}')
            }
        }
    }
}

pub const fn language_packs() -> &'static [LanguagePack] {
    LANGUAGE_PACKS
}

// The strings are ordered by physical key position on the common US/Russian
// keyboard pair. Punctuation is included because those keys produce letters
// in Russian and therefore may occur inside a wrong-layout word.
const ENGLISH_LOWER: &str = "`qwertyuiop[]asdfghjkl;'zxcvbnm,.";
const RUSSIAN_LOWER: &str = "ёйцукенгшщзхъфывапролджэячсмитьбю";
const ENGLISH_UPPER: &str = "~QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>";
const RUSSIAN_UPPER: &str = "ЁЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ";

/// Reinterpret visible text as the same physical keys under another direct
/// keyboard layout.
///
/// Returns `None` when the pair needs an IME, a not-yet-validated physical-key
/// mapper, or any character is outside the known mapping. A partial conversion
/// is never returned.
pub fn transpose_word(word: &str, from: Language, to: Language) -> Option<String> {
    if from == to {
        return Some(word.to_owned());
    }

    debug_assert_eq!(ENGLISH_LOWER.chars().count(), RUSSIAN_LOWER.chars().count());
    debug_assert_eq!(ENGLISH_UPPER.chars().count(), RUSSIAN_UPPER.chars().count());

    word.chars()
        .map(|character| transpose_character(character, from, to))
        .collect()
}

pub(crate) fn can_extend_word(character: char, from: Language) -> bool {
    from.accepts_character(character)
        || from.automatic_targets().iter().copied().any(|target| {
            transpose_character(character, from, target)
                .is_some_and(|mapped| target.accepts_character(mapped))
        })
}

fn transpose_character(character: char, from: Language, to: Language) -> Option<char> {
    let (source_lower, target_lower, source_upper, target_upper) = match (from, to) {
        (Language::English, Language::Russian) => {
            (ENGLISH_LOWER, RUSSIAN_LOWER, ENGLISH_UPPER, RUSSIAN_UPPER)
        }
        (Language::Russian, Language::English) => {
            (RUSSIAN_LOWER, ENGLISH_LOWER, RUSSIAN_UPPER, ENGLISH_UPPER)
        }
        _ => return None,
    };

    find_mapped(character, source_lower, target_lower)
        .or_else(|| find_mapped(character, source_upper, target_upper))
}

fn find_mapped(character: char, source: &str, target: &str) -> Option<char> {
    let index = source
        .chars()
        .position(|candidate| candidate == character)?;
    target.chars().nth(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transposes_common_words_in_both_directions() {
        assert_eq!(
            transpose_word("ghbdtn", Language::English, Language::Russian),
            Some("привет".to_owned())
        );
        assert_eq!(
            transpose_word("руддщ", Language::Russian, Language::English),
            Some("hello".to_owned())
        );
    }

    #[test]
    fn preserves_case() {
        assert_eq!(
            transpose_word("Ghbdtn", Language::English, Language::Russian),
            Some("Привет".to_owned())
        );
        assert_eq!(
            transpose_word("РУДДЩ", Language::Russian, Language::English),
            Some("HELLO".to_owned())
        );
    }

    #[test]
    fn rejects_unknown_or_not_yet_validated_layout_pairs() {
        assert_eq!(
            transpose_word("hello🙂", Language::English, Language::Russian),
            None
        );
        assert_eq!(
            transpose_word("tere", Language::English, Language::Estonian),
            None
        );
        assert_eq!(
            transpose_word("konnichiwa", Language::English, Language::Japanese),
            None
        );
    }

    #[test]
    fn registry_contains_separate_estonian_and_japanese_packs() {
        assert_eq!(language_packs().len(), 4);
        assert_eq!(Language::Estonian.id(), "et-EE");
        assert_eq!(Language::Estonian.tray_code(), "ET");
        assert_eq!(
            Language::Estonian.pack().automatic_correction,
            AutomaticCorrection::Enabled
        );
        assert_eq!(Language::Japanese.pack().kind, PackKind::ImeRomaji);
        assert_eq!(
            Language::Japanese.pack().automatic_correction,
            AutomaticCorrection::NeedsImeAdapter
        );
    }

    #[test]
    fn russian_letter_keys_are_not_forced_to_be_word_boundaries() {
        for character in [',', '.', '[', ']', ';', '\''] {
            assert!(can_extend_word(character, Language::English));
        }
        assert!(!can_extend_word('/', Language::English));
    }
}
