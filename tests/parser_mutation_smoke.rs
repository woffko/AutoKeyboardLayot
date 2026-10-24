//! Deterministic mutation smoke test of the parsers that read data from outside the process.
//!
//! Each of the 13 parsers gets inputs derived from in-repo seeds by xorshift-driven mutations (bit
//! flips, replaced, deleted and inserted bytes, truncation, duplicated chunks, hostile snippets and
//! chunks of sibling seeds). No input may make a parser panic, hang or take long. The generator is
//! deterministic: a failure reports the target, the iteration and the input in hex, and running the
//! test again reproduces it.
//!
//! The release catalog and the language package seeds are public release assets (the signed
//! catalog `catalog.aklc` and the smallest package `zh-CN-r1.aklp` of release `lang-r5-20260919`).

use std::{
    collections::BTreeMap,
    panic::{self, AssertUnwindSafe},
    time::{Duration, Instant},
};

use autokeyboardlayot::{
    Hotkey, InputPackDescriptor, ScoringModel, Settings, UserLexicon,
    backend_rules::BackendRules,
    configuration::ConfigurationDocument,
    installer_protocol::RequestDecoder,
    language_package::{PackageTrust, VerifiedLanguagePackage},
    layout_model::LayoutModel,
    localization::CatalogRegistry,
    package_catalog::VerifiedReleaseCatalog,
    privacy::ExclusionPolicy,
};

/// Inside the validity window of the seed catalog (2026-09-19 to 2026-09-26).
const CATALOG_NOW: u64 = 1_790_000_000;
const SESSION: &str = "0123456789abcdef0123456789abcdef";
/// One input that takes longer than this means a hang or a blow-up in a parser.
const SLOWEST_INPUT: Duration = Duration::from_secs(5);

const CATALOG: &[u8] = include_bytes!("fixtures/mutation-seed-catalog.aklc");
const PACKAGE: &[u8] = include_bytes!("fixtures/mutation-seed-package.aklp");
const LAYOUT_MODEL: &[u8] = include_bytes!("../data/layout-model/en-ru-et.aklm");

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

const SNIPPETS: [&str; 14] = [
    "é",
    "я",
    "ü",
    "€",
    "😀",
    "\u{0301}",
    "\u{202e}",
    "{",
    "}",
    "\u{0}",
    "\\",
    "\"",
    "99999999999999999999",
    "-1",
];

/// One to four mutations of `seed`. Positions stay below `window` bytes, which keeps mutations of
/// a large seed (a model file) in its header, where the format checks live.
fn mutate(rng: &mut Rng, seed: &[u8], pool: &[Vec<u8>], window: usize) -> Vec<u8> {
    let mut data = seed.to_vec();
    for _ in 0..1 + rng.below(4) {
        let reach = data.len().min(window);
        match rng.below(9) {
            0 if reach > 0 => {
                let index = rng.below(reach);
                data[index] ^= 1 << rng.below(8);
            }
            1 if reach > 0 => {
                let index = rng.below(reach);
                data[index] = rng.next() as u8;
            }
            2 if reach > 0 => {
                let index = rng.below(reach);
                data.remove(index);
            }
            3 => {
                let index = rng.below(reach + 1);
                data.insert(index, rng.next() as u8);
            }
            4 if reach > 0 => {
                let length = rng.below(reach);
                data.truncate(length);
            }
            5 if reach > 0 => {
                let start = rng.below(reach);
                let end = (start + 1 + rng.below(64)).min(data.len());
                let chunk = data[start..end].to_vec();
                let at = rng.below(reach + 1);
                data.splice(at..at, chunk);
            }
            6 => {
                let snippet = SNIPPETS[rng.below(SNIPPETS.len())];
                let at = rng.below(reach + 1);
                data.splice(at..at, snippet.bytes());
            }
            7 if !pool.is_empty() && reach > 0 => {
                let other = &pool[rng.below(pool.len())];
                if !other.is_empty() {
                    let start = rng.below(other.len());
                    let end = (start + 1 + rng.below(200)).min(other.len());
                    let at = rng.below(reach + 1);
                    data.splice(at..at, other[start..end].to_vec());
                }
            }
            _ => {}
        }
    }
    data
}

