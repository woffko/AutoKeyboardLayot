//! Offline evaluation of the current dictionary detector on generated typing
//! cases. Not installed, published or used by the agent.
//!
//! Usage: evaluate_layout_detection (STORE_COPY | --bundled) INPUT.jsonl OUTPUT.jsonl
//!        [--model MODEL.aklm] [--single-letter-words] [--flip-same-text] [--sequences]
//!        [--layout-context] [--no-user-switches]
//!
//! The detector runs with the program's default settings; --single-letter-words turns on the
//! conversion of single-letter words, which is off by default.
//!
//! STORE_COPY is a copy of an installed package store (never the live one); with --bundled the
//! dictionaries this build carries are used instead (data/language-packs in a default build),
//! which measures a change of those source lists before any package is republished.
//! Each input line is {"id": N, "typed": "en-US", "candidates": {"en-US":
//! "...", "ru-RU": "...", "et-EE": "..."}}: the same physical keys mapped to
//! every layout. Each output line is {"id": N, "target": "ru-RU" | null}.
//! With --model the given layout model is enabled as the second stage and the
//! case's "context" (layouts of previous words) is passed to it.
//!
//! Decisions are settled as the agent settles them: a word that keeps its text keeps the layout,
//! and one that several layouts read alike goes to the one the field used last (the case's
//! context stands for the field). --flip-same-text leaves the detector's raw target instead,
//! which is what the agent did before.
//!
//! --sequences replays the cases as the typing they were generated from: words follow each other
//! until a case with an empty context, and each is read in the layout the previous decision left.
//! The output is one line of totals. The keys of a word do not depend on the layout, so each case
//! gives the word's text in any of them. The user's own switches come from the cases: a word
//! typed in another layout than the previous word's label; --no-user-switches leaves the layout
//! to the agent alone. --layout-context gives the model the layouts of the previous words
//! instead of the languages they read in.
use autokeyboardlayot::{
    Detection, Detector, DetectorConfig, DictionaryRegistry, Language, PackId,
    installed_packages::InstalledPackages,
    language_package::PackageTrust,
    package_store::PackageStore,
    profile_resolver::{KeyboardProfileProbe, resolve_keyboard_profiles},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
};

const LAYOUTS: [(usize, &str); 3] = [
    (0x0409, "00000409"),
    (0x0419, "00000419"),
    (0x0425, "00000425"),
];

struct Fixture(usize);
impl KeyboardProfileProbe for Fixture {
    fn loaded_layouts(&mut self) -> Option<Vec<usize>> {
        Some(LAYOUTS.iter().map(|(layout, _)| *layout).collect())
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
        let (_, name) = LAYOUTS.iter().find(|(layout, _)| *layout == self.0)?;
        let mut result = [0; 9];
        for (slot, value) in result.iter_mut().zip(name.encode_utf16()) {
            *slot = value;
        }
        Some(result)
    }
}

const USAGE: &str = "usage: evaluate_layout_detection (STORE_COPY | --bundled) INPUT.jsonl \
                     OUTPUT.jsonl [--model MODEL.aklm] [--single-letter-words] \
                     [--flip-same-text] [--sequences] [--layout-context] [--no-user-switches]";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Options {
    /// None: the dictionaries this build carries.
    store: Option<OsString>,
    input: OsString,
    output: OsString,
    model: Option<OsString>,
    single_letter_words: bool,
    flip_same_text: bool,
    sequences: bool,
    layout_context: bool,
    user_switches: bool,
}

fn options(args: &[OsString]) -> Result<Options, String> {
    let mut positional = Vec::new();
    let mut model = None;
    let mut single_letter_words = false;
    let mut bundled = false;
    let mut flip_same_text = false;
    let mut sequences = false;
    let mut layout_context = false;
    let mut user_switches = true;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--model" {
            model = Some(rest.next().ok_or(USAGE)?.clone());
        } else if arg == "--single-letter-words" {
            single_letter_words = true;
        } else if arg == "--bundled" {
            bundled = true;
        } else if arg == "--flip-same-text" {
            flip_same_text = true;
        } else if arg == "--sequences" {
            sequences = true;
        } else if arg == "--layout-context" {
            layout_context = true;
        } else if arg == "--no-user-switches" {
            user_switches = false;
        } else {
            positional.push(arg.clone());
        }
    }
    let (store, input, output) = if bundled {
        let [input, output] = <[OsString; 2]>::try_from(positional).map_err(|_| USAGE)?;
        (None, input, output)
    } else {
        let [store, input, output] = <[OsString; 3]>::try_from(positional).map_err(|_| USAGE)?;
        (Some(store), input, output)
    };
    Ok(Options {
        store,
        input,
        output,
        model,
        single_letter_words,
        flip_same_text,
        sequences,
        layout_context,
        user_switches,
    })
}

/// The program's default detector settings, with single-letter words converted only when asked.
fn detector_config(single_letter_words: bool) -> DetectorConfig {
    DetectorConfig {
        single_letter_words,
        ..Default::default()
    }
}

