//! Read-only sizing of the actual normalized build outputs before pack migration.
//! This does not export user words, sign packages, or grant adapter readiness.
use fst::{Set, Streamer};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize)]
struct Statistics {
    id: &'static str,
    words: usize,
    word_bytes: usize,
    newline_delimited_bytes: usize,
    max_word_bytes: usize,
    max_word_characters: usize,
    newline_delimited_sha256: String,
}

fn statistics(id: &'static str, bytes: &[u8]) -> Result<Statistics, Box<dyn std::error::Error>> {
    let set = Set::new(bytes)?;
    let mut stream = set.stream();
    let mut digest = Sha256::new();
    let mut result = Statistics {
        id,
        words: 0,
        word_bytes: 0,
        newline_delimited_bytes: 0,
        max_word_bytes: 0,
        max_word_characters: 0,
        newline_delimited_sha256: String::new(),
    };
    while let Some(word) = stream.next() {
        let text = std::str::from_utf8(word)?;
        result.words += 1;
        result.word_bytes += word.len();
        result.newline_delimited_bytes += word.len() + 1;
        result.max_word_bytes = result.max_word_bytes.max(word.len());
        result.max_word_characters = result.max_word_characters.max(text.chars().count());
        digest.update(word);
        digest.update(b"\n");
    }
    result.newline_delimited_sha256 = hex(&digest.finalize());
    Ok(result)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Inspect precisely what build.rs embedded, without maintaining a second
    // gzip/Hunspell normalizer with potentially different Unicode semantics.
    let inputs: [(&str, &[u8]); 6] = [
        (
            "en-US",
            include_bytes!(concat!(env!("OUT_DIR"), "/en-US.fst")),
        ),
        (
            "ru-RU",
            include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU.fst")),
        ),
        (
            "et-EE",
            include_bytes!(concat!(env!("OUT_DIR"), "/et-EE.fst")),
        ),
        (
            "en-US-short",
            include_bytes!(concat!(env!("OUT_DIR"), "/en-US-short.fst")),
        ),
        (
            "ru-RU-short",
            include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU-short.fst")),
        ),
        (
            "et-EE-short",
            include_bytes!(concat!(env!("OUT_DIR"), "/et-EE-short.fst")),
        ),
    ];
    let result = inputs
        .into_iter()
        .map(|(id, bytes)| statistics(id, bytes))
        .collect::<Result<Vec<_>, _>>()?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_utf8_bytes_separately_from_characters_and_hashes_exact_lf_stream() {
        let set = Set::from_iter(["hi", "õun", "не"]).unwrap();
        let result = statistics("fixture", set.as_fst().as_bytes()).unwrap();
        assert_eq!(result.words, 3);
        assert_eq!(result.word_bytes, 10);
        assert_eq!(result.newline_delimited_bytes, 13);
        assert_eq!(result.max_word_bytes, 4);
        assert_eq!(result.max_word_characters, 3);
        assert_eq!(
            result.newline_delimited_sha256,
            hex(&Sha256::digest("hi\nõun\nне\n"))
        );
    }

    #[test]
    fn empty_tier_has_no_synthetic_newline() {
        let set = Set::from_iter(std::iter::empty::<&str>()).unwrap();
        let result = statistics("empty", set.as_fst().as_bytes()).unwrap();
        assert_eq!(result.words, 0);
        assert_eq!(result.newline_delimited_bytes, 0);
        assert_eq!(result.newline_delimited_sha256, hex(&Sha256::digest(b"")));
    }
}
