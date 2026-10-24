//! Shared by `tests/dev_tokens.rs` and `examples/measure_token_false_positives.rs`
//! so the ratchet test and the measuring tool count exactly the same thing.
//!
//! Every token of the corpus is a command, tool or package name typed correctly
//! in the English layout. The question is how often the detector would wrongly
//! rewrite it as the same physical keys read in another layout (Russian or
//! Estonian), first with the dictionary detector alone and then with the layout
//! model switched on. Each token is judged as the first word of a sentence
//! (empty context), which is the least favorable case for the model.

use autokeyboardlayot::{
    Detection, Detector, DetectorConfig, DictionaryRegistry, Language,
    profile_resolver::{KeyboardProfileProbe, resolve_keyboard_profiles},
};
use std::{collections::BTreeMap, sync::Arc};

/// The shipped corpus, see its header for the sources.
pub const CORPUS: &str = include_str!("../fixtures/dev-tokens.txt");
/// The US, Russian and Estonian layouts the corpus is read through.
pub const LAYOUTS: &str = include_str!("../fixtures/layouts-en-ru-et.json");

const MINIMUM_LETTERS: usize = 3;
/// (layout, KLID) of the three layouts a platform with all packs would report.
const LOADED: [(usize, &str); 3] = [
    (0x0409, "00000409"),
    (0x0419, "00000419"),
    (0x0425, "00000425"),
];

/// Lowercase tokens of `MINIMUM_LETTERS` or more letters, one per line, sorted
/// and unique. Blank lines and lines that start with `#` are skipped.
pub fn parse_tokens(text: &str) -> Result<Vec<String>, String> {
    let mut tokens: Vec<String> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = index + 1;
        if line.len() < MINIMUM_LETTERS || !line.bytes().all(|byte| byte.is_ascii_lowercase()) {
            return Err(format!(
                "line {number}: {line:?} is not a token of {MINIMUM_LETTERS} or more lowercase letters"
            ));
        }
        if tokens
            .last()
            .is_some_and(|previous| previous.as_str() >= line)
        {
            return Err(format!(
                "line {number}: {line:?} is out of order or repeated"
            ));
        }
        tokens.push(line.to_owned());
    }
    Ok(tokens)
}

/// A physical key: scan code and whether Shift is held.
type Key = (String, bool);

/// One layout, read in both directions.
struct Layout {
    /// The key that prints a character (the lowest scan code wins).
    key_of: BTreeMap<char, Key>,
    /// The character a key prints.
    character_of: BTreeMap<Key, char>,
}

impl Layout {
    fn parse(layouts: &[serde_json::Value], language_id: &str) -> Result<Self, String> {
        let layout = layouts
            .iter()
            .find(|layout| layout["language_id"] == language_id)
            .ok_or_else(|| format!("layout {language_id} is missing"))?;
        let keys = layout["keys"].as_object().ok_or("a layout has no keys")?;
        let (mut key_of, mut character_of) = (BTreeMap::new(), BTreeMap::new());
        for (code, key) in keys {
            for (shift, field) in [(false, "normal"), (true, "shift")] {
                if let Some(character) = key[field].as_str().and_then(|s| s.chars().next()) {
                    key_of.entry(character).or_insert((code.clone(), shift));
                    character_of.insert((code.clone(), shift), character);
                }
            }
        }
        Ok(Self {
            key_of,
            character_of,
        })
    }

    fn read(&self, keys: &[&Key]) -> Option<String> {
        keys.iter()
            .map(|key| self.character_of.get(*key).copied())
            .collect()
    }
}

/// What the three layouts print for each physical key.
pub struct Layouts {
    english: Layout,
    russian: Layout,
    estonian: Layout,
}

impl Layouts {
    pub fn parse(text: &str) -> Result<Self, String> {
        let document: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let layouts = document["layouts"].as_array().ok_or("no layouts array")?;
        Ok(Self {
            english: Layout::parse(layouts, "0409")?,
            russian: Layout::parse(layouts, "0419")?,
            estonian: Layout::parse(layouts, "0425")?,
        })
    }

