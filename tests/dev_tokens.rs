//! Ratchets for unwanted conversions of text typed correctly in the English
//! layout: command, tool and package names in terminals (audit finding F14),
//! English words with punctuation next to them (findings C1-1 and C1-2), and
//! code paths, file names and identifiers with punctuation between letters.
//!
//! Each token is replayed through the detector as if the same physical keys
//! had been read in the Russian and Estonian layouts. The detector converts a
//! few command names; the budgets below are the numbers measured when that
//! corpus was added (6 of 1659 by the dictionary detector, 1 more by the layout
//! model). It converts none of the punctuated words and none of the technical
//! tokens. The budgets may only go down.
//!
//! Re-baselining: run `cargo run --example measure_token_false_positives` (add
//! `--punctuated` for the punctuated words or `--technical` for the technical
//! tokens) to list the converted tokens, then
//! change the constants in the same commit as the detector or corpus change and
//! give the reason in the commit message. A higher budget is a detection-policy
//! decision (D4 of the audit remediation plan), not a test fix.
#![cfg(feature = "legacy-bundled-input")]

#[path = "support/token_measure.rs"]
mod token_measure;

use autokeyboardlayot::{DictionaryRegistry, Language};
use std::sync::Arc;
use token_measure::{
    CORPUS, LAYOUTS, Layouts, PUNCTUATED, TECHNICAL, detectors, measure, parse_punctuated_tokens,
    parse_technical_tokens, parse_tokens,
};

/// A smaller corpus would make the percentages meaningless.
const MINIMUM_TOKENS: usize = 1500;
/// Tokens the dictionary detector alone converts.
const DICTIONARY_BUDGET: usize = 6;
/// Tokens converted only once the layout model is switched on.
const MODEL_EXTRA_BUDGET: usize = 1;
/// The same for the punctuated English words.
const MINIMUM_PUNCTUATED_TOKENS: usize = 2500;
const PUNCTUATED_DICTIONARY_BUDGET: usize = 0;
const PUNCTUATED_MODEL_EXTRA_BUDGET: usize = 0;
/// The same for the code paths, file names and identifiers.
const MINIMUM_TECHNICAL_TOKENS: usize = 5000;
const TECHNICAL_DICTIONARY_BUDGET: usize = 0;
const TECHNICAL_MODEL_EXTRA_BUDGET: usize = 0;

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

fn assert_within_budget(tokens: &[String], dictionary_budget: usize, model_extra_budget: usize) {
    let (dictionary_only, with_model) = bundled();
    let result = measure(tokens, &layouts(), &dictionary_only, &with_model);
    assert_eq!(
        result.tokens,
        tokens.len(),
        "every corpus token must be readable in all three layouts"
    );
    assert!(
        result.dictionary.len() <= dictionary_budget,
        "the dictionary detector converts {} of {} tokens typed correctly in the English layout, \
         budget {dictionary_budget}:{}\n{REBASELINE}",
        result.dictionary.len(),
        result.tokens,
        listed(&result.dictionary)
    );
    assert!(
        result.model_extra.len() <= model_extra_budget,
        "the layout model converts {} more tokens, budget {model_extra_budget}:{}\n{REBASELINE}",
        result.model_extra.len(),
        listed(&result.model_extra)
    );
    if result.dictionary.len() < dictionary_budget || result.model_extra.len() < model_extra_budget
    {
        eprintln!(
            "note: fewer unwanted conversions than budgeted ({} + {}); lower the constants to keep the gain",
            result.dictionary.len(),
            result.model_extra.len()
        );
    }
}

#[test]
fn dev_tokens_false_positive_budget() {
    let tokens = parse_tokens(CORPUS).expect("corpus");
    assert_within_budget(&tokens, DICTIONARY_BUDGET, MODEL_EXTRA_BUDGET);
}

#[test]
fn punctuated_english_false_positive_budget() {
    let tokens = parse_punctuated_tokens(PUNCTUATED).expect("punctuated corpus");
    assert_within_budget(
        &tokens,
        PUNCTUATED_DICTIONARY_BUDGET,
        PUNCTUATED_MODEL_EXTRA_BUDGET,
    );
}

#[test]
fn technical_tokens_false_positive_budget() {
    let tokens = parse_technical_tokens(TECHNICAL).expect("technical corpus");
    assert_within_budget(
        &tokens,
        TECHNICAL_DICTIONARY_BUDGET,
        TECHNICAL_MODEL_EXTRA_BUDGET,
    );
}

