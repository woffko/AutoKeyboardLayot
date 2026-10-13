//! Explicit multilingual fixtures, compiled only into unit-test executables.
//! Production embedded/default behavior remains feature-dependent and EN-only
//! in the modular base. Runtime parsers construct these optional test packs.
use crate::{
    Detector, DetectorConfig, DictionaryPack, DictionaryRegistry, InputPackDescriptor, PackId,
    ScoringModel,
};
use std::{
    io::Read,
    sync::{Arc, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

// Between its clone and its exec, a child process holds copies of every descriptor open in this
// process, including the lock file of a package store writer that another test has just released,
// which then looks busy for a moment. Tests that rely on those locks hold the read side for their
// whole run; tests that spawn a child (the package store's own, the curl transport's) hold the
// write side, so the two never overlap.
static CHILD_SPAWN: RwLock<()> = RwLock::new(());

/// Held by a test that relies on file locks staying its own.
pub fn no_child_spawn() -> RwLockReadGuard<'static, ()> {
    CHILD_SPAWN.read().unwrap_or_else(PoisonError::into_inner)
}

/// Held by a test that spawns a child process.
pub fn spawning_children() -> RwLockWriteGuard<'static, ()> {
    CHILD_SPAWN.write().unwrap_or_else(PoisonError::into_inner)
}

pub fn registry() -> DictionaryRegistry {
    static FIXTURE: OnceLock<DictionaryRegistry> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            let mut registry = DictionaryRegistry::english_base();
            let mut ru = String::new();
            flate2::read::GzDecoder::new(
                &include_bytes!("../data/language-packs/ru-RU/words.txt.gz")[..],
            )
            .read_to_string(&mut ru)
            .unwrap();
            let et: String = include_bytes!("../data/language-packs/et-EE/words.dic")
                .iter()
                .map(|&b| match b {
                    0xa4 => '€',
                    0xa6 => 'Š',
                    0xa8 => 'š',
                    0xb4 => 'Ž',
                    0xb8 => 'ž',
                    0xbc => 'Œ',
                    0xbd => 'œ',
                    0xbe => 'Ÿ',
                    _ => char::from(b),
                })
                .collect();
            for (id, words, short, model, descriptor) in [
                (
                    "ru-RU",
                    ru.as_str(),
                    include_str!("../data/language-packs/ru-RU/common-short-words.txt"),
                    &include_bytes!("../data/scoring/ru-RU.json")[..],
                    &include_bytes!("../data/input/ru-RU.json")[..],
                ),
                (
                    "et-EE",
                    et.as_str(),
                    include_str!("../data/language-packs/et-EE/common-short-words.txt"),
                    &include_bytes!("../data/scoring/et-EE.json")[..],
                    &include_bytes!("../data/input/et-EE.json")[..],
                ),
            ] {
                let id = PackId::parse(id).unwrap();
                let normalize = |text: &str, minimum: usize| -> Vec<String> {
                    text.lines()
                        .map(|line| {
                            line.split_once('/')
                                .map_or(line, |(word, _)| word)
                                .trim()
                                .to_lowercase()
                        })
                        .filter(|word| {
                            word.chars().count() >= minimum && word.chars().all(char::is_alphabetic)
                        })
                        .collect()
                };
                let words = normalize(words, 2);
                let short = normalize(short, 1);
                registry
                    .insert(
                        DictionaryPack::from_words(
                            id,
                            words.iter().map(String::as_str),
                            short.iter().map(String::as_str),
                        )
                        .unwrap()
                        .with_scoring_model(ScoringModel::from_json(model).unwrap())
                        .with_input_descriptor(InputPackDescriptor::from_json(descriptor).unwrap())
                        .unwrap(),
                    )
                    .unwrap();
            }
            registry
                .set_enabled([
                    crate::Language::English,
                    crate::Language::Russian,
                    crate::Language::Estonian,
                ])
                .unwrap();
            registry
        })
        .clone()
}

pub fn detector() -> Detector {
    Detector::with_registry(DetectorConfig::default(), Arc::new(registry()))
}

pub fn configured_detector(config: DetectorConfig) -> Detector {
    Detector::with_registry(config, Arc::new(registry()))
}

pub fn descriptor(id: &PackId) -> Option<Arc<InputPackDescriptor>> {
    registry()
        .active(id)
        .and_then(|pack| pack.input_descriptor().cloned())
        .map(Arc::new)
}
