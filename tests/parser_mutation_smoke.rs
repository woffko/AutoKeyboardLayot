//! Deterministic mutation smoke test of the parsers that read data from outside the process.
//!
//! Each of the 13 parsers gets inputs derived from in-repo seeds by the deterministic mutations of
//! `tests/support/mutation_harness.rs`. No input may make a parser panic, hang or take long, every
//! seed must be accepted by its parser, and a signed catalog or package that still verifies after
//! a mutation must say exactly what its seed says. A failure reports the target, the iteration and
//! the input in hex, and running the test again reproduces it.
//!
//! The release catalog and the language package seeds are public release assets (the signed
//! catalog `catalog.aklc` and the smallest package `zh-CN-r1.aklp` of release `lang-r5-20260919`).

#[path = "support/mutation_harness.rs"]
mod harness;

use std::{
    collections::BTreeMap,
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
const REPOSITORY: &str = "woffko/AutoKeyboardLayot";
/// One input that takes longer than this means a hang or a blow-up in a parser.
const SLOWEST_INPUT: Duration = Duration::from_secs(5);

const CATALOG: &[u8] = include_bytes!("fixtures/mutation-seed-catalog.aklc");
const PACKAGE: &[u8] = include_bytes!("fixtures/mutation-seed-package.aklp");
const LAYOUT_MODEL: &[u8] = include_bytes!("../data/layout-model/en-ru-et.aklm");

use harness::{Target, failure, mutate, rng_for, run_target, summary};

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
            // Words exactly at the 64-character limit once lowercased (İ lowercases to two
            // characters): one more İ, a snippet, takes a word past the limit only after
            // lowercasing.
            "dotted_config",
            format!(
                "schema_version=4\n[packages]\nmode=legacy\n[user_dictionary]\nen-US: {}\n[word_exclusions]\nru-RU: {}{}\n",
                "İ".repeat(32),
                "a".repeat(60),
                "İ".repeat(2)
            )
            .into_bytes(),
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
        (
            "process_exclusions",
            "private.exe\nKeePass\n# comment\n\nC:\\Program Files\\Signal\\Signal.exe\n"
                .as_bytes()
                .to_vec(),
        ),
    ])
}

/// The body of the `[settings]` section of a configuration: what `Settings` reads.
fn settings_section(config: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(config).unwrap();
    let (_, body) = text.split_once("[settings]\n")?;
    let end = body.find("\n[").map_or(body.len(), |end| end + 1);
    Some(body.as_bytes()[..end].to_vec())
}

/// What a verified catalog says.
fn catalog_content(data: &[u8], trust: &PackageTrust) -> Option<String> {
    VerifiedReleaseCatalog::verify(data, trust, REPOSITORY, None, CATALOG_NOW)
        .ok()
        .map(|catalog| format!("{catalog:?}"))
}

/// What a verified package says, without the hash and the size of the bytes it came in.
fn package_content(data: &[u8], trust: &PackageTrust) -> Option<String> {
    let package = VerifiedLanguagePackage::verify(data, trust).ok()?;
    Some(format!(
        "{:?}",
        (
            package.id(),
            package.revision(),
            package.runtime_api(),
            package.ui_locale(),
            package.ui(),
            package.license(),
            package.notice(),
            package.input(),
        )
    ))
}

