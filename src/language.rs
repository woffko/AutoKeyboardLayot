//! Pack identity and direct keyboard-layout mappings.

/// Compatibility name for stable pack identity. An ID does not imply that an
/// input profile, dictionary or adapter is installed or ready.
pub type Language = crate::dictionary_registry::PackId;

/// The canonical spelling of the built-in pack IDs. Which packs take part in
/// conversion is decided at run time from the installed packages and the
/// loaded keyboard layouts (`Detector::automatic_targets`).
const CANONICAL_IDS: [(Language, &str); 4] = [
    (Language::English, "en-US"),
    (Language::Russian, "ru-RU"),
    (Language::Estonian, "et-EE"),
    (Language::Japanese, "ja-JP"),
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

    /// BCP-47 identifier used by settings and diagnostics.
    pub fn id(&self) -> &str {
        CANONICAL_IDS
            .iter()
            .find(|(language, _)| language == self)
            .map_or(self.as_str(), |(_, id)| id)
    }
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
        assert_eq!(Language::from_id("eng"), Some(Language::English));
        assert_eq!(Language::from_id("EN-us"), Some(Language::English));
        assert!(Language::from_id("../de-DE").is_none());
        assert_eq!(std::mem::size_of::<Language>(), 64);
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
    fn built_in_pack_ids_keep_their_canonical_spelling() {
        for (language, canonical) in [
            (Language::English, "en-US"),
            (Language::Russian, "ru-RU"),
            (Language::Estonian, "et-EE"),
            (Language::Japanese, "ja-JP"),
        ] {
            assert_eq!(language.id(), canonical);
            assert_eq!(Language::from_id(canonical), Some(language));
            assert_eq!(language.as_str(), canonical.to_ascii_lowercase());
        }
    }

    /// The README lists the built-in packs by these IDs and no longer
    /// advertises a static metadata table.
    #[test]
    fn the_readme_lists_the_built_in_packs_by_their_canonical_ids() {
        let readme = include_str!("../README.md");
        let section = readme
            .split("\n## Language packs\n")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("README section Language packs");
        for (language, _) in CANONICAL_IDS {
            assert!(
                section.contains(&format!("- `{}`", language.id())),
                "{}",
                language.id()
            );
        }
        assert!(!readme.contains("language_packs()"));
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