/// The layouts a case gives the text of, without `typed`.
fn readings(detector: &Detector, typed: Language, case: &Value) -> Vec<(Language, String)> {
    detector
        .automatic_targets(typed)
        .filter_map(|target| {
            let text = case["candidates"].get(target.id())?.as_str()?;
            Some((target, text.to_owned()))
        })
        .collect()
}

/// What the agent does with a detection: the layout it goes to, when it edits at all.
fn settle(
    detection: Option<Detection>,
    readings: &[(Language, String)],
    recent: &[Language],
) -> Option<Language> {
    let mut detection = detection.filter(Detection::changes_text)?;
    detection.prefer_layouts(readings, recent);
    Some(detection.target_language)
}

/// The totals of a replay of typing.
#[derive(Debug, Default, PartialEq, Eq)]
struct Totals {
    words: u64,
    unreadable: u64,
    edits: u64,
    edits_without_text: u64,
    switches: u64,
    latin_switches: u64,
    wrong: u64,
    wrong_needing_a_letter: u64,
}

fn is_latin(language: Language) -> bool {
    language == Language::English || language == Language::Estonian
}

/// Remembers `language` as the newest layout of the field.
fn note_layout(recent: &mut Vec<Language>, language: Language) {
    recent.retain(|&known| known != language);
    recent.insert(0, language);
}

