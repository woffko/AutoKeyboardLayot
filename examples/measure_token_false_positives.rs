//! Offline measurement of unwanted conversions on typed-correctly tokens.
//! Not installed, published or used by the agent.
//!
//! Usage: measure_token_false_positives [--store STORE_COPY] [TOKENS.txt]
//!        measure_token_false_positives [--store STORE_COPY] --random LENGTH COUNT
//!
//! Without a mode it measures tests/fixtures/dev-tokens.txt (real command,
//! tool and package names) against the bundled EN/RU/ET dictionaries, exactly
//! as tests/dev_tokens.rs does; run it to see which tokens are converted
//! before changing the budgets there. With --random it measures COUNT uniformly
//! random lowercase strings of LENGTH letters instead (a fixed seed per
//! length): machine-generated text such as hashes and identifiers, which no
//! dictionary knows. With --store the dictionaries come from a copy of an
//! installed package store (never the live one), as in evaluate_layout_detection.
use autokeyboardlayot::{
    DictionaryRegistry, PackId, installed_packages::InstalledPackages,
    language_package::PackageTrust, package_store::PackageStore,
};
use std::{collections::BTreeSet, ffi::OsString, fs, path::Path, sync::Arc};

#[path = "../tests/support/token_measure.rs"]
mod token_measure;
use token_measure::{CORPUS, LAYOUTS, Layouts, detectors, measure, parse_tokens};

const USAGE: &str = "usage: measure_token_false_positives [--store STORE_COPY] [TOKENS.txt | --random LENGTH COUNT]";
/// Random strings are only counted; this many converted ones are listed.
const RANDOM_LISTED: usize = 10;

enum Mode {
    Corpus(Option<OsString>),
    Random { length: usize, count: usize },
}

/// `count` as a percentage of `total`.
fn percent(count: usize, total: usize) -> f64 {
    100.0 * count as f64 / total.max(1) as f64
}

fn registry_from_store(path: &Path) -> Result<Arc<DictionaryRegistry>, Box<dyn std::error::Error>> {
    let trust = PackageTrust::release()?;
    let snapshot = PackageStore::open(path)?.load(&trust)?;
    let selected: BTreeSet<PackId> = ["en-US", "ru-RU", "et-EE"]
        .into_iter()
        .map(|id| PackId::parse(id).expect("valid ID"))
        .collect();
    Ok(InstalledPackages::from_store(&snapshot, &selected)?.dictionaries)
}

/// Lowercase letters from a xorshift generator; the same strings every run.
fn random_tokens(length: usize, count: usize) -> Vec<String> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64 ^ length as u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..count)
        .map(|_| {
            (0..length)
                .map(|_| char::from(b'a' + (next() % 26) as u8))
                .collect()
        })
        .collect()
}

fn parse_arguments() -> Result<(Option<OsString>, Mode), Box<dyn std::error::Error>> {
    let mut store: Option<OsString> = None;
    let mut mode: Option<Mode> = None;
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--store" && store.is_none() {
            store = Some(arguments.next().ok_or(USAGE)?);
        } else if argument == "--random" && mode.is_none() {
            let number = |value: Option<OsString>| -> Result<usize, Box<dyn std::error::Error>> {
                Ok(value
                    .ok_or(USAGE)?
                    .to_str()
                    .ok_or(USAGE)?
                    .parse::<usize>()
                    .map_err(|_| USAGE)?)
            };
            let length = number(arguments.next())?;
            let count = number(arguments.next())?;
            if !(1..=64).contains(&length) || !(1..=1_000_000).contains(&count) {
                return Err("LENGTH must be 1 to 64 and COUNT 1 to 1000000".into());
            }
            mode = Some(Mode::Random { length, count });
        } else if mode.is_none() && !argument.to_string_lossy().starts_with("--") {
            mode = Some(Mode::Corpus(Some(argument)));
        } else {
            return Err(USAGE.into());
        }
    }
    Ok((store, mode.unwrap_or(Mode::Corpus(None))))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (store, mode) = parse_arguments()?;
    let (tokens, listed) = match mode {
        Mode::Corpus(path) => {
            let text = match path {
                Some(path) => fs::read_to_string(path)?,
                None => CORPUS.to_owned(),
            };
            (parse_tokens(&text)?, usize::MAX)
        }
        Mode::Random { length, count } => (random_tokens(length, count), RANDOM_LISTED),
    };
    let layouts = Layouts::parse(LAYOUTS)?;
    let dictionaries = match store {
        Some(path) => registry_from_store(Path::new(&path))?,
        None => Arc::new(DictionaryRegistry::embedded()),
    };
    let (dictionary_only, with_model) = detectors(dictionaries)?;
    let result = measure(&tokens, &layouts, &dictionary_only, &with_model);

    println!(
        "tokens read in every layout: {} of {}",
        result.tokens,
        tokens.len()
    );
    for (title, list) in [
        ("dictionary detector converts", &result.dictionary),
        ("layout model adds", &result.model_extra),
    ] {
        println!(
            "{title}: {} ({:.2}%)",
            list.len(),
            percent(list.len(), result.tokens)
        );
        for conversion in list.iter().take(listed) {
            println!("    {conversion}");
        }
        if list.len() > listed {
            println!("    ... {} more", list.len() - listed);
        }
    }
    Ok(())
}