    /// The token's physical keys read as Russian and as Estonian, or `None`
    /// when one of the layouts has no character for some key.
    pub fn readings(&self, token: &str) -> Option<Vec<(Language, String)>> {
        let keys: Vec<&Key> = token
            .chars()
            .map(|character| self.english.key_of.get(&character))
            .collect::<Option<_>>()?;
        Some(vec![
            (Language::Russian, self.russian.read(&keys)?),
            (Language::Estonian, self.estonian.read(&keys)?),
        ])
    }
}

/// A platform that reports the three layouts as loaded, with English active.
struct LoadedLayouts(usize);

impl KeyboardProfileProbe for LoadedLayouts {
    fn loaded_layouts(&mut self) -> Option<Vec<usize>> {
        Some(LOADED.iter().map(|(layout, _)| *layout).collect())
    }
    fn current_layout(&mut self) -> Option<usize> {
        Some(self.0)
    }
    fn is_ime(&mut self, _: usize) -> bool {
        false
    }
    fn activate_layout(&mut self, layout: usize) -> Option<usize> {
        Some(std::mem::replace(&mut self.0, layout))
    }
    fn current_layout_name(&mut self) -> Option<[u16; 9]> {
        let (_, name) = LOADED.iter().find(|(layout, _)| *layout == self.0)?;
        let mut result = [0; 9];
        for (slot, unit) in result.iter_mut().zip(name.encode_utf16()) {
            *slot = unit;
        }
        Some(result)
    }
}

/// The detector as shipped without and with the layout model, both bound to
/// `dictionaries` and to a platform that has all three layouts loaded.
pub fn detectors(dictionaries: Arc<DictionaryRegistry>) -> Result<(Detector, Detector), String> {
    let mut detector = Detector::with_registry(
        DetectorConfig {
            single_letter_words: true,
            ..Default::default()
        },
        dictionaries,
    );
    let profiles = resolve_keyboard_profiles(&mut LoadedLayouts(0x0409))
        .map_err(|error| format!("keyboard profiles: {error:?}"))?;
    detector.set_resolved_profiles(Some(&profiles));
    let mut with_model = detector.clone();
    with_model.set_layout_model(autokeyboardlayot::layout_model::embedded());
    if !with_model.layout_model_enabled() {
        return Err("the embedded layout model did not parse".into());
    }
    Ok((detector, with_model))
}

/// One token the detector would rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    pub token: String,
    pub target: Language,
    pub replacement: String,
}

impl Conversion {
    fn new(token: &str, detection: &Detection) -> Self {
        Self {
            token: token.to_owned(),
            target: detection.target_language,
            replacement: detection.replacement.clone(),
        }
    }
}

impl std::fmt::Display for Conversion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} -> {} {}",
            self.token,
            self.target.id(),
            self.replacement
        )
    }
}

#[derive(Debug, Default)]
pub struct Measurement {
    /// Tokens that could be read in every layout.
    pub tokens: usize,
    /// Tokens the dictionary detector alone would convert.
    pub dictionary: Vec<Conversion>,
    /// Tokens converted only once the layout model is switched on.
    pub model_extra: Vec<Conversion>,
}

pub fn measure(
    tokens: &[String],
    layouts: &Layouts,
    dictionary_only: &Detector,
    with_model: &Detector,
) -> Measurement {
    let mut result = Measurement::default();
    for token in tokens {
        let Some(mapped) = layouts.readings(token) else {
            continue;
        };
        result.tokens += 1;
        if let Some(detection) =
            dictionary_only.detect_mapped_candidates(token, Language::English, &mapped)
        {
            result.dictionary.push(Conversion::new(token, &detection));
        } else if let Some(detection) =
            with_model.detect_mapped_candidates_in_context(token, Language::English, &mapped, &[])
        {
            result.model_extra.push(Conversion::new(token, &detection));
        }
    }
    result
}
