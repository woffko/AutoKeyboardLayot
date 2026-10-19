//! Offline evaluation of the current dictionary detector on generated typing
//! cases. Not installed, published or used by the agent.
//!
//! Usage: evaluate_layout_detection STORE_COPY INPUT.jsonl OUTPUT.jsonl [--model MODEL.aklm]
//!        [--single-letter-words]
//!
//! The detector runs with the program's default settings; --single-letter-words turns on the
//! conversion of single-letter words, which is off by default.
//!
//! STORE_COPY is a copy of an installed package store (never the live one).
//! Each input line is {"id": N, "typed": "en-US", "candidates": {"en-US":
//! "...", "ru-RU": "...", "et-EE": "..."}}: the same physical keys mapped to
//! every layout. Each output line is {"id": N, "target": "ru-RU" | null}.
//! With --model the given layout model is enabled as the second stage and the
//! case's "context" (layouts of previous words) is passed to it.
use autokeyboardlayot::{
    Detector, DetectorConfig, Language, PackId,
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

const USAGE: &str = "usage: evaluate_layout_detection STORE_COPY INPUT.jsonl OUTPUT.jsonl \
                     [--model MODEL.aklm] [--single-letter-words]";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Options {
    store: OsString,
    input: OsString,
    output: OsString,
    model: Option<OsString>,
    single_letter_words: bool,
}

fn options(args: &[OsString]) -> Result<Options, String> {
    let mut positional = Vec::new();
    let mut model = None;
    let mut single_letter_words = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--model" {
            model = Some(rest.next().ok_or(USAGE)?.clone());
        } else if arg == "--single-letter-words" {
            single_letter_words = true;
        } else {
            positional.push(arg.clone());
        }
    }
    let [store, input, output] = <[OsString; 3]>::try_from(positional).map_err(|_| USAGE)?;
    Ok(Options {
        store,
        input,
        output,
        model,
        single_letter_words,
    })
}

/// The program's default detector settings, with single-letter words converted only when asked.
fn detector_config(single_letter_words: bool) -> DetectorConfig {
    DetectorConfig {
        single_letter_words,
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Options {
        store,
        input,
        output,
        model,
        single_letter_words,
    } = options(&args)?;
    let trust = PackageTrust::release()?;
    let snapshot = PackageStore::open(Path::new(&store))?.load(&trust)?;
    let selected: BTreeSet<PackId> = ["en-US", "ru-RU", "et-EE"]
        .into_iter()
        .map(|id| PackId::parse(id).expect("valid ID"))
        .collect();
    let installed = InstalledPackages::from_store(&snapshot, &selected)?;
    let mut detector =
        Detector::with_registry(detector_config(single_letter_words), installed.dictionaries);
    let profiles = resolve_keyboard_profiles(&mut Fixture(0x0409))?;
    detector.set_resolved_profiles(Some(&profiles));
    if let Some(path) = model {
        let model = autokeyboardlayot::layout_model::LayoutModel::parse(&fs::read(path)?)
            .map_err(|error| format!("layout model: {error:?}"))?;
        detector.set_layout_model(Some(std::sync::Arc::new(model)));
    }

    let mut writer = BufWriter::new(fs::File::create(output)?);
    for line in BufReader::new(fs::File::open(input)?).lines() {
        let case: Value = serde_json::from_str(&line?)?;
        let typed = Language::from_id(case["typed"].as_str().ok_or("typed")?)
            .ok_or("unknown typed language")?;
        let candidates = case["candidates"].as_object().ok_or("candidates")?;
        let word = candidates[typed.id()].as_str().ok_or("typed candidate")?;
        let mapped: Vec<(Language, String)> = detector
            .automatic_targets(typed)
            .filter_map(|target| {
                let text = candidates.get(target.id())?.as_str()?;
                Some((target, text.to_owned()))
            })
            .collect();
        let previous: Vec<Language> = case["context"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| Language::from_id(item.as_str()?))
                    .collect()
            })
            .unwrap_or_default();
        let target = detector
            .detect_mapped_candidates_in_context(word, typed, &mapped, &previous)
            .map(|detection| detection.target_language.id().to_owned());
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
        ] {
            assert!(options(&args(wrong)).is_err(), "{wrong:?}");
        }
    }
}
