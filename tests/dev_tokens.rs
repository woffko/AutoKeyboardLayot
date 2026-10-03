//! Ratchet for audit finding F14: unwanted conversions in terminals.
//!
//! Real command, tool and package names, typed correctly in the English
//! layout, are replayed through the detector as if the same physical keys had
//! been read in the Russian and Estonian layouts. The detector converts a few
//! of them; the budgets below are the numbers measured when the corpus was
//! added (6 of 1659 by the dictionary detector, 1 more by the layout model).
//! They may only go down.
//!
//! Re-baselining: run `cargo run --example measure_token_false_positives` to
//! list the converted tokens, then change the constants in the same commit as
//! the detector or corpus change and give the reason in the commit message. A
//! higher budget is a detection-policy decision (D4 of the audit remediation
//! plan), not a test fix.
#![cfg(feature = "legacy-bundled-input")]

#[path = "support/token_measure.rs"]
mod token_measure;

use autokeyboardlayot::{DictionaryRegistry, Language};
use std::sync::Arc;
use token_measure::{CORPUS, LAYOUTS, Layouts, detectors, measure, parse_tokens};

/// A smaller corpus would make the percentages meaningless.
const MINIMUM_TOKENS: usize = 1500;
/// Tokens the dictionary detector alone converts.
const DICTIONARY_BUDGET: usize = 6;
/// Tokens converted only once the layout model is switched on.
const MODEL_EXTRA_BUDGET: usize = 1;

const REBASELINE: &str = "If the change is intended, follow the re-baselining steps at the top of tests/dev_tokens.rs. \
                          Raising a budget changes detection policy and needs an explicit decision.";

fn layouts() -> Layouts {
    Layouts::parse(LAYOUTS).expect("layout fixture")
}

fn bundled() -> (autokeyboardlayot::Detector, autokeyboardlayot::Detector) {
    detectors(Arc::new(DictionaryRegistry::embedded())).expect("detectors")
}

fn listed(conversions: &[token_measure::Conversion]) -> String {
    conversions
        .iter()
        .map(|conversion| format!("\n    {conversion}"))
        .collect()
}

#[test]
fn dev_tokens_false_positive_budget() {
    let tokens = parse_tokens(CORPUS).expect("corpus");
    let (dictionary_only, with_model) = bundled();
    let result = measure(&tokens, &layouts(), &dictionary_only, &with_model);
    assert_eq!(
        result.tokens,
        tokens.len(),
        "every corpus token must be readable in all three layouts"
    );
    assert!(
        result.dictionary.len() <= DICTIONARY_BUDGET,
        "the dictionary detector converts {} of {} tokens typed correctly in the English layout, \
         budget {DICTIONARY_BUDGET}:{}\n{REBASELINE}",
        result.dictionary.len(),
        result.tokens,
        listed(&result.dictionary)
    );
    assert!(
        result.model_extra.len() <= MODEL_EXTRA_BUDGET,
        "the layout model converts {} more tokens, budget {MODEL_EXTRA_BUDGET}:{}\n{REBASELINE}",
        result.model_extra.len(),
        listed(&result.model_extra)
    );
    if result.dictionary.len() < DICTIONARY_BUDGET || result.model_extra.len() < MODEL_EXTRA_BUDGET
    {
        eprintln!(
            "note: fewer unwanted conversions than budgeted ({} + {}); lower the constants to keep the gain",
            result.dictionary.len(),
            result.model_extra.len()
        );
    }
}

#[test]
fn the_corpus_is_a_sorted_unique_list_with_a_provenance_header() {
    let tokens = parse_tokens(CORPUS).expect("corpus");
    assert!(
        tokens.len() >= MINIMUM_TOKENS,
        "{} tokens, at least {MINIMUM_TOKENS} are needed",
        tokens.len()
    );
    let header: Vec<&str> = CORPUS
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    for required in ["Sources", "Snapshot taken on", "Format:"] {
        assert!(
            header.iter().any(|line| line.contains(required)),
            "the corpus header must contain {required:?}"
        );
    }
    assert!(parse_tokens("abc\nabc\n").is_err(), "repeats are rejected");
    assert!(
        parse_tokens("abd\nabc\n").is_err(),
        "unsorted input is rejected"
    );
    assert!(parse_tokens("ab\n").is_err(), "short tokens are rejected");
    assert!(parse_tokens("Abc\n").is_err(), "capitals are rejected");
    assert!(parse_tokens("ab-c\n").is_err(), "separators are rejected");
}

#[test]
fn the_layouts_read_the_same_physical_keys() {
    let layouts = layouts();
    assert_eq!(
        layouts.readings("ghbdtn").expect("letters"),
        vec![
            (Language::Russian, "привет".to_owned()),
            (Language::Estonian, "ghbdtn".to_owned()),
        ]
    );
    assert_eq!(
        layouts.readings("k[simusele").expect("bracket key"),
        vec![
            (Language::Russian, "лхышьгыуду".to_owned()),
            (Language::Estonian, "küsimusele".to_owned()),
        ]
    );
    assert!(layouts.readings("é").is_none(), "no key prints it");
}

/// A budget of zero conversions must never be satisfied by a measurement that
/// cannot convert anything: both stages have to work through the same
/// detectors the ratchet uses.
#[test]
fn the_measuring_detectors_can_convert_at_both_stages() {
    let layouts = layouts();
    let (dictionary_only, with_model) = bundled();
    let mapped = layouts.readings("ghbdtn").expect("letters");
    let detection = dictionary_only
        .detect_mapped_candidates("ghbdtn", Language::English, &mapped)
        .expect("a mistyped Russian word is converted by the dictionary");
    assert_eq!(detection.replacement, "привет");

    let mapped = layouts.readings("k[simusele").expect("bracket key");
    assert!(
        dictionary_only
            .detect_mapped_candidates("k[simusele", Language::English, &mapped)
            .is_none(),
        "this word needs the model"
    );
    let detection = with_model
        .detect_mapped_candidates_in_context(
            "k[simusele",
            Language::English,
            &mapped,
            &[Language::English],
        )
        .expect("the layout model converts it");
    assert_eq!(detection.replacement, "küsimusele");

    let tokens = vec!["ghbdtn".to_owned(), "hello".to_owned()];
    let result = measure(&tokens, &layouts, &dictionary_only, &with_model);
    assert_eq!(result.tokens, 2);
    assert_eq!(result.dictionary.len(), 1);
    assert_eq!(result.dictionary[0].token, "ghbdtn");
    assert!(result.model_extra.is_empty());
}
