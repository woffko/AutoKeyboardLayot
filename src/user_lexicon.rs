//! Small user-managed dictionary and do-not-convert overlays.

use std::collections::{HashMap, HashSet};

use crate::{Language, PackId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserLexicon {
    words: HashMap<PackId, HashSet<String>>,
}

impl UserLexicon {
    pub fn from_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut lexicon = Self::default();
        for line in lines {
            if let Some((id, word)) = Self::normalize_pack_entry(line) {
                lexicon.insert_pack(id, word);
            }
        }
        lexicon
    }

    pub fn contains(&self, language: Language, word: &str) -> bool {
        self.contains_pack(&language, word)
    }

    pub fn contains_pack(&self, id: &PackId, word: &str) -> bool {
        self.words
            .get(id)
            .is_some_and(|words| words.contains(&word.to_lowercase()))
    }

    pub fn insert(&mut self, language: Language, word: impl AsRef<str>) -> bool {
        self.insert_pack(language, word)
    }

    pub fn insert_pack(&mut self, id: PackId, word: impl AsRef<str>) -> bool {
        let Some(word) = normalize_word(word.as_ref()) else {
            return false;
        };
        self.words.entry(id).or_default().insert(word)
    }

    pub fn len(&self) -> usize {
        self.words.values().map(HashSet::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.words.values().all(HashSet::is_empty)
    }

    pub fn format_entry(language: Language, word: &str) -> Option<String> {
        normalize_word(word).map(|word| format!("{}: {word}\n", language.id()))
    }

    pub fn normalize_entry(line: &str) -> Option<(Language, String)> {
        let (id, word) = Self::normalize_pack_entry(line)?;
        Some((id, word))
    }

    /// Persistent overlays are not filtered by installed or enabled packs.
    pub fn normalize_pack_entry(line: &str) -> Option<(PackId, String)> {
        let (id, word) = parse_entry(line)?;
        Some((id, normalize_word(word)?))
    }

    pub fn format_pack_entry(id: &PackId, word: &str) -> Option<String> {
        let label = id.id();
        normalize_word(word).map(|word| format!("{label}: {word}\n"))
    }
}

fn parse_entry(line: &str) -> Option<(PackId, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (language, word) = if let Some((language, word)) = line.split_once(':') {
        (language.trim(), word.trim())
    } else {
        let mut parts = line.split_whitespace();
        let language = parts.next()?;
        let word = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        (language, word)
    };
    Some((Language::from_id(language)?, word))
}

fn normalize_word(word: &str) -> Option<String> {
    let word = word.trim();
    let character_count = word.chars().count();
    if character_count == 0
        || character_count > 64
        || word
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return None;
    }
    Some(word.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_colon_tab_and_space_separated_entries() {
        let lexicon = UserLexicon::from_lines([
            "en-US: RustDesk",
            "ru\tруддщ",
            "et-EE klaviatuur",
            "ja-JP: こんにちは",
        ]);
        assert!(lexicon.contains(Language::English, "rustdesk"));
        assert!(lexicon.contains(Language::Russian, "РУДДЩ"));
        assert!(lexicon.contains(Language::Estonian, "klaviatuur"));
        assert!(lexicon.contains(Language::Japanese, "こんにちは"));
    }

    #[test]
    fn ignores_comments_invalid_ids_and_multiword_entries() {
        let lexicon = UserLexicon::from_lines([
            "# en: hidden",
            "../xx: unknown",
            "en-US two words",
            "ru-RU:",
        ]);
        assert!(lexicon.is_empty());
    }

    #[test]
    fn formats_an_explicit_exclusion_without_losing_layout_punctuation() {
        assert_eq!(
            UserLexicon::format_entry(Language::English, "HF,JNFTN"),
            Some("en-US: hf,jnftn\n".to_owned())
        );
    }
}