/// One parser entry point, fed arbitrary bytes. It must return, never panic.
type ParserCall<'a> = Box<dyn Fn(&[u8]) + 'a>;

struct Target<'a> {
    name: &'static str,
    seeds: Vec<Vec<u8>>,
    iterations: usize,
    /// How far into a seed mutations may reach.
    window: usize,
    run: ParserCall<'a>,
}

#[derive(Debug)]
struct Failure {
    iteration: usize,
    input: Vec<u8>,
}

#[derive(Debug)]
struct Report {
    panics: usize,
    first_failure: Option<Failure>,
    slowest: Duration,
    elapsed: Duration,
}

/// The generator is seeded from the target name only, so every run is the same run.
fn rng_for(name: &str) -> Rng {
    let spread = name.bytes().fold(0u64, |sum, byte| {
        sum.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    Rng(0x9E37_79B9_7F4A_7C15 ^ spread.wrapping_mul(7919))
}

/// Feeds every seed unchanged, then mutated inputs, to the target and counts panics.
fn run_target(target: &Target<'_>) -> Report {
    let started = Instant::now();
    let mut rng = rng_for(target.name);
    let mut report = Report {
        panics: 0,
        first_failure: None,
        slowest: Duration::ZERO,
        elapsed: Duration::ZERO,
    };
    for iteration in 0..target.iterations {
        let seed = &target.seeds[iteration % target.seeds.len()];
        let input = if iteration < target.seeds.len() {
            seed.clone()
        } else {
            mutate(&mut rng, seed, &target.seeds, target.window)
        };
        let begin = Instant::now();
        let result = panic::catch_unwind(AssertUnwindSafe(|| (target.run)(&input)));
        report.slowest = report.slowest.max(begin.elapsed());
        if result.is_err() {
            report.panics += 1;
            if report.first_failure.is_none() {
                report.first_failure = Some(Failure { iteration, input });
            }
        }
    }
    report.elapsed = started.elapsed();
    report
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(300)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn text_seeds() -> BTreeMap<&'static str, Vec<u8>> {
    let full_config = "schema_version=4\n\n[packages]\nmode=managed\n\n[settings]\nui_language=ru\nforce_hotkey_key=Pause/Break\nenabled_input_packs=en-us,ru-ru\nlayout_model=true\n\n[user_dictionary]\nen-US: hello\n\n[word_exclusions]\nru-RU: привет\n\n[process_exclusions]\nprivate.exe\n\n[input_profiles]\nen-US=0409:00000409\n\n[backend_rules]\nnotepad.exe = protected-paste\n";
    BTreeMap::from([
        ("full_config", full_config.as_bytes().to_vec()),
        (
            "default_config",
            ConfigurationDocument::default().to_text().unwrap().into_bytes(),
        ),
        (
            "managed_config",
            ConfigurationDocument::english_only_managed()
                .to_text()
                .unwrap()
                .into_bytes(),
        ),
        (
            "lines",
            "en-US: hello\nru-RU: привет\net-EE: tere\n# comment\n\nde-DE: Hallo\n"
                .as_bytes()
                .to_vec(),
        ),
        (
            "backend_rules",
            "notepad.exe = protected-paste\nfirefox.exe = physical-replay\n* = capability-probe\nC:\\Apps\\x.exe = observe-only\n"
                .as_bytes()
                .to_vec(),
        ),
        (
            "protocol",
            format!(
                r#"{{"format":1,"session":"{SESSION}","sequence":1,"command":{{"action":"select","view":1,"ids":["ru-RU","et-EE"]}}}}"#
            )
            .into_bytes(),
        ),
    ])
}

fn targets<'a>(trust: &'a PackageTrust) -> Vec<Target<'a>> {
    let text = text_seeds();
    let configs = vec![
        text["full_config"].clone(),
        text["default_config"].clone(),
        text["managed_config"].clone(),
    ];
    let scoring = vec![
        include_bytes!("../data/scoring/en-US.json").to_vec(),
        include_bytes!("../data/scoring/ru-RU.json").to_vec(),
        include_bytes!("../data/scoring/et-EE.json").to_vec(),
    ];
    let inputs = vec![
        include_bytes!("../data/input/en-US.json").to_vec(),
        include_bytes!("../data/input/ru-RU.json").to_vec(),
        include_bytes!("../data/input/et-EE.json").to_vec(),
    ];
    let locales = vec![
        include_bytes!("../data/locales/en.json").to_vec(),
        include_bytes!("../data/package-locales/ru.json").to_vec(),
        include_bytes!("../data/package-locales/ja.json").to_vec(),
    ];
    let lines = text["lines"].clone();
    let all = usize::MAX;
    vec![
        Target {
            name: "ConfigurationDocument::from_text",
            seeds: configs.clone(),
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let _ = ConfigurationDocument::from_text(&String::from_utf8_lossy(data));
            }),
        },
        Target {
            name: "Settings::from_text",
            seeds: configs,
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let _ = Settings::from_text(&String::from_utf8_lossy(data));
            }),
        },
        Target {
            name: "UserLexicon::from_lines",
            seeds: vec![lines.clone()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data);
                let _ = UserLexicon::from_lines(text.lines());
            }),
        },
        Target {
            name: "BackendRules::from_lines",
            seeds: vec![text["backend_rules"].clone()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data);
                let _ = BackendRules::from_lines(text.lines());
            }),
        },
        Target {
            name: "ExclusionPolicy::from_lines",
            seeds: vec![lines],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data);
                let _ = ExclusionPolicy::from_lines(text.lines());
            }),
        },
        Target {
            name: "Hotkey::from_parts",
            seeds: vec![b"Pause/Break".to_vec(), b"Ctrl+Shift+F12".to_vec()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data).into_owned();
                let middle = text
                    .char_indices()
                    .map(|(index, _)| index)
                    .nth(text.chars().count() / 2)
                    .unwrap_or(0);
                let (key, modifiers) = text.split_at(middle);
                let _ = Hotkey::from_parts(key, modifiers);
                let _ = Hotkey::from_parts(&text, &text);
            }),
        },
        Target {
            name: "ScoringModel::from_json",
            seeds: scoring,
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let _ = ScoringModel::from_json(data);
            }),
        },
        Target {
            name: "InputPackDescriptor::from_json",
            seeds: inputs,
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let _ = InputPackDescriptor::from_json(data);
            }),
        },
        Target {
            name: "CatalogRegistry::add",
            seeds: locales,
            iterations: 4_000,
            window: all,
            run: Box::new(|data| {
                let mut registry = CatalogRegistry::default();
                let _ = registry.add(data);
            }),
        },
        Target {
            name: "RequestDecoder::accept",
            seeds: vec![text["protocol"].clone()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let mut decoder = RequestDecoder::new(SESSION.into()).unwrap();
                let _ = decoder.accept(data);
            }),
        },
        // A 1.3 MB seed: mutations stay in the first 128 bytes, where the header and its size
        // checks are, and the iteration count is lower because a valid header means a full parse.
        Target {
            name: "LayoutModel::parse",
            seeds: vec![LAYOUT_MODEL.to_vec()],
            iterations: 500,
            window: 128,
            run: Box::new(|data| {
                let _ = LayoutModel::parse(data);
            }),
        },
        Target {
            name: "VerifiedReleaseCatalog::verify",
            seeds: vec![CATALOG.to_vec()],
            iterations: 20_000,
            window: all,
            run: Box::new(move |data| {
                let _ = VerifiedReleaseCatalog::verify(
                    data,
                    trust,
                    "woffko/AutoKeyboardLayot",
                    None,
                    CATALOG_NOW,
                );
            }),
        },
        Target {
            name: "VerifiedLanguagePackage::verify",
            seeds: vec![PACKAGE.to_vec()],
            iterations: 5_000,
            window: all,
            run: Box::new(move |data| {
                let _ = VerifiedLanguagePackage::verify(data, trust);
            }),
        },
    ]
}

