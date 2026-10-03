//! Offline measurement of unwanted conversions on real command-line names.
//! Not installed, published or used by the agent.
//!
//! Usage: measure_token_false_positives [--store STORE_COPY] [TOKENS.txt]
//!
//! Without arguments it measures tests/fixtures/dev-tokens.txt against the
//! bundled EN/RU/ET dictionaries, exactly as tests/dev_tokens.rs does. With
//! --store the dictionaries come from a copy of an installed package store
//! (never the live one), as in evaluate_layout_detection. Run it to see which
//! tokens are converted before changing the budgets in tests/dev_tokens.rs.
use autokeyboardlayot::{
    DictionaryRegistry, PackId, installed_packages::InstalledPackages,
    language_package::PackageTrust, package_store::PackageStore,
};
use std::{collections::BTreeSet, ffi::OsString, fs, path::Path, sync::Arc};

#[path = "../tests/support/token_measure.rs"]
mod token_measure;
use token_measure::{CORPUS, LAYOUTS, Layouts, detectors, measure, parse_tokens};

const USAGE: &str = "usage: measure_token_false_positives [--store STORE_COPY] [TOKENS.txt]";

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut store: Option<OsString> = None;
    let mut corpus: Option<OsString> = None;
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--store" && store.is_none() {
            store = Some(arguments.next().ok_or(USAGE)?);
        } else if corpus.is_none() && !argument.to_string_lossy().starts_with("--") {
            corpus = Some(argument);
        } else {
            return Err(USAGE.into());
        }
    }
    let text = match corpus {
        Some(path) => fs::read_to_string(path)?,
        None => CORPUS.to_owned(),
    };
    let tokens = parse_tokens(&text)?;
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
        for conversion in list {
            println!("    {conversion}");
        }
    }
    Ok(())
}
