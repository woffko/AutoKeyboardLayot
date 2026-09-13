//! Immutable dictionary snapshots. Construction belongs outside input processing.

use crate::Language;
use fst::Set;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, OnceLock},
};

pub(crate) const MAX_DICTIONARY_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_SHORT_DICTIONARY_BYTES: usize = 16 * 1024 * 1024;
const MAX_DICTIONARY_ROWS: usize = 1_500_000;
const MAX_SHORT_DICTIONARY_ROWS: usize = 500_000;

/// Persistent case-insensitive identity, independent of installed data.
/// This is neither an OS keyboard profile nor a UI locale preference.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackId {
    bytes: [u8; 63],
    len: u8,
}

impl PackId {
    pub fn parse(value: &str) -> Result<Self, DictionaryError> {
        if value.len() > 63 || value.is_empty() {
            return Err(DictionaryError::InvalidId);
        }
        let mut parts = value.split('-');
        let first = parts.next().ok_or(DictionaryError::InvalidId)?;
        if !(2..=8).contains(&first.len())
            || !first.bytes().all(|b| b.is_ascii_alphabetic())
            || parts.any(|part| {
                part.is_empty()
                    || part.len() > 8
                    || !part.bytes().all(|b| b.is_ascii_alphanumeric())
            })
        {
            return Err(DictionaryError::InvalidId);
        }
        Ok(Self::from_valid_ascii(value))
    }
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("PackId is constructed only from validated ASCII")
    }

    /// Internal constant construction; public callers must use `parse`.
    pub(crate) const fn from_valid_ascii(value: &str) -> Self {
        assert!(!value.is_empty() && value.len() <= 63);
        let mut bytes = [0; 63];
        let mut index = 0;
        while index < value.len() {
            let byte = value.as_bytes()[index];
            assert!(byte.is_ascii_alphanumeric() || byte == b'-');
            bytes[index] = byte.to_ascii_lowercase();
            index += 1;
        }
        Self {
            bytes,
            len: value.len() as u8,
        }
    }
}

impl fmt::Display for PackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for PackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PackId").field(&self.as_str()).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictionaryError {
    InvalidId,
    InvalidWord,
    LimitExceeded,
    DuplicateId,
    InvalidDictionary,
}

impl fmt::Display for DictionaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dictionary_registry_{self:?}")
    }
}
impl std::error::Error for DictionaryError {}

/// Data only: possessing a dictionary does not grant input-adapter readiness.
#[derive(Debug)]
pub struct DictionaryPack {
    id: PackId,
    model: Option<Arc<crate::ScoringModel>>,
    input_descriptor: Option<Arc<crate::InputPackDescriptor>>,
    words: Set<Cow<'static, [u8]>>,
    common_short: Set<Cow<'static, [u8]>>,
}

impl DictionaryPack {
    /// Compile bounded word lists to owned FSTs outside the input worker.
    /// Raw external FSTs are deliberately not accepted by this API. Word data
    /// never supplies an input descriptor, even for a built-in pack identity.
    pub fn from_words<'a>(
        id: PackId,
        words: impl IntoIterator<Item = &'a str>,
        common_short: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, DictionaryError> {
        Ok(Self {
            id,
            model: crate::scoring_model::embedded_model(&id),
            input_descriptor: None,
            words: compile_words(words, MAX_DICTIONARY_ROWS, MAX_DICTIONARY_BYTES)?,
            common_short: compile_words(
                common_short,
                MAX_SHORT_DICTIONARY_ROWS,
                MAX_SHORT_DICTIONARY_BYTES,
            )?,
        })
    }
    pub fn id(&self) -> &PackId {
        &self.id
    }
    /// Scoring data never grants input-adapter readiness.
    pub fn with_scoring_model(mut self, model: crate::ScoringModel) -> Self {
        self.model = Some(Arc::new(model));
        self
    }
    pub fn scoring_model(&self) -> Option<&crate::ScoringModel> {
        self.model.as_deref()
    }
    /// Install requirements for this identity, not an adapter or readiness grant.
    pub fn with_input_descriptor(
        mut self,
        descriptor: crate::InputPackDescriptor,
    ) -> Result<Self, crate::InputDescriptorError> {
        if descriptor.pack_id() != self.id {
            return Err(crate::InputDescriptorError::PackIdMismatch);
        }
        self.input_descriptor = Some(Arc::new(descriptor));
        Ok(self)
    }
    pub fn input_descriptor(&self) -> Option<&crate::InputPackDescriptor> {
        self.input_descriptor.as_deref()
    }
    pub fn contains(&self, word: &str) -> bool {
        self.words.contains(word.to_lowercase())
    }
    pub fn common_short_contains(&self, word: &str) -> bool {
        self.common_short.contains(word.to_lowercase())
    }
}