/// Replays `cases` as typing. See the module's documentation.
fn replay(
    detector: &Detector,
    cases: &[Value],
    flip_same_text: bool,
    layout_context: bool,
    user_switches: bool,
) -> Totals {
    let mut totals = Totals::default();
    let mut layout = Language::English;
    let mut label = None;
    let mut previous: Vec<Language> = Vec::new();
    let mut field: Vec<Language> = Vec::new();
    for case in cases {
        let Some(typed) = case["typed"].as_str().and_then(Language::from_id) else {
            continue;
        };
        let starts = case["context"].as_array().is_none_or(Vec::is_empty);
        if starts {
            previous.clear();
            field.clear();
            layout = typed;
        } else if user_switches && label != Some(typed) {
            layout = typed;
        }
        label = case["label"].as_str().and_then(Language::from_id);
        totals.words += 1;
        let Some(word) = case["candidates"][layout.id()].as_str() else {
            totals.unreadable += 1;
            continue;
        };
        let mapped = readings(detector, layout, case);
        note_layout(&mut field, layout);
        let detection =
            detector.detect_mapped_candidates_in_context(word, layout, &mapped, &previous);
        let word_language = detection.as_ref().map(|found| found.target_language);
        let same_text = detection
            .as_ref()
            .is_some_and(|found| !found.changes_text());
        let target = if flip_same_text {
            detection.map(|found| found.target_language)
        } else {
            settle(detection, &mapped, &field)
        };
        let before = layout;
        if let Some(target) = target {
            totals.edits += 1;
            totals.edits_without_text += u64::from(same_text);
            layout = target;
        }
        if layout != before {
            totals.switches += 1;
            totals.latin_switches += u64::from(is_latin(layout) && is_latin(before));
        }
        note_layout(&mut field, layout);
        if previous.len() == 4 {
            previous.remove(0);
        }
        previous.push(if layout_context {
            layout
        } else {
            word_language.unwrap_or(layout)
        });
        let intended = case["intended"].as_str();
        if case["candidates"][layout.id()].as_str() != intended {
            totals.wrong += 1;
            totals.wrong_needing_a_letter +=
                u64::from(case["candidates"]["en-US"] != case["candidates"]["et-EE"]);
        }
    }
    totals
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Options {
        store,
        input,
        output,
        model,
        single_letter_words,
        flip_same_text,
        sequences,
        layout_context,
        user_switches,
    } = options(&args)?;
    let dictionaries = match store {
        Some(store) => {
            let trust = PackageTrust::release()?;
            let snapshot = PackageStore::open(Path::new(&store))?.load(&trust)?;
            let selected: BTreeSet<PackId> = ["en-US", "ru-RU", "et-EE"]
                .into_iter()
                .map(|id| PackId::parse(id).expect("valid ID"))
                .collect();
            InstalledPackages::from_store(&snapshot, &selected)?.dictionaries
        }
        None => std::sync::Arc::new(DictionaryRegistry::embedded()),
    };
    let mut detector = Detector::with_registry(detector_config(single_letter_words), dictionaries);
    let profiles = resolve_keyboard_profiles(&mut Fixture(0x0409))?;
    detector.set_resolved_profiles(Some(&profiles));
    if let Some(path) = model {
        let model = autokeyboardlayot::layout_model::LayoutModel::parse(&fs::read(path)?)
            .map_err(|error| format!("layout model: {error:?}"))?;
        detector.set_layout_model(Some(std::sync::Arc::new(model)));
    }

    let mut writer = BufWriter::new(fs::File::create(output)?);
    if sequences {
        let cases = BufReader::new(fs::File::open(input)?)
            .lines()
            .map(|line| Ok(serde_json::from_str(&line?)?))
            .collect::<Result<Vec<Value>, Box<dyn std::error::Error>>>()?;
        let totals = replay(
            &detector,
            &cases,
            flip_same_text,
            layout_context,
            user_switches,
        );
        writeln!(writer, "{totals:?}")?;
        writer.flush()?;
        return Ok(());
    }
    for line in BufReader::new(fs::File::open(input)?).lines() {
        let case: Value = serde_json::from_str(&line?)?;
        let typed = Language::from_id(case["typed"].as_str().ok_or("typed")?)
            .ok_or("unknown typed language")?;
        let candidates = case["candidates"].as_object().ok_or("candidates")?;
        let word = candidates[typed.id()].as_str().ok_or("typed candidate")?;
        let mapped = readings(&detector, typed, &case);
        let previous: Vec<Language> = case["context"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| Language::from_id(item.as_str()?))
                    .collect()
            })
            .unwrap_or_default();
        let detection =
            detector.detect_mapped_candidates_in_context(word, typed, &mapped, &previous);
        let target = if flip_same_text {
            detection.map(|detection| detection.target_language)
        } else {
            let mut recent = previous;
            recent.reverse();
            settle(detection, &mapped, &recent)
        };
        let target = target.map(|language| language.id().to_owned());
        writeln!(writer, "{}", json!({"id": case["id"], "target": target}))?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn detection(original: &str, replacement: &str, target: Language) -> Detection {
        Detection {
            source_language: Language::Russian,
            target_language: target,
            original: original.to_owned(),
            replacement: replacement.to_owned(),
            source_score: 0.0,
            target_score: 1.0,
        }
    }

    #[test]
    fn a_word_that_keeps_its_text_keeps_its_layout() {
        let kept = detection("selle", "selle", Language::Estonian);
        assert_eq!(settle(Some(kept), &[], &[]), None);
        assert_eq!(settle(None, &[], &[]), None);
    }

    #[test]
    fn a_word_goes_to_the_layout_its_field_used_last() {
        let alike = [
            (Language::English, "just".to_owned()),
            (Language::Estonian, "just".to_owned()),
        ];
        let word = detection("огые", "just", Language::English);
        assert_eq!(
            settle(Some(word.clone()), &alike, &[Language::Estonian]),
            Some(Language::Estonian)
        );
        assert_eq!(settle(Some(word), &alike, &[]), Some(Language::English));
    }

    #[test]
    fn a_field_lists_each_layout_once_newest_first() {
        let mut field = Vec::new();
        for language in [Language::English, Language::Russian, Language::English] {
            note_layout(&mut field, language);
        }
        assert_eq!(field, [Language::English, Language::Russian]);
    }

    #[test]
    fn the_replay_is_asked_for_by_name() {
        let plain = options(&args(&["--bundled", "in.jsonl", "out.jsonl"])).unwrap();
        assert!(!plain.flip_same_text && !plain.sequences && !plain.layout_context);
        assert!(plain.user_switches);
        let asked = options(&args(&[
            "--bundled",
            "--flip-same-text",
            "--sequences",
            "--layout-context",
            "--no-user-switches",
            "in.jsonl",
            "out.jsonl",
        ]))
        .unwrap();
        assert!(asked.flip_same_text && asked.sequences && asked.layout_context);
        assert!(!asked.user_switches);
    }

    #[test]
    fn the_detector_runs_with_the_programs_defaults_unless_asked_otherwise() {
        let plain = options(&args(&["store", "in.jsonl", "out.jsonl"])).unwrap();
        assert!(!plain.single_letter_words);
        assert_eq!(plain.model, None);
        assert_eq!(
            plain.single_letter_words,
            DetectorConfig::default().single_letter_words
        );
        let asked = options(&args(&[
            "--single-letter-words",
            "store",
            "in.jsonl",
            "out.jsonl",
            "--model",
            "model.aklm",
        ]))
        .unwrap();
        assert!(asked.single_letter_words);
        assert_eq!(asked.model, Some(OsString::from("model.aklm")));
        assert_eq!(asked.output, OsString::from("out.jsonl"));
        assert!(!detector_config(plain.single_letter_words).single_letter_words);
        assert!(detector_config(asked.single_letter_words).single_letter_words);
        for wrong in [
            &["store", "in.jsonl"][..],
            &["store", "in.jsonl", "out.jsonl", "--model"],
            &["store", "in.jsonl", "out.jsonl", "extra"],
            &["--bundled", "store", "in.jsonl", "out.jsonl"],
        ] {
            assert!(options(&args(wrong)).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn bundled_takes_the_dictionaries_of_the_build_instead_of_a_store() {
        assert_eq!(
            options(&args(&["store", "in.jsonl", "out.jsonl"]))
                .unwrap()
                .store,
            Some(OsString::from("store"))
        );
        let bundled = options(&args(&["--bundled", "in.jsonl", "out.jsonl"])).unwrap();
        assert_eq!(bundled.store, None);
        assert_eq!(bundled.input, OsString::from("in.jsonl"));
        assert_eq!(bundled.output, OsString::from("out.jsonl"));
    }
}