#[test]
fn no_parser_panics_on_mutated_input() {
    let trust = PackageTrust::release().unwrap();
    let started = Instant::now();
    let mut failures = Vec::new();
    for target in targets(&trust) {
        let report = run_target(&target);
        println!(
            "{:<34} iterations={:>6} panics={} slowest={:>9.1?} total={:>8.1?}",
            target.name, target.iterations, report.panics, report.slowest, report.elapsed
        );
        assert!(
            report.slowest < SLOWEST_INPUT,
            "{}: one input took {:?}; a parser hangs or blows up",
            target.name,
            report.slowest
        );
        if let Some(failure) = report.first_failure {
            failures.push(format!(
                "{}: {} panics; first at iteration {} with input ({} bytes, hex of the first 300): {}",
                target.name,
                report.panics,
                failure.iteration,
                failure.input.len(),
                hex(&failure.input)
            ));
        }
    }
    println!("all targets together: {:.1?}", started.elapsed());
    assert!(
        failures.is_empty(),
        "parsers panicked:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_seed_is_accepted_by_its_parser() {
    let trust = PackageTrust::release().unwrap();
    let text = text_seeds();
    assert!(
        ConfigurationDocument::from_text(&String::from_utf8_lossy(&text["full_config"])).is_ok()
    );
    assert!(
        ConfigurationDocument::from_text(&String::from_utf8_lossy(&text["managed_config"])).is_ok()
    );
    assert!(LayoutModel::parse(LAYOUT_MODEL).is_ok());
    assert!(
        VerifiedReleaseCatalog::verify(
            CATALOG,
            &trust,
            "woffko/AutoKeyboardLayot",
            None,
            CATALOG_NOW
        )
        .is_ok(),
        "the seed catalog must verify inside its validity window, or the mutations never reach the parser"
    );
    assert!(
        VerifiedLanguagePackage::verify(PACKAGE, &trust).is_ok(),
        "the seed package must verify, or the mutations never reach the parser"
    );
    assert!(ScoringModel::from_json(include_bytes!("../data/scoring/ru-RU.json")).is_ok());
    assert!(InputPackDescriptor::from_json(include_bytes!("../data/input/ru-RU.json")).is_ok());
    assert!(
        CatalogRegistry::default()
            .add(include_bytes!("../data/package-locales/ru.json"))
            .is_ok()
    );
    assert!(
        RequestDecoder::new(SESSION.into())
            .unwrap()
            .accept(&text["protocol"])
            .is_ok()
    );
}

#[test]
fn the_generator_is_deterministic_and_the_harness_reports_a_panicking_parser() {
    let pool = vec![b"abcdefghij".to_vec()];
    let first: Vec<Vec<u8>> = {
        let mut rng = rng_for("determinism");
        (0..50)
            .map(|_| mutate(&mut rng, b"seed text 0123456789", &pool, usize::MAX))
            .collect()
    };
    let second: Vec<Vec<u8>> = {
        let mut rng = rng_for("determinism");
        (0..50)
            .map(|_| mutate(&mut rng, b"seed text 0123456789", &pool, usize::MAX))
            .collect()
    };
    assert_eq!(first, second);
    assert!(first.iter().any(|input| input != b"seed text 0123456789"));
    // A window keeps mutations inside the header of a large seed: four mutations add at most 64
    // bytes each, so everything beyond 16 + 256 bytes is still the original filler.
    let mut rng = rng_for("window");
    for _ in 0..200 {
        let input = mutate(&mut rng, &[7u8; 1000], &[], 16);
        assert!(input.iter().skip(16 + 4 * 64).all(|byte| *byte == 7));
    }

    // The harness itself must notice a parser that panics, and report where.
    let target = Target {
        name: "synthetic panic",
        seeds: vec![b"aaaa".to_vec()],
        iterations: 500,
        window: usize::MAX,
        run: Box::new(|data| {
            assert!(data.iter().all(|byte| *byte >= 0x20), "synthetic failure");
        }),
    };
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let report = run_target(&target);
    panic::set_hook(previous);
    assert!(
        report.panics > 0,
        "no mutated input contained a control byte: {report:?}"
    );
    let failure = report.first_failure.unwrap();
    assert!(failure.input.iter().any(|byte| *byte < 0x20) && failure.iteration >= 1);
}
