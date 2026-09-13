//! Compile complete lossless candidate lists through the production API.
//! No filtering, signing, installation, input-readiness grant, or publication.
use autokeyboardlayot::{DictionaryPack, PackId};
use serde::Deserialize;
use std::{fs::File, io::Read, path::Path, time::Instant};

const REVISION: &str = "42233e6c36ce792031bcccfa17cdd0cec9af5fa7";
const MAX_FILE: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    format: u8,
    source_revision: String,
    source_sha256: String,
    bins: Vec<Vec<String>>,
}

fn audit(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for (id, list) in [
        ("ar", "large_ar"),
        ("bn", "large_bn"),
        ("de", "large_de"),
        ("es", "large_es"),
        ("fr", "large_fr"),
        ("ja", "large_ja"),
        ("pt", "large_pt"),
        ("zh", "large_zh"),
        ("hi", "small_hi"),
        ("id", "small_id"),
        ("ur", "small_ur"),
    ] {
        let mut bytes = Vec::new();
        File::open(root.join(format!("{list}.json")))?
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err("candidate file exceeds bound".into());
        }
        let source: Source = serde_json::from_slice(&bytes)?;
        if source.format != 1
            || source.source_revision != REVISION
            || source.source_sha256.len() != 64
            || !source.source_sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || source.bins.len() != if list.starts_with("large") { 800 } else { 600 }
        {
            return Err("unexpected source metadata".into());
        }
        drop(bytes);
        let words: Vec<&str> = source.bins.iter().flatten().map(String::as_str).collect();
        let started = Instant::now();
        let compiled = DictionaryPack::from_words(PackId::parse(id)?, words.iter().copied(), []);
        match compiled {
            Ok(pack) => {
                for word in &words {
                    if !pack.contains(word) {
                        return Err("full-list membership round trip failed".into());
                    }
                }
                if pack.input_descriptor().is_some() || pack.scoring_model().is_some() {
                    return Err("candidate unexpectedly gained input rules".into());
                }
                println!(
                    "{id}: complete_round_trip=pass records={} elapsed_ms={} rules=absent",
                    words.len(),
                    started.elapsed().as_millis()
                );
            }
            Err(error) => {
                println!(
                    "{id}: complete_round_trip=rejected records={} error={error}; no filtered retry",
                    words.len()
                );
                for (bin, bucket) in source.bins.iter().enumerate() {
                    for word in bucket {
                        if word.is_empty()
                            || word.len() > 256
                            || word.chars().any(|c| c.is_control() || c.is_whitespace())
                        {
                            // JSON escaping keeps source control characters out of log structure.
                            println!(
                                "exception {}",
                                serde_json::json!({"language": id, "negative_centibels": bin, "word": word})
                            );
                        }
                    }
                }
            }
        }
    }
    println!("WORDFREQ_RUNTIME_AUDIT_COMPLETE (candidate audit, not acceptance)");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = args
        .next()
        .ok_or("expected lossless audit export directory")?;
    if args.next().is_some() {
        return Err("expected exactly one directory".into());
    }
    audit(Path::new(&root))
}