fn compile_words<'a>(
    words: impl IntoIterator<Item = &'a str>,
    max_rows: usize,
    max_bytes: usize,
) -> Result<Set<Cow<'static, [u8]>>, DictionaryError> {
    let mut normalized = BTreeSet::new();
    let mut bytes = 0usize;
    let mut normalized_bytes = 0usize;
    for (index, word) in words.into_iter().enumerate() {
        // Count all input, including duplicates, before normalization.
        if index >= max_rows || word.len() > 256 {
            return Err(DictionaryError::LimitExceeded);
        }
        bytes += word.len();
        if bytes > max_bytes {
            return Err(DictionaryError::LimitExceeded);
        }
        // Dictionary storage is independent of the current-input-word limit.
        // Real dictionaries contain long compounds; the 256-byte bound above
        // limits their storage without discarding valid source vocabulary.
        // InputSession continues to enforce its own 64-character privacy limit.
        if word.is_empty() || word.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(DictionaryError::InvalidWord);
        }
        let lower = word.to_lowercase();
        // Unicode lowercasing can expand UTF-8. Bound both representations,
        // including duplicates, before retaining the normalized word.
        normalized_bytes += lower.len();
        if normalized_bytes > max_bytes {
            return Err(DictionaryError::LimitExceeded);
        }
        normalized.insert(lower);
    }
    let set = Set::from_iter(normalized).map_err(|_| DictionaryError::InvalidDictionary)?;
    Set::new(Cow::Owned(set.into_fst().into_inner()))
        .map_err(|_| DictionaryError::InvalidDictionary)
}

/// Cheaply cloned snapshot. Missing selections are retained, not treated as ready.
/// Clones share immutable dictionary storage, but their enabled sets are independent.
#[derive(Debug, Clone, Default)]
pub struct DictionaryRegistry {
    packs: BTreeMap<PackId, Arc<DictionaryPack>>,
    enabled: BTreeSet<PackId>,
}

impl DictionaryRegistry {
    pub fn installed_ids(&self) -> impl Iterator<Item = &PackId> {
        self.packs.keys()
    }

    /// Inspect installed metadata without enabling input participation.
    pub fn installed(&self, id: &PackId) -> Option<&Arc<DictionaryPack>> {
        self.packs.get(id)
    }

    pub fn insert(&mut self, pack: DictionaryPack) -> Result<(), DictionaryError> {
        self.insert_shared(Arc::new(pack))
    }

