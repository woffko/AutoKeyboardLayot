//! Validate every prepared dictionary row through the runtime compiler, read-only.
use autokeyboardlayot::{DictionaryPack, PackId};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path, time::Instant};

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("prepared source exceeds bound".into());
    }
    Ok(bytes)
}

fn audit(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for id in [
        "ar", "bn", "de", "es", "fr", "ja", "pt", "zh", "hi", "id", "ur",
    ] {
        let directory = root.join(id);
        let report: Value =
            serde_json::from_slice(&read_bounded(&directory.join("SOURCE.json"), 64 * 1024)?)?;
        let words = read_bounded(&directory.join("words.txt"), 32 * 1024 * 1024)?;
        let hash: String = Sha256::digest(&words)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let files = report["files"].as_array().ok_or("missing source files")?;
        let entries: Vec<_> = files.iter().filter(|f| f["path"] == "words.txt").collect();
        if report["format"] != 1
            || report["language"] != id
            || report["removed_records"] != 0
            || report["input_records"] != report["output_records"]
            || entries.len() != 1
            || entries[0]["bytes"] != words.len()
            || entries[0]["sha256"] != hash
        {
            return Err("prepared source receipt mismatch".into());
        }
        let words = std::str::from_utf8(&words)?;
        let count = words.lines().count();
        if report["output_records"] != count {
            return Err("prepared source row count mismatch".into());
        }
        let started = Instant::now();
        let pack = DictionaryPack::from_words(PackId::parse(id)?, words.lines(), [])?;
        for word in words.lines() {
            if !pack.contains(word) {
                return Err("prepared source membership mismatch".into());
            }
        }
        if pack.input_descriptor().is_some() || pack.scoring_model().is_some() {
            return Err("dictionary-only source unexpectedly granted rules".into());
        }
        println!(
            "{id}: full_prepared_round_trip=pass rows={count} word_bytes={} elapsed_ms={} rules=absent",
            words.len(),
            started.elapsed().as_millis()
        );
    }
    println!("PREPARED_DICTIONARIES_COMPLETE; input rules and physical acceptance remain pending");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = args.next().ok_or("expected prepared source directory")?;
    if args.next().is_some() {
        return Err("expected exactly one directory".into());
    }
    audit(Path::new(&root))
}