fn targets<'a>(trust: &'a PackageTrust) -> Vec<Target<'a>> {
    let text = text_seeds();
    let configs = vec![
        text["full_config"].clone(),
        text["default_config"].clone(),
        text["managed_config"].clone(),
        text["dotted_config"].clone(),
    ];
    let settings = configs
        .iter()
        .filter_map(|config| settings_section(config))
        .collect();
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
        include_bytes!("../data/package-locales/et.json").to_vec(),
        include_bytes!("../data/package-locales/ru.json").to_vec(),
        include_bytes!("../data/package-locales/ja.json").to_vec(),
    ];
    let seed_catalog = catalog_content(CATALOG, trust);
    let seed_package = package_content(PACKAGE, trust);
    let all = usize::MAX;
    vec![
        Target {
            name: "ConfigurationDocument::from_text",
            seeds: configs,
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                ConfigurationDocument::from_text(&String::from_utf8_lossy(data)).is_ok()
            }),
        },
        Target {
            name: "Settings::from_text",
            seeds: settings,
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data);
                let _ = Settings::from_text(&text);
                Settings::try_from_text(&text).is_ok()
            }),
        },
        Target {
            name: "UserLexicon::from_lines",
            seeds: vec![text["lines"].clone()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data);
                UserLexicon::from_lines(text.lines()) != UserLexicon::default()
            }),
        },
        Target {
            name: "BackendRules::from_lines",
            seeds: vec![text["backend_rules"].clone()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data);
                BackendRules::from_lines(text.lines()).is_ok()
            }),
        },
        Target {
            name: "ExclusionPolicy::from_lines",
            seeds: vec![text["process_exclusions"].clone()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data);
                !ExclusionPolicy::from_lines(text.lines()).is_empty()
            }),
        },
        Target {
            name: "Hotkey::from_parts",
            // "key|modifiers"; without a `|` the text is split in the middle.
            seeds: vec![b"Pause/Break|".to_vec(), b"F12|Ctrl+Shift".to_vec()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let text = String::from_utf8_lossy(data).into_owned();
                let middle = text
                    .char_indices()
                    .map(|(index, _)| index)
                    .nth(text.chars().count() / 2)
                    .unwrap_or(0);
                let (key, modifiers) = text
                    .split_once('|')
                    .unwrap_or_else(|| text.split_at(middle));
                let _ = Hotkey::from_parts(&text, &text);
                Hotkey::from_parts(key, modifiers).is_ok()
            }),
        },
        Target {
            name: "ScoringModel::from_json",
            seeds: scoring,
            iterations: 20_000,
            window: all,
            run: Box::new(|data| ScoringModel::from_json(data).is_ok()),
        },
        Target {
            name: "InputPackDescriptor::from_json",
            seeds: inputs,
            iterations: 20_000,
            window: all,
            run: Box::new(|data| InputPackDescriptor::from_json(data).is_ok()),
        },
        Target {
            name: "CatalogRegistry::add",
            seeds: locales,
            iterations: 4_000,
            window: all,
            run: Box::new(|data| CatalogRegistry::default().add(data).is_ok()),
        },
        Target {
            name: "RequestDecoder::accept",
            seeds: vec![text["protocol"].clone()],
            iterations: 20_000,
            window: all,
            run: Box::new(|data| {
                let mut decoder = RequestDecoder::new(SESSION.into()).unwrap();
                decoder.accept(data).is_ok()
            }),
        },
        // A 1.3 MB seed: mutations stay in the first 128 bytes, where the header and its size
        // checks are, and the iteration count is lower because a valid header means a full parse.
        Target {
            name: "LayoutModel::parse",
            seeds: vec![LAYOUT_MODEL.to_vec()],
            iterations: 500,
            window: 128,
            run: Box::new(|data| LayoutModel::parse(data).is_ok()),
        },
        Target {
            name: "VerifiedReleaseCatalog::verify",
            seeds: vec![CATALOG.to_vec()],
            iterations: 20_000,
            window: all,
            // The catalog is signed: whatever still verifies must say what the seed says.
            run: Box::new(move |data| {
                let content = catalog_content(data, trust);
                assert!(
                    content.is_none() || content == seed_catalog,
                    "a changed catalog verified with other content"
                );
                content.is_some()
            }),
        },
        Target {
            name: "VerifiedLanguagePackage::verify",
            seeds: vec![PACKAGE.to_vec()],
            iterations: 5_000,
            window: all,
            // So is the package.
            run: Box::new(move |data| {
                let content = package_content(data, trust);
                assert!(
                    content.is_none() || content == seed_package,
                    "a changed package verified with other content"
                );
                content.is_some()
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
        println!("{}", summary(&target, &report));
        assert!(
            report.slowest < SLOWEST_INPUT,
            "{}: one input took {:?}; a parser hangs or blows up",
            target.name,
            report.slowest
        );
        failures.extend(failure(&target, &report));
    }
    println!("all targets together: {:.1?}", started.elapsed());
    assert!(
        failures.is_empty(),
        "parsers panicked (so does a signed input that verified with other content):\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_seed_is_accepted_by_its_parser() {
    let trust = PackageTrust::release().unwrap();
    for target in targets(&trust) {
        for (index, seed) in target.seeds.iter().enumerate() {
            assert!(
                (target.run)(seed),
                "{}: seed {index} is refused, so its mutations never get past the first check; \
                 a signed seed must verify inside its validity window",
                target.name
            );
        }
    }
}

#[test]
fn the_generator_is_deterministic_and_a_window_keeps_mutations_in_the_header() {
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
}
