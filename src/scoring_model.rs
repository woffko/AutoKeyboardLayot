//! Bounded, immutable scoring data. Parsing belongs outside input processing.

use crate::PackId;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
};

/// Statistical/character data only; never an input-adapter capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoringModel {
    ranges: Vec<(char, char)>,
    letters: BTreeSet<char>,
    marks: BTreeSet<char>,
    vowels: BTreeSet<char>,
    bigrams: BTreeSet<String>,
    trigrams: BTreeSet<String>,
    rare: BTreeSet<String>,
    policy: ScoringPolicy,
}

/// Format 2 makes optional linguistic evidence explicit. Disabling a feature
/// removes both its bonus and its penalty; it does not grant input support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoringPolicy {
    bigrams: bool,
    trigrams: bool,
    vowels: bool,
    statistical_targets: bool,
}

impl ScoringPolicy {
    const LEGACY: Self = Self {
        bigrams: true,
        trigrams: true,
        vowels: true,
        statistical_targets: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoringModelError {
    InvalidData,
    LimitExceeded,
}

impl std::fmt::Display for ScoringModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "scoring_model_{self:?}")
    }
}
impl std::error::Error for ScoringModelError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDocument {
    format: u32,
    ranges: Vec<(char, char)>,
    vowels: String,
    bigrams: Vec<String>,
    trigrams: Vec<String>,
    rare: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDocumentV2 {
    format: u32,
    ranges: Vec<(char, char)>,
    vowels: String,
    bigrams: Vec<String>,
    trigrams: Vec<String>,
    rare: Vec<String>,
    policy: ScoringPolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDocumentV3 {
    format: u32,
    ranges: Vec<(char, char)>,
    marks: String,
    vowels: String,
    bigrams: Vec<String>,
    trigrams: Vec<String>,
    rare: Vec<String>,
    policy: ScoringPolicy,
}

/// Sparse repertoires avoid thousands of singleton range pairs. This version
/// does not accept ranges, and keeps all existing file and feature bounds.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDocumentV4 {
    format: u32,
    letters: String,
    marks: String,
    vowels: String,
    bigrams: Vec<String>,
    trigrams: Vec<String>,
    rare: Vec<String>,
    policy: ScoringPolicy,
}

fn is_mark(c: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    matches!(
        get_general_category(c),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

impl ScoringModel {
    /// Parse bounded, lowercase repertoire/scoring data without file access.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ScoringModelError> {
        use ScoringModelError::{InvalidData, LimitExceeded};
        if bytes.len() > 64 * 1024 {
            return Err(LimitExceeded);
        }
        #[derive(Deserialize)]
        struct Header {
            format: u32,
        }
        let header: Header = serde_json::from_slice(bytes).map_err(|_| InvalidData)?;
        // Deserialize the original bytes again, not a Value: duplicate keys
        // must remain visible to the strict version-specific parser.
        let (document, policy, marks, letters) = match header.format {
            1 => (
                serde_json::from_slice::<ModelDocument>(bytes).map_err(|_| InvalidData)?,
                ScoringPolicy::LEGACY,
                String::new(),
                String::new(),
            ),
            2 => {
                let v2: ModelDocumentV2 = serde_json::from_slice(bytes).map_err(|_| InvalidData)?;
                let document = ModelDocument {
                    format: v2.format,
                    ranges: v2.ranges,
                    vowels: v2.vowels,
                    bigrams: v2.bigrams,
                    trigrams: v2.trigrams,
                    rare: v2.rare,
                };
                (document, v2.policy, String::new(), String::new())
            }
            3 => {
                let v3: ModelDocumentV3 = serde_json::from_slice(bytes).map_err(|_| InvalidData)?;
                let document = ModelDocument {
                    format: v3.format,
                    ranges: v3.ranges,
                    vowels: v3.vowels,
                    bigrams: v3.bigrams,
                    trigrams: v3.trigrams,
                    rare: v3.rare,
                };
                (document, v3.policy, v3.marks, String::new())
            }
            4 => {
                let v4: ModelDocumentV4 = serde_json::from_slice(bytes).map_err(|_| InvalidData)?;
                let document = ModelDocument {
                    format: v4.format,
                    ranges: Vec::new(),
                    vowels: v4.vowels,
                    bigrams: v4.bigrams,
                    trigrams: v4.trigrams,
                    rare: v4.rare,
                };
                (document, v4.policy, v4.marks, v4.letters)
            }
            _ => return Err(InvalidData),
        };
        if document.format != header.format || (document.ranges.is_empty() && letters.is_empty()) {
            return Err(InvalidData);
        }
        if header.format >= 2
            && ((!policy.bigrams && !document.bigrams.is_empty())
                || (!policy.trigrams && !document.trigrams.is_empty())
                || (!policy.vowels && !document.vowels.is_empty())
                || (policy.bigrams && document.bigrams.is_empty())
                || (policy.trigrams && document.trigrams.is_empty())
                || (policy.vowels && document.vowels.is_empty())
                || (policy.statistical_targets && !policy.bigrams && !policy.trigrams))
        {
            return Err(InvalidData);
        }
        if document.ranges.len() > 64
            || letters.len() > 32 * 1024
            || marks.len() > 1024
            || document.vowels.len() > 256
            || document.bigrams.len() > 512
            || document.trigrams.len() > 512
            || document.rare.len() > 128
        {
            return Err(LimitExceeded);
        }
        let letter_set: BTreeSet<char> = letters.chars().collect();
        if letter_set.len() != letters.chars().count()
            || letters.chars().any(|c| {
                !c.is_alphabetic() || is_mark(c) || !c.to_lowercase().eq(std::iter::once(c))
            })
        {
            return Err(InvalidData);
        }
        let mark_set: BTreeSet<char> = marks.chars().collect();
        if mark_set.len() != marks.chars().count()
            || marks
                .chars()
                .any(|c| !is_mark(c) || !c.to_lowercase().eq(std::iter::once(c)))
        {
            return Err(InvalidData);
        }
        let mut ranges = document.ranges;
        ranges.sort_unstable();
        let mut total = 0u32;
        let mut previous = None;
        for &(start, end) in &ranges {
            if start > end || previous.is_some_and(|last| start <= last) {
                return Err(InvalidData);
            }
            total += u32::from(end) - u32::from(start) + 1;
            if total > 65536 {
                return Err(LimitExceeded);
            }
            if (u32::from(start)..=u32::from(end))
                .filter_map(char::from_u32)
                .any(|c| {
                    !c.is_alphabetic()
                        || !c.to_lowercase().eq(std::iter::once(c))
                        || (header.format == 3 && is_mark(c))
                })
            {
                return Err(InvalidData);
            }
            previous = Some(end);
        }
        let accepts = |c: char| {
            mark_set.contains(&c)
                || letter_set.contains(&c)
                || ranges
                    .iter()
                    .any(|&(start, end)| (start..=end).contains(&c))
        };
        if document.vowels.chars().any(|c| !accepts(c))
            || document.vowels.to_lowercase() != document.vowels
        {
            return Err(InvalidData);
        }
        for (list, min, max) in [
            (&document.bigrams, 2, 2),
            (&document.trigrams, 3, 3),
            (&document.rare, 2, 8),
        ] {
            let mut seen = BTreeSet::new();
            for value in list {
                if !(min..=max).contains(&value.chars().count())
                    || value.to_lowercase() != *value
                    || value.chars().any(|c| !accepts(c))
                    || !seen.insert(value)
                {
                    return Err(InvalidData);
                }
            }
        }
        let vowels: BTreeSet<_> = document.vowels.chars().collect();
        if vowels.len() != document.vowels.chars().count() {
            return Err(InvalidData);
        }
        Ok(Self {
            ranges,
            letters: letter_set,
            marks: mark_set,
            vowels,
            bigrams: document.bigrams.into_iter().collect(),
            trigrams: document.trigrams.into_iter().collect(),
            rare: document.rare.into_iter().collect(),
            policy,
        })
    }

    /// The caller supplies a lowercased Unicode scalar.
    pub fn accepts(&self, character: char) -> bool {
        self.marks.contains(&character)
            || self.letters.contains(&character)
            || self
                .ranges
                .iter()
                .any(|&(start, end)| (start..=end).contains(&character))
    }
    /// Lowercase scalar repertoire check, not linguistic word segmentation.
    /// Explicit continuation marks cannot start a scoring token. Legacy models
    /// have no explicit marks and retain their existing repertoire semantics.
    pub fn accepts_word(&self, word: &str) -> bool {
        let mut characters = word.chars();
        characters
            .next()
            .is_some_and(|first| self.accepts(first) && !self.marks.contains(&first))
            && characters.all(|c| self.accepts(c))
    }
    pub(crate) fn is_vowel(&self, character: char) -> bool {
        self.vowels.contains(&character)
    }
    pub(crate) fn uses_bigrams(&self) -> bool {
        self.policy.bigrams
    }
    pub(crate) fn uses_trigrams(&self) -> bool {
        self.policy.trigrams
    }
    pub(crate) fn uses_vowels(&self) -> bool {
        self.policy.vowels
    }
    pub(crate) fn allows_statistical_targets(&self) -> bool {
        self.policy.statistical_targets
    }
    pub(crate) fn bigrams(&self) -> &BTreeSet<String> {
        &self.bigrams
    }
    pub(crate) fn trigrams(&self) -> &BTreeSet<String> {
        &self.trigrams
    }
    pub(crate) fn rare(&self) -> impl Iterator<Item = &str> {
        self.rare.iter().map(String::as_str)
    }
}

pub(crate) fn embedded_model(id: &PackId) -> Option<Arc<ScoringModel>> {
    static MODELS: OnceLock<BTreeMap<PackId, Arc<ScoringModel>>> = OnceLock::new();
    MODELS
        .get_or_init(|| {
            [
                ("en-US", &include_bytes!("../data/scoring/en-US.json")[..]),
                #[cfg(feature = "legacy-bundled-input")]
                ("ru-RU", &include_bytes!("../data/scoring/ru-RU.json")[..]),
                #[cfg(feature = "legacy-bundled-input")]
                ("et-EE", &include_bytes!("../data/scoring/et-EE.json")[..]),
            ]
            .into_iter()
            .map(|(id, bytes)| {
                (
                    PackId::parse(id).expect("valid embedded ID"),
                    Arc::new(ScoringModel::from_json(bytes).expect("valid embedded model")),
                )
            })
            .collect()
        })
        .get(id)
        .cloned()
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "legacy-bundled-input"))]
    #[test]
    fn base_build_excludes_optional_models() {
        for id in ["ru-RU", "et-EE"] {
            assert!(super::embedded_model(&crate::PackId::parse(id).unwrap()).is_none());
        }
        assert!(super::embedded_model(&crate::Language::English).is_some());
    }

    use super::*;
    use serde_json::{Value, json};

    fn valid() -> Value {
        json!({"format": 1, "ranges": [["a", "z"]], "vowels": "aeiou",
            "bigrams": ["ab"], "trigrams": ["abc"], "rare": ["zz"]})
    }
    fn parse(value: &Value) -> Result<ScoringModel, ScoringModelError> {
        ScoringModel::from_json(&serde_json::to_vec(value).unwrap())
    }

    fn with_marks() -> Value {
        json!({"format":3,"ranges":[["a","z"],["क","क"],["ক","ক"]],
            "marks":"\u{0301}\u{0345}\u{093e}\u{09be}\u{064e}\u{20dd}",
            "vowels":"","bigrams":["a\u{0301}","क\u{093e}"],"trigrams":[],"rare":[],
            "policy":{"bigrams":true,"trigrams":false,"vowels":false,"statistical_targets":true}})
    }

    #[test]
    fn sparse_letters_are_bounded_and_strictly_versioned() {
        let valid = json!({"format":4,"letters":"a中日本𠀀","marks":"\u{0301}",
            "vowels":"","bigrams":["日本"],"trigrams":[],"rare":[],
            "policy":{"bigrams":true,"trigrams":false,"vowels":false,"statistical_targets":false}});
        let model = parse(&valid).unwrap();
        assert!(model.accepts_word("日本中𠀀a\u{0301}"));
        assert!(!model.accepts_word("\u{0301}日本"));
        assert!(!model.accepts('未'));
        assert!(!model.allows_statistical_targets());
        for letters in [
            "",
            "aa日本",
            "A日本",
            "1日本",
            "\u{0345}日本",
            "\u{0301}日本",
        ] {
            let mut invalid = valid.clone();
            invalid["letters"] = json!(letters);
            assert!(parse(&invalid).is_err(), "{letters:?}");
        }
        for field in [
            "letters", "marks", "policy", "bigrams", "trigrams", "vowels", "rare",
        ] {
            let mut invalid = valid.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(parse(&invalid).is_err(), "missing {field}");
        }
        let mut invalid = valid.clone();
        invalid["ranges"] = json!([]);
        assert!(parse(&invalid).is_err());
        for version in [1, 2, 3] {
            let mut invalid = valid.clone();
            invalid["format"] = json!(version);
            assert!(parse(&invalid).is_err());
        }
        let serialized = serde_json::to_string(&valid).unwrap();
        for key in ["letters", "marks", "format", "policy"] {
            let duplicate = serialized.replacen(
                &format!("\"{key}\":"),
                &format!("\"{key}\":null,\"{key}\":"),
                1,
            );
            assert!(ScoringModel::from_json(duplicate.as_bytes()).is_err());
        }
        let letters: String = "ab"
            .chars()
            .chain((0x4e00..0x4e00 + 10922).map(|n| char::from_u32(n).unwrap()))
            .collect();
        assert_eq!(letters.len(), 32768);
        let mut boundary = valid.clone();
        boundary["letters"] = json!(letters);
        assert!(parse(&boundary).is_ok());
        boundary["letters"] = json!(format!("{letters}c"));
        assert_eq!(parse(&boundary), Err(ScoringModelError::LimitExceeded));
        let mut raw = serde_json::to_vec(&valid).unwrap();
        raw.resize(65536, b' ');
        assert!(ScoringModel::from_json(&raw).is_ok());
        raw.push(b' ');
        assert_eq!(
            ScoringModel::from_json(&raw),
            Err(ScoringModelError::LimitExceeded)
        );
    }

    #[test]
    fn explicit_marks_are_versioned_continuations_not_adapter_grants() {
        let mut document = with_marks();
        let model = parse(&document).unwrap();
        for word in [
            "a\u{0301}",
            "a\u{0345}",
            "का",
            "কা",
            "a\u{064e}",
            "a\u{20dd}",
        ] {
            assert!(model.accepts_word(word), "{word}");
            assert!(!model.is_vowel(word.chars().last().unwrap()));
        }
        for word in ["", "\u{0301}", "\u{093e}क", "a\u{0300}", "a\u{200d}", "a b"] {
            assert!(!model.accepts_word(word), "{word}");
        }
        // A model may explicitly classify a dependent vowel sign as a vowel.
        document["vowels"] = json!("\u{093e}");
        document["policy"]["vowels"] = json!(true);
        assert!(parse(&document).unwrap().is_vowel('\u{093e}'));
        for version in [1, 2] {
            document["format"] = json!(version);
            assert!(parse(&document).is_err());
        }
        // Legacy Alphabetic ranges keep their old behavior, including marks
        // which happen to have the derived Alphabetic property.
        let mut legacy = valid();
        legacy["ranges"] = json!([["a", "z"], ["\u{093e}", "\u{093e}"]]);
        assert!(parse(&legacy).unwrap().accepts_word("\u{093e}a"));
        let mut modern = with_marks();
        modern["ranges"] = legacy["ranges"].clone();
        assert!(parse(&modern).is_err());
        modern = with_marks();
        modern["marks"] = json!("");
        modern["bigrams"] = json!(["ab"]);
        assert!(parse(&modern).unwrap().accepts_word("abc"));
    }

    #[test]
    fn mark_schema_rejects_nonmarks_duplicates_missing_and_oversized_data() {
        for value in [
            json!("a"),
            json!("\u{0301}\u{0301}"),
            json!("\u{200c}"),
            json!("\u{200d}"),
            json!("\n"),
            json!("🦀"),
            json!(["\u{0301}"]),
            Value::Null,
        ] {
            let mut document = with_marks();
            document["marks"] = value;
            assert!(parse(&document).is_err());
        }
        let mut document = with_marks();
        document.as_object_mut().unwrap().remove("marks");
        assert!(parse(&document).is_err());
        document["marks"] = json!("\u{0301}".repeat(513));
        assert_eq!(parse(&document), Err(ScoringModelError::LimitExceeded));
        // Exercise the exact UTF-8 byte limit with distinct valid marks, so
        // success is not obscured by duplicate or repertoire failures.
        let mut document = with_marks();
        document["bigrams"] = json!(["ab"]);
        let mut marks = String::from("\u{0300}\u{0301}");
        for c in (0x800..=0xffff)
            .filter_map(char::from_u32)
            .filter(|&c| is_mark(c))
        {
            if c.to_lowercase().eq(std::iter::once(c)) {
                marks.push(c);
            }
            if marks.len() == 1024 {
                break;
            }
        }
        assert_eq!(marks.len(), 1024);
        document["marks"] = json!(marks);
        assert!(parse(&document).is_ok());
        marks.push('\u{20dd}');
        document["marks"] = json!(marks);
        assert_eq!(parse(&document), Err(ScoringModelError::LimitExceeded));
        let serialized = serde_json::to_string(&with_marks()).unwrap();
        for duplicate in [
            serialized.replacen("\"marks\":", "\"marks\":\"\",\"marks\":", 1),
            serialized.replacen("\"format\":3", "\"format\":3,\"format\":3", 1),
        ] {
            assert_ne!(serialized, duplicate);
            assert!(ScoringModel::from_json(duplicate.as_bytes()).is_err());
        }
        for (field, value) in [
            ("extra", json!(true)),
            ("bigrams", json!(["a\u{0300}"])),
            ("trigrams", json!(["ab\u{0301}"])),
            ("policy", Value::Null),
        ] {
            let mut document = with_marks();
            document[field] = value;
            assert!(parse(&document).is_err());
        }
    }

    #[test]
    fn explicit_policy_is_versioned_and_requires_consistent_evidence() {
        let mut document = valid();
        document["policy"] = json!({"bigrams":true,"trigrams":true,
            "vowels":true,"statistical_targets":true});
        assert!(parse(&document).is_err());
        document["format"] = json!(2);
        assert_eq!(parse(&document).unwrap(), parse(&valid()).unwrap());
        for field in ["bigrams", "trigrams", "vowels", "statistical_targets"] {
            let mut invalid = document.clone();
            invalid["policy"].as_object_mut().unwrap().remove(field);
            assert!(parse(&invalid).is_err(), "missing {field}");
            invalid["policy"][field] = json!("true");
            assert!(parse(&invalid).is_err(), "typed {field}");
        }
        let mut invalid = document.clone();
        invalid["policy"]["extra"] = json!(true);
        assert!(parse(&invalid).is_err());
        let serialized = serde_json::to_string(&document).unwrap();
        for duplicate in [
            serialized.replacen("\"format\":2", "\"format\":2,\"format\":2", 1),
            serialized.replacen("\"bigrams\":true", "\"bigrams\":true,\"bigrams\":true", 1),
        ] {
            assert_ne!(duplicate, serialized);
            assert!(ScoringModel::from_json(duplicate.as_bytes()).is_err());
        }
        for field in ["bigrams", "trigrams", "vowels"] {
            let mut invalid = document.clone();
            invalid["policy"][field] = json!(false);
            assert!(parse(&invalid).is_err(), "{field}");
        }
        document["bigrams"] = json!([]);
        document["trigrams"] = json!([]);
        document["vowels"] = json!("");
        document["policy"] = json!({"bigrams":false,"trigrams":false,
            "vowels":false,"statistical_targets":false});
        let model = parse(&document).unwrap();
        assert!(!model.uses_vowels());
        assert!(!model.uses_bigrams());
        assert!(!model.uses_trigrams());
        assert!(!model.allows_statistical_targets());
        document["policy"]["statistical_targets"] = json!(true);
        assert!(parse(&document).is_err());
        document["policy"]["statistical_targets"] = json!(false);
        for field in ["bigrams", "trigrams", "vowels"] {
            let mut invalid = document.clone();
            invalid["policy"][field] = json!(true);
            assert!(parse(&invalid).is_err(), "empty enabled {field}");
        }
        document["policy"] = Value::Null;
        assert!(parse(&document).is_err());
        document.as_object_mut().unwrap().remove("policy");
        assert!(parse(&document).is_err());
    }

    #[test]
    fn data_is_owned_and_validated_without_an_input_capability() {
        let model = parse(&valid()).unwrap();
        assert!(model.accepts('b'));
        assert!(!model.accepts('@'));
        assert!(model.is_vowel('a'));
        assert!(model.bigrams().contains("ab"));
        assert!(model.trigrams().contains("abc"));
        assert_eq!(model.rare().collect::<Vec<_>>(), vec!["zz"]);
        assert!(embedded_model(&PackId::parse("de-DE").unwrap()).is_none());
        for id in ["en-US", "ru-RU", "et-EE"] {
            assert_eq!(
                embedded_model(&PackId::parse(id).unwrap()).is_some(),
                id == "en-US" || cfg!(feature = "legacy-bundled-input")
            );
        }
    }

    #[test]
    fn rejects_malformed_ranges_sequences_versions_and_fields() {
        for (field, value) in [
            ("format", json!(2)),
            ("extra", json!(true)),
            ("ranges", json!([])),
            ("ranges", json!([["z", "a"]])),
            ("ranges", json!([["a", "m"], ["m", "z"]])),
            ("ranges", json!([["A", "Z"]])),
            ("ranges", json!([["@", "z"]])),
            ("ranges", json!([["\n", "z"]])),
            ("ranges", json!([["aa", "z"]])),
            ("vowels", json!("aa")),
            ("vowels", json!("A")),
            ("vowels", json!("ё")),
            ("bigrams", json!(["ab", "ab"])),
            ("bigrams", json!(["a"])),
            ("bigrams", json!(["Aб"])),
            ("trigrams", json!(["abcd"])),
            ("rare", json!([""])),
            ("rare", json!(["abcdefghi"])),
        ] {
            let mut document = valid();
            document[field] = value;
            assert!(parse(&document).is_err(), "{field}");
        }
        assert!(ScoringModel::from_json(br#"{"format":1,"format":1}"#).is_err());
        assert!(ScoringModel::from_json(b"not json").is_err());
    }

    #[test]
    fn bounds_bytes_and_rows_before_deduplicating() {
        assert_eq!(
            ScoringModel::from_json(&vec![b' '; 65537]),
            Err(ScoringModelError::LimitExceeded)
        );
        for (field, value) in [
            ("ranges", json!(vec![["a", "a"]; 65])),
            ("bigrams", json!(vec!["ab"; 513])),
            ("trigrams", json!(vec!["abc"; 513])),
            ("rare", json!(vec!["ab"; 129])),
            ("vowels", json!("a".repeat(257))),
        ] {
            let mut document = valid();
            document[field] = value;
            assert_eq!(parse(&document), Err(ScoringModelError::LimitExceeded));
        }
    }
}