#[test]
fn the_technical_corpus_is_a_sorted_unique_list_with_a_provenance_header() {
    let tokens = parse_technical_tokens(TECHNICAL).expect("technical corpus");
    assert!(
        tokens.len() >= MINIMUM_TECHNICAL_TOKENS,
        "{} tokens, at least {MINIMUM_TECHNICAL_TOKENS} are needed",
        tokens.len()
    );
    assert_provenance_header(TECHNICAL);
    for required in ["Default::default", "LANGID:KLID", "key.md"] {
        assert!(
            tokens.binary_search(&required.to_owned()).is_ok(),
            "{required:?} is part of the corpus"
        );
    }
    assert!(
        parse_technical_tokens("key.md\nkey.md\n").is_err(),
        "repeats are rejected"
    );
    assert!(
        parse_technical_tokens("ab.cd\naa.bb\n").is_err(),
        "unsorted input is rejected"
    );
    assert!(
        parse_technical_tokens("key\n").is_err(),
        "a token needs a separator"
    );
    assert!(
        parse_technical_tokens("a.md\n").is_err(),
        "one-letter parts are rejected"
    );
    assert!(
        parse_technical_tokens("key.\n").is_err(),
        "edges are letters"
    );
    assert!(
        parse_technical_tokens("key:::md\n").is_err(),
        "at most two colons"
    );
    assert!(parse_technical_tokens("key.m2\n").is_err(), "letters only");
}

fn assert_provenance_header(corpus: &str) {
    let header: Vec<&str> = corpus
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    for required in ["Sources", "Snapshot taken on", "Format:"] {
        assert!(
            header.iter().any(|line| line.contains(required)),
            "the corpus header must contain {required:?}"
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
    assert_provenance_header(CORPUS);
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
fn the_punctuated_corpus_is_a_sorted_unique_list_with_a_provenance_header() {
    let tokens = parse_punctuated_tokens(PUNCTUATED).expect("punctuated corpus");
    assert!(
        tokens.len() >= MINIMUM_PUNCTUATED_TOKENS,
        "{} tokens, at least {MINIMUM_PUNCTUATED_TOKENS} are needed",
        tokens.len()
    );
    assert_provenance_header(PUNCTUATED);
    let thousands = tokens.len() / 1000;
    let documented = format!(
        "{thousands} {:03} English words with punctuation",
        tokens.len() % 1000
    );
    let docs = include_str!("../docs/layout-model.md");
    let mentions = docs.matches("English words with punctuation").count();
    assert!(
        mentions > 0 && docs.matches(&documented).count() == mentions,
        "docs/layout-model.md must give the corpus size wherever it names it: {documented:?}"
    );
    for required in [
        "can't", "don't", "e.g.", "i've", "it.", "key.", "me.", "now,",
    ] {
        assert!(
            tokens.binary_search(&required.to_owned()).is_ok(),
            "{required:?} is part of the corpus"
        );
    }
    assert!(
        parse_punctuated_tokens("it.\nit.\n").is_err(),
        "repeats are rejected"
    );
    assert!(
        parse_punctuated_tokens("me.\nit.\n").is_err(),
        "unsorted input is rejected"
    );
    assert!(
        parse_punctuated_tokens("a.\n").is_err(),
        "one letter is rejected"
    );
    assert!(
        parse_punctuated_tokens("word\n").is_err(),
        "a token needs punctuation"
    );
    assert!(
        parse_punctuated_tokens("It.\n").is_err(),
        "capitals are rejected"
    );
    assert!(
        parse_punctuated_tokens("it!\n").is_err(),
        "other punctuation is rejected"
    );
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

    // And so does measure() itself, which judges every token with no context.
    let tokens = vec![
        "ghbdtn".to_owned(),
        "hello".to_owned(),
        "k[simusele".to_owned(),
    ];
    let result = measure(&tokens, &layouts, &dictionary_only, &with_model);
    assert_eq!(result.tokens, 3);
    assert_eq!(result.dictionary.len(), 1);
    assert_eq!(result.dictionary[0].token, "ghbdtn");
    assert_eq!(result.model_extra.len(), 1, "{:?}", result.model_extra);
    assert_eq!(result.model_extra[0].token, "k[simusele");
    assert_eq!(result.model_extra[0].replacement, "küsimusele");
}
