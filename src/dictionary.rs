//! Embedded, read-only dictionaries built from language-pack data.

use std::sync::OnceLock;

use fst::Set;

use crate::Language;

static ENGLISH: OnceLock<Set<&'static [u8]>> = OnceLock::new();
static RUSSIAN: OnceLock<Set<&'static [u8]>> = OnceLock::new();
static ESTONIAN: OnceLock<Set<&'static [u8]>> = OnceLock::new();
static ENGLISH_SHORT: OnceLock<Set<&'static [u8]>> = OnceLock::new();
static RUSSIAN_SHORT: OnceLock<Set<&'static [u8]>> = OnceLock::new();
static ESTONIAN_SHORT: OnceLock<Set<&'static [u8]>> = OnceLock::new();

const ENGLISH_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/en-US.fst"));
const RUSSIAN_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU.fst"));
const ESTONIAN_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/et-EE.fst"));

/// A small, reviewed common-word tier. A broad spelling dictionary can contain
/// abbreviations such as `yt` and must not veto the much more likely Russian `не`.
pub(crate) fn common_short_contains(language: Language, word: &str) -> bool {
    let (cache, bytes): (&OnceLock<Set<&'static [u8]>>, &'static [u8]) = match language {
        Language::English => (
            &ENGLISH_SHORT,
            include_bytes!(concat!(env!("OUT_DIR"), "/en-US-short.fst")),
        ),
        Language::Russian => (
            &RUSSIAN_SHORT,
            include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU-short.fst")),
        ),
        Language::Estonian => (
            &ESTONIAN_SHORT,
            include_bytes!(concat!(env!("OUT_DIR"), "/et-EE-short.fst")),
        ),
        Language::Japanese => return false,
    };
    cache
        .get_or_init(|| Set::new(bytes).expect("generated short-word FST is valid"))
        .contains(word.to_lowercase())
}

pub(crate) fn contains(language: Language, word: &str) -> bool {
    let normalized = word.to_lowercase();
    dictionary(language).is_some_and(|dictionary| dictionary.contains(normalized))
}

fn dictionary(language: Language) -> Option<&'static Set<&'static [u8]>> {
    match language {
        Language::English => Some(
            ENGLISH
                .get_or_init(|| Set::new(ENGLISH_BYTES).expect("generated English FST is valid")),
        ),
        Language::Russian => Some(
            RUSSIAN
                .get_or_init(|| Set::new(RUSSIAN_BYTES).expect("generated Russian FST is valid")),
        ),
        Language::Estonian => Some(
            ESTONIAN
                .get_or_init(|| Set::new(ESTONIAN_BYTES).expect("generated Estonian FST is valid")),
        ),
        Language::Japanese => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_pack_dictionaries_contain_real_words() {
        for (language, words) in [
            (Language::English, &["hello", "keyboard"][..]),
            (
                Language::Russian,
                &["сейчас", "переключение", "следующее"][..],
            ),
            (Language::Estonian, &["eesti", "klaviatuur"][..]),
        ] {
            for word in words {
                assert!(
                    contains(language, word),
                    "{word} is absent from {language:?}"
                );
            }
        }
    }
}