    pub(crate) fn insert_shared(
        &mut self,
        pack: Arc<DictionaryPack>,
    ) -> Result<(), DictionaryError> {
        if self.packs.contains_key(pack.id()) {
            return Err(DictionaryError::DuplicateId);
        }
        if self.packs.len() >= 64 {
            return Err(DictionaryError::LimitExceeded);
        }
        self.packs.insert(pack.id, pack);
        Ok(())
    }
    /// Installation does not enable a pack. Absent IDs may remain selected for
    /// later installation; `active` still returns None until data is installed.
    pub fn set_enabled(
        &mut self,
        ids: impl IntoIterator<Item = PackId>,
    ) -> Result<(), DictionaryError> {
        let mut enabled = BTreeSet::new();
        for (index, id) in ids.into_iter().enumerate() {
            if index >= 64 {
                return Err(DictionaryError::LimitExceeded);
            }
            enabled.insert(id);
        }
        self.enabled = enabled;
        Ok(())
    }
    pub fn enabled_ids(&self) -> impl Iterator<Item = &PackId> {
        self.enabled.iter()
    }
    pub fn active(&self, id: &PackId) -> Option<&Arc<DictionaryPack>> {
        self.enabled
            .contains(id)
            .then(|| self.packs.get(id))
            .flatten()
    }
    pub fn remove(&mut self, id: &PackId) -> Option<Arc<DictionaryPack>> {
        self.packs.remove(id)
    }
    pub(crate) fn active_language(&self, language: Language) -> Option<&Arc<DictionaryPack>> {
        self.active(&language)
    }
    /// Protected base for managed installations; does not initialize RU/ET data.
    pub fn english_base() -> Self {
        static BASE: OnceLock<DictionaryRegistry> = OnceLock::new();
        BASE.get_or_init(|| {
            let mut registry = Self::default();
            let id = Language::English;
            registry.enabled.insert(id);
            registry
                .insert(DictionaryPack {
                    id,
                    model: crate::scoring_model::embedded_model(&id),
                    input_descriptor: crate::input_profile::embedded_descriptor(&id),
                    words: Set::new(Cow::Borrowed(
                        &include_bytes!(concat!(env!("OUT_DIR"), "/en-US.fst"))[..],
                    ))
                    .expect("generated FST is valid"),
                    common_short: Set::new(Cow::Borrowed(
                        &include_bytes!(concat!(env!("OUT_DIR"), "/en-US-short.fst"))[..],
                    ))
                    .expect("generated short-word FST is valid"),
                })
                .expect("embedded base is valid");
            registry
        })
        .clone()
    }
    /// Transitional EN/RU/ET bootstrap; the final base installer embeds only EN.
    #[cfg(feature = "legacy-bundled-input")]
    pub fn embedded() -> Self {
        static EMBEDDED: OnceLock<DictionaryRegistry> = OnceLock::new();
        EMBEDDED
            .get_or_init(|| {
                let mut registry = Self::english_base();
                for (id, words, common_short) in [
                    (
                        "ru-RU",
                        &include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU.fst"))[..],
                        &include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU-short.fst"))[..],
                    ),
                    (
                        "et-EE",
                        &include_bytes!(concat!(env!("OUT_DIR"), "/et-EE.fst"))[..],
                        &include_bytes!(concat!(env!("OUT_DIR"), "/et-EE-short.fst"))[..],
                    ),
                ] {
                    let id = PackId::parse(id).expect("embedded ID is valid");
                    registry.enabled.insert(id);
                    registry
                        .insert(DictionaryPack {
                            id,
                            model: crate::scoring_model::embedded_model(&id),
                            input_descriptor: crate::input_profile::embedded_descriptor(&id),
                            words: Set::new(Cow::Borrowed(words)).expect("generated FST is valid"),
                            common_short: Set::new(Cow::Borrowed(common_short))
                                .expect("generated short-word FST is valid"),
                        })
                        .expect("embedded registry is valid");
                }
                registry
            })
            .clone()
    }

