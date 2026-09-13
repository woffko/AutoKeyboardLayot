//! Language-pack metadata and direct keyboard-layout mappings.

/// Compatibility name for stable pack identity. An ID does not imply that an
/// input profile, dictionary or adapter is installed or ready.
pub type Language = crate::dictionary_registry::PackId;

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

impl Language {
    #[allow(non_upper_case_globals)]
    pub const English: Self = Self::from_valid_ascii("en-US");
    #[allow(non_upper_case_globals)]
    pub const Russian: Self = Self::from_valid_ascii("ru-RU");
    #[allow(non_upper_case_globals)]
    pub const Estonian: Self = Self::from_valid_ascii("et-EE");
    #[allow(non_upper_case_globals)]
    pub const Japanese: Self = Self::from_valid_ascii("ja-JP");

    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "en" | "en-us" | "eng" => Some(Self::English),
            "ru" | "ru-ru" | "rus" => Some(Self::Russian),
            "et" | "et-ee" | "est" => Some(Self::Estonian),
            "ja" | "ja-jp" | "jpn" => Some(Self::Japanese),
            _ => Self::parse(id.trim()).ok(),
        }
    }

    pub fn pack(self) -> Option<&'static LanguagePack> {
        LANGUAGE_PACKS.iter().find(|pack| pack.language == self)
    }

    /// BCP-47 identifier used by settings and diagnostics.
    pub fn id(&self) -> &str {
        self.pack().map_or(self.as_str(), |pack| pack.id)
    }

    /// Short label suitable for a tooltip or larger tray icon.
    pub fn display_code(&self) -> &str {
        self.pack().map_or(self.as_str(), |pack| pack.display_code)
    }

    /// Compact label that remains legible in a 16 by 16 tray icon.
    pub fn tray_code(self) -> &'static str {
        self.pack().map_or("??", |pack| pack.tray_code)
    }

    /// Legacy catalog metadata only. Runtime code must use
    /// `Detector::automatic_targets`, which validates the active snapshot.
    pub fn automatic_targets(self) -> impl Iterator<Item = Language> {
        self.pack()
            .into_iter()
            .flat_map(|source| automatic_targets_in(source, LANGUAGE_PACKS))
    }
}

/// Capability metadata, not dictionary presence, controls adapter eligibility.
/// Installed/enabled selection is checked separately by the runtime snapshot.
fn automatic_targets_in<'a>(
    source: &'a LanguagePack,
    packs: &'a [LanguagePack],
) -> impl Iterator<Item = Language> + 'a {
    let ready = |pack: &LanguagePack| {
        pack.kind == PackKind::DirectKeyboardLayout
            && pack.automatic_correction == AutomaticCorrection::Enabled
    };
    packs
        .iter()
        .filter(move |target| ready(source) && ready(target) && source.language != target.language)
        .map(|target| target.language)
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

pub(crate) fn transpose_character(character: char, from: Language, to: Language) -> Option<char> {
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
    fn runtime_identity_is_stable_without_a_builtin_descriptor() {
        let id = Language::from_id("DE-de").unwrap();
        assert_eq!(id, crate::PackId::parse("de-DE").unwrap());
        assert_eq!(id.id(), "de-de");
        assert!(id.pack().is_none());
        assert_eq!(id.tray_code(), "??");
        assert_eq!(id.display_code(), "de-de");
        assert!(id.automatic_targets().next().is_none());
        assert_eq!(Language::from_id("eng"), Some(Language::English));
        assert_eq!(Language::from_id("EN-us"), Some(Language::English));
        assert!(Language::from_id("../de-DE").is_none());
        assert_eq!(std::mem::size_of::<Language>(), 64);
    }

    #[test]
    fn automatic_targets_follow_descriptor_capabilities() {
        for source in language_packs() {
            let targets: Vec<_> = source.language.automatic_targets().collect();
            if source.kind == PackKind::ImeRomaji {
                assert!(targets.is_empty());
            } else {
                assert_eq!(targets.len(), 2);
                assert!(!targets.contains(&source.language));
                assert!(!targets.contains(&Language::Japanese));
            }
        }
        let mut packs = language_packs().to_vec();
        packs[1].automatic_correction = AutomaticCorrection::NeedsPhysicalKeyMapper;
        packs[2].dictionary_embedded = false;
        assert_eq!(
            automatic_targets_in(&packs[0], &packs).collect::<Vec<_>>(),
            vec![Language::Estonian]
        );
        assert!(automatic_targets_in(&packs[1], &packs).next().is_none());
        packs[2].kind = PackKind::ImeRomaji;
        assert!(automatic_targets_in(&packs[0], &packs).next().is_none());
    }

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
            Language::Estonian.pack().unwrap().automatic_correction,
            AutomaticCorrection::Enabled
        );
        assert_eq!(Language::Japanese.pack().unwrap().kind, PackKind::ImeRomaji);
        assert_eq!(
            Language::Japanese.pack().unwrap().automatic_correction,
            AutomaticCorrection::NeedsImeAdapter
        );
    }

    #[test]
    fn russian_letter_keys_are_not_forced_to_be_word_boundaries() {
        let detector = crate::test_support::detector();
        for character in [',', '.', '[', ']', ';', '\''] {
            assert!(detector.can_extend_word(character, Language::English));
        }
        assert!(!detector.can_extend_word('/', Language::English));
    }
}
