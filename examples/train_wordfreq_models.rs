//! Offline candidate models, never input-adapter authorization.
//! Usage: train_wordfreq_models PREPARED_ROOT NEW_OUTPUT_DIRECTORY
use autokeyboardlayot::ScoringModel;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_script::{Script, UnicodeScript};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("source exceeds bound".into());
    }
    Ok(bytes)
}

fn mark(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

fn scripts(id: &str) -> &'static [Script] {
    match id {
        "ar" | "ur" => &[Script::Arabic],
        "bn" => &[Script::Bengali],
        "hi" => &[Script::Devanagari],
        "ja" => &[Script::Han, Script::Hiragana, Script::Katakana],
        "zh" => &[Script::Han],
        _ => &[Script::Latin],
    }
}

fn eligible(word: &str, scripts: &[Script]) -> bool {
    !word.is_empty()
        && !word.chars().next().is_some_and(mark)
        && word.chars().all(|c| {
            c.to_lowercase().eq(std::iter::once(c))
                && ((mark(c) && c.script() == Script::Inherited)
                    || ((mark(c) || c.is_alphabetic())
                        && scripts
                            .iter()
                            .any(|s| c.script_extension().contains_script(*s))))
        })
}

fn ranges(chars: &BTreeSet<char>) -> Vec<(char, char)> {
    let mut result: Vec<(char, char)> = Vec::new();
    for &c in chars {
        if let Some(last) = result.last_mut()
            && u32::from(last.1) + 1 == u32::from(c)
        {
            last.1 = c;
        } else {
            result.push((c, c));
        }
    }
    result
}

fn top(counts: BTreeMap<String, u64>) -> Vec<String> {
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts.into_iter().take(512).map(|(s, _)| s).collect()
}

fn train(root: &Path, output: &Path, id: &str) -> Result<()> {
    let dir = root.join(id);
    let receipt: Value = serde_json::from_slice(&read(&dir.join("SOURCE.json"), 65536)?)?;
    let bytes = read(&dir.join("words.txt"), 32 * 1024 * 1024)?;
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let entries: Vec<_> = receipt["files"]
        .as_array()
        .ok_or("missing files")?
        .iter()
        .filter(|f| f["path"] == "words.txt")
        .collect();
    let text = std::str::from_utf8(&bytes)?;
    let rows = text.lines().count();
    if receipt["format"] != 1
        || receipt["language"] != id
        || receipt["removed_records"] != 0
        || receipt["input_records"] != rows
        || receipt["output_records"] != rows
        || entries.len() != 1
        || entries[0]["bytes"] != bytes.len()
        || entries[0]["sha256"] != hash
    {
        return Err("prepared source receipt mismatch".into());
    }
    let mut letters = BTreeSet::new();
    let mut marks = BTreeSet::new();
    let mut bi = BTreeMap::new();
    let mut tri = BTreeMap::new();
    let mut training_rows = 0;
    for word in text.lines().filter(|w| eligible(w, scripts(id))) {
        training_rows += 1;
        let chars: Vec<_> = word.chars().collect();
        for &c in &chars {
            if mark(c) {
                marks.insert(c);
            } else {
                letters.insert(c);
            }
        }
        // Row support, not corpus-frequency weighting; repeated ngrams in one
        // row count once. No dictionary records are removed or rewritten.
        for (n, counts) in [(2, &mut bi), (3, &mut tri)] {
            let unique: BTreeSet<String> = chars.windows(n).map(|w| w.iter().collect()).collect();
            for gram in unique {
                *counts.entry(gram).or_insert(0) += 1;
            }
        }
    }
    if training_rows == 0 {
        return Err("empty training repertoire".into());
    }
    let ranges = ranges(&letters);
    let bigrams = top(bi);
    let trigrams = top(tri);
    let model = json!({"format":4, "letters":letters.iter().collect::<String>(), "marks":marks.iter().collect::<String>(),
        "vowels":"", "bigrams":bigrams, "trigrams":trigrams, "rare":[],
        "policy":{"bigrams":!bigrams.is_empty(), "trigrams":!trigrams.is_empty(),
        "vowels":false, "statistical_targets":false}});
    let encoded = serde_json::to_vec_pretty(&model)?;
    let validation = ScoringModel::from_json(&encoded);
    let status = match validation {
        Ok(model) => {
            for word in text.lines().filter(|w| eligible(w, scripts(id))) {
                if !model.accepts_word(word) {
                    return Err("runtime rejected a training token".into());
                }
            }
            "accepted".to_owned()
        }
        Err(e) => e.to_string(),
    };
    let report = json!({"format":1,"language":id,"source_words_sha256":hash,
        "source_rows":rows,"training_rows":training_rows,"excluded_training_rows":rows-training_rows,
        "dictionary_records_removed":0,"weighting":"distinct-ngram row support",
        "letters":letters.len(),"ranges":ranges.len(),"marks":marks.len(),
        "model_bytes":encoded.len(),"runtime_validation":status,"correction_ready":false,
        "license":"Derivative of prepared wordfreq data; original source notices and attribution remain required."});
    fs::write(output.join(format!("{id}.candidate.json")), encoded)?;
    fs::write(
        output.join(format!("{id}.training.json")),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "{id}: rows={rows} training={training_rows} letters={} ranges={} runtime={status}",
        letters.len(),
        ranges.len()
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("expected PREPARED_ROOT NEW_OUTPUT_DIRECTORY".into());
    }
    let root = Path::new(&args[0]);
    let output = Path::new(&args[1]);
    // Do not overwrite a previous experiment or any installed package.
    fs::create_dir(output)?;
    for id in [
        "ar", "bn", "de", "es", "fr", "ja", "pt", "zh", "hi", "id", "ur",
    ] {
        train(root, output, id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_filter_and_ranges_are_lossless() {
        assert!(eligible("cafe\u{301}", scripts("fr")));
        assert!(eligible("किताब", scripts("hi")));
        assert!(!eligible("\u{301}a", scripts("fr")));
        assert!(!eligible("hello", scripts("hi")));
        assert!(!eligible("hello-world", scripts("fr")));
        assert_eq!(
            ranges(&"abdé".chars().collect()),
            vec![('a', 'b'), ('d', 'd'), ('é', 'é')]
        );
        assert_eq!(
            top(BTreeMap::from([("ba".into(), 2), ("ab".into(), 2)])),
            vec!["ab", "ba"]
        );
    }
}