    #[cfg(not(feature = "legacy-bundled-input"))]
    pub fn embedded() -> Self {
        Self::english_base()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_descriptor_replacement_preserves_prior_snapshot_and_selection() {
        let id = Language::English;
        let us = crate::WindowsKeyboardProfile::parse("0409:00000409").unwrap();
        let intl = crate::WindowsKeyboardProfile::parse("0409:00020409").unwrap();
        let old = crate::test_support::registry();
        let mut next = old.clone();
        let original = next.remove(&id).unwrap();
        let descriptor = crate::InputPackDescriptor::from_json(br#"{
            "format":1,"pack_id":"EN-us","windows_keyboard_profiles":[
                {"profile":"0409:00020409","required_capabilities":["physical-key-v1","dead-key-v1"]}
            ]
        }"#).unwrap();
        next.insert(
            DictionaryPack::from_words(id, ["replacement"], [])
                .unwrap()
                .with_input_descriptor(descriptor)
                .unwrap(),
        )
        .unwrap();
        assert!(
            old.active(&id)
                .unwrap()
                .input_descriptor()
                .unwrap()
                .profile(us)
                .is_some()
        );
        assert!(
            old.active(&id)
                .unwrap()
                .input_descriptor()
                .unwrap()
                .profile(intl)
                .is_none()
        );
        assert!(
            next.active(&id)
                .unwrap()
                .input_descriptor()
                .unwrap()
                .profile(us)
                .is_none()
        );
        assert!(
            next.active(&id)
                .unwrap()
                .input_descriptor()
                .unwrap()
                .profile(intl)
                .is_some()
        );
        assert!(Arc::ptr_eq(&original, old.active(&id).unwrap()));
        assert!(old.active(&id).unwrap().contains("hello"));
        assert!(!next.active(&id).unwrap().contains("hello"));
        assert!(next.active(&id).unwrap().contains("replacement"));
        assert!(old.enabled_ids().eq(next.enabled_ids()));
    }

    #[test]
    fn descriptor_cannot_be_attached_to_another_pack_identity() {
        let de = PackId::parse("de-DE").unwrap();
        let pack = DictionaryPack::from_words(de, ["hallo"], []).unwrap();
        assert!(pack.input_descriptor().is_none());
        let descriptor =
            crate::InputPackDescriptor::from_json(include_bytes!("../data/input/en-US.json"))
                .unwrap();
        assert!(matches!(
            pack.with_input_descriptor(descriptor),
            Err(crate::InputDescriptorError::PackIdMismatch)
        ));
    }

    #[test]
    fn language_pack_dictionaries_contain_real_words() {
        let registry = crate::test_support::registry();
        for (language, words) in [
            (Language::English, &["hello", "keyboard"][..]),
            (
                Language::Russian,
                &["сейчас", "переключение", "следующее"][..],
            ),
            (Language::Estonian, &["eesti", "klaviatuur"][..]),
        ] {
            for word in words {
                assert!(registry.active_language(language).unwrap().contains(word));
            }
        }
    }
    #[test]
    fn stable_ids_are_bounded_case_insensitive_and_not_paths() {
        assert_eq!(PackId::parse("zh-Hans"), PackId::parse("ZH-hans"));
        let longest = "abcdefgh-abcdefgh-abcdefgh-abcdefgh-abcdefgh-abcdefgh-abcdef-ab";
        assert_eq!(longest.len(), 63);
        let id = PackId::parse(longest).unwrap();
        let copied = id;
        assert_eq!(copied.as_str(), longest);
        assert_eq!(
            copied,
            PackId::parse(&longest.to_ascii_uppercase()).unwrap()
        );
        assert!(PackId::parse(&format!("{longest}c")).is_err());
        assert_eq!(
            ["de-DE", "en-US", "de"]
                .map(|s| PackId::parse(s).unwrap())
                .into_iter()
                .collect::<BTreeSet<_>>()
                .iter()
                .map(PackId::as_str)
                .collect::<Vec<_>>(),
            vec!["de", "de-de", "en-us"]
        );
        for bad in [
            "",
            "e",
            " en-US",
            "en-US ",
            "../en",
            "en_US",
            "en--US",
            "en/US",
            "en-ä",
            "123",
            "en-123456789",
        ] {
            assert!(PackId::parse(bad).is_err(), "{bad}");
        }
        assert!(PackId::parse(&"a".repeat(64)).is_err());
    }
    #[test]
    fn owned_registration_is_explicit_and_duplicates_are_atomic() {
        let id = PackId::parse("de-DE").unwrap();
        let mut registry = DictionaryRegistry::default();
        registry
            .insert(DictionaryPack::from_words(id, ["Hallo", "HALLO"], ["Ja"]).unwrap())
            .unwrap();
        assert!(registry.active(&id).is_none());
        registry.set_enabled([id]).unwrap();
        assert!(registry.active(&id).unwrap().contains("HALLO"));
        assert!(registry.active(&id).unwrap().common_short_contains("ja"));
        assert_eq!(
            registry.insert(DictionaryPack::from_words(id, ["other"], []).unwrap()),
            Err(DictionaryError::DuplicateId)
        );
        assert!(!registry.active(&id).unwrap().contains("other"));
    }
    #[test]
    fn missing_removed_and_disabled_selections_survive_snapshot_cloning() {
        let id = PackId::parse("ru-RU").unwrap();
        let absent = PackId::parse("de-DE").unwrap();
        let original = crate::test_support::registry();
        let mut next = original.clone();
        assert!(Arc::ptr_eq(
            original.active(&id).unwrap(),
            next.active(&id).unwrap()
        ));
        next.set_enabled([id, absent]).unwrap();
        assert!(next.active(&absent).is_none());
        next.remove(&id).unwrap();
        assert_eq!(next.enabled_ids().count(), 2);
        assert!(next.active(&id).is_none());
        assert!(original.active(&id).is_some());
        next.set_enabled([]).unwrap();
        assert!(next.active_language(Language::English).is_none());
    }
    #[test]
    fn invalid_words_and_excessive_selections_do_not_mutate_registry() {
        for bad in ["", "two words", "word\n", "\0"] {
            assert!(
                DictionaryPack::from_words(PackId::parse("en-US").unwrap(), [bad], []).is_err()
            );
        }
        let mut registry = crate::test_support::registry();
        let previous = registry.enabled.clone();
        assert_eq!(
            registry.set_enabled(std::iter::repeat_n(PackId::parse("en-US").unwrap(), 65)),
            Err(DictionaryError::LimitExceeded)
        );
        assert_eq!(registry.enabled, previous);
        assert!(
            DictionaryPack::from_words(
                PackId::parse("en-US").unwrap(),
                ["x".repeat(257).as_str()],
                []
            )
            .is_err()
        );
    }

    #[test]
    fn long_dictionary_words_remain_byte_bounded_without_input_readiness() {
        let compound =
            "donaudampfschifffahrtselektrizitätenhauptbetriebswerkbauunterbeamtengesellschaft";
        assert!(compound.chars().count() > 64);
        let ascii_boundary = "a".repeat(256);
        let unicode_boundary = "é".repeat(128);
        let pack = DictionaryPack::from_words(
            PackId::parse("de-DE").unwrap(),
            [compound, ascii_boundary.as_str(), unicode_boundary.as_str()],
            [],
        )
        .unwrap();
        for word in [compound, ascii_boundary.as_str(), unicode_boundary.as_str()] {
            assert!(pack.contains(word));
        }
        assert!(pack.input_descriptor().is_none());
        assert!(pack.scoring_model().is_none());
        for oversized in ["a".repeat(257), "é".repeat(129)] {
            assert!(matches!(
                DictionaryPack::from_words(
                    PackId::parse("de-DE").unwrap(),
                    [oversized.as_str()],
                    []
                ),
                Err(DictionaryError::LimitExceeded)
            ));
        }
    }

    #[test]
    fn dictionary_quota_boundaries_count_duplicates_and_unicode_expansion() {
        assert!(compile_words(["a", "a"], 2, 2).is_ok());
        assert!(matches!(
            compile_words(["a", "a", "a"], 2, 3),
            Err(DictionaryError::LimitExceeded)
        ));
        assert!(matches!(
            compile_words(["aa", "aa"], 2, 3),
            Err(DictionaryError::LimitExceeded)
        ));
        // U+0130 is two input bytes but lowercases to i + combining dot (3).
        assert!(compile_words(["İ"], 1, 3).unwrap().contains("i\u{307}"));
        assert!(matches!(
            compile_words(["İ"], 1, 2),
            Err(DictionaryError::LimitExceeded)
        ));
        let id = PackId::parse("ru-RU").unwrap();
        assert!(
            DictionaryPack::from_words(id, std::iter::repeat_n("word", MAX_DICTIONARY_ROWS), [])
                .is_ok()
        );
        assert!(
            DictionaryPack::from_words(
                id,
                [],
                std::iter::repeat_n("hi", MAX_SHORT_DICTIONARY_ROWS)
            )
            .is_ok()
        );
        assert!(matches!(
            DictionaryPack::from_words(
                id,
                [],
                std::iter::repeat_n("hi", MAX_SHORT_DICTIONARY_ROWS + 1)
            ),
            Err(DictionaryError::LimitExceeded)
        ));
    }

    #[test]
    fn main_and_short_byte_quotas_accept_exact_limit_and_reject_one_more() {
        let word = "a".repeat(64);
        for (rows, bytes) in [
            (MAX_DICTIONARY_ROWS, MAX_DICTIONARY_BYTES),
            (MAX_SHORT_DICTIONARY_ROWS, MAX_SHORT_DICTIONARY_BYTES),
        ] {
            let count = bytes / word.len();
            assert!(count < rows);
            assert!(compile_words(std::iter::repeat_n(word.as_str(), count), rows, bytes).is_ok());
            assert!(matches!(
                compile_words(
                    std::iter::repeat_n(word.as_str(), count).chain(["a"]),
                    rows,
                    bytes
                ),
                Err(DictionaryError::LimitExceeded)
            ));
        }
    }

    #[test]
    fn input_row_and_pack_count_limits_include_duplicates() {
        let id = PackId::parse("en-US").unwrap();
        assert!(matches!(
            DictionaryPack::from_words(
                id,
                std::iter::repeat_n("word", MAX_DICTIONARY_ROWS + 1),
                []
            ),
            Err(DictionaryError::LimitExceeded)
        ));
        let mut registry = DictionaryRegistry::default();
        for index in 0..64 {
            registry
                .insert(
                    DictionaryPack::from_words(
                        PackId::parse(&format!("en-x{index}")).unwrap(),
                        ["word"],
                        [],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        assert_eq!(
            registry.insert(
                DictionaryPack::from_words(PackId::parse("en-extra").unwrap(), ["word"], [])
                    .unwrap()
            ),
            Err(DictionaryError::LimitExceeded)
        );
        assert_eq!(registry.packs.len(), 64);
    }
}
