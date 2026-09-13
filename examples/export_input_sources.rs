//! Bootstrap reviewed built-in data into a NEW source directory; never signs or installs.
use fst::{Set, Streamer};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;

#[derive(Serialize)]
struct FileRecord {
    path: String,
    bytes: usize,
    sha256: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_source(root: &Path, name: &str, bytes: &[u8]) -> io::Result<FileRecord> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(name))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(FileRecord {
        path: name.to_owned(),
        bytes: bytes.len(),
        sha256: hex(&Sha256::digest(bytes)),
    })
}

fn write_words(
    root: &Path,
    name: &str,
    fst: &[u8],
) -> Result<FileRecord, Box<dyn std::error::Error>> {
    let set = Set::new(fst)?;
    let mut stream = set.stream();
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(name))?;
    let mut writer = BufWriter::new(file);
    let mut hash = Sha256::new();
    let mut bytes = 0;
    while let Some(word) = stream.next() {
        std::str::from_utf8(word)?;
        writer.write_all(word)?;
        writer.write_all(b"\n")?;
        hash.update(word);
        hash.update(b"\n");
        bytes += word.len() + 1;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(FileRecord {
        path: name.to_owned(),
        bytes,
        sha256: hex(&hash.finalize()),
    })
}

fn export(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // No overwrite or merging. On failure retain the partial directory for inspection;
    // the final completion marker is deliberately absent until all writes succeed.
    fs::create_dir(root)?;
    let mut records = Vec::new();
    for (id, words, short) in [
        (
            "ru-RU",
            include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU.fst")).as_slice(),
            include_bytes!(concat!(env!("OUT_DIR"), "/ru-RU-short.fst")).as_slice(),
        ),
        (
            "et-EE",
            include_bytes!(concat!(env!("OUT_DIR"), "/et-EE.fst")).as_slice(),
            include_bytes!(concat!(env!("OUT_DIR"), "/et-EE-short.fst")).as_slice(),
        ),
    ] {
        fs::create_dir(root.join(id))?;
        records.push(write_words(root, &format!("{id}/words.txt"), words)?);
        records.push(write_words(root, &format!("{id}/short-words.txt"), short)?);
    }
    // Compile-time allowlist: no user lexicon, config, credentials, or runtime file scan.
    for (name, bytes) in [
        (
            "ru-RU/input.json",
            include_bytes!("../data/input/ru-RU.json").as_slice(),
        ),
        (
            "et-EE/input.json",
            include_bytes!("../data/input/et-EE.json").as_slice(),
        ),
        (
            "ru-RU/scoring.json",
            include_bytes!("../data/scoring/ru-RU.json").as_slice(),
        ),
        (
            "et-EE/scoring.json",
            include_bytes!("../data/scoring/et-EE.json").as_slice(),
        ),
        (
            "ru-RU/LICENSE.words.txt",
            include_bytes!("../data/language-packs/ru-RU/LICENSE.words.txt").as_slice(),
        ),
        (
            "et-EE/LICENSE.words.txt",
            include_bytes!("../data/language-packs/et-EE/LICENSE.words.txt").as_slice(),
        ),
        (
            "ru-RU/original.words.txt.gz",
            include_bytes!("../data/language-packs/ru-RU/words.txt.gz").as_slice(),
        ),
        (
            "et-EE/original.words.dic",
            include_bytes!("../data/language-packs/et-EE/words.dic").as_slice(),
        ),
        (
            "et-EE/original.words.aff",
            include_bytes!("../data/language-packs/et-EE/words.aff").as_slice(),
        ),
        (
            "ru-RU/original.short-words.txt",
            include_bytes!("../data/language-packs/ru-RU/common-short-words.txt").as_slice(),
        ),
        (
            "et-EE/original.short-words.txt",
            include_bytes!("../data/language-packs/et-EE/common-short-words.txt").as_slice(),
        ),
        (
            "UPSTREAM.md",
            include_bytes!("../data/language-packs/README.md").as_slice(),
        ),
    ] {
        records.push(write_source(root, name, bytes)?);
    }
    let provenance = serde_json::json!({
        "format": 1,
        "status": "unsigned-source-draft",
        "normalization": "build.rs: trim, Unicode lowercase, alphabetic only, >=2 characters, sorted unique LF; short tier <=3 characters; ET base entries only, affixes NOT expanded",
        "release_gates": ["project-owned rules and short-tier licensing", "production signing authorization", "adapter and physical acceptance"],
        "files": records,
    });
    write_source(
        root,
        "SOURCE-COMPLETE.json",
        &serde_json::to_vec_pretty(&provenance)?,
    )?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = args
        .next()
        .ok_or("usage: export_input_sources NEW_DIRECTORY")?;
    if args.next().is_some() {
        return Err("expected exactly one new output directory".into());
    }
    export(Path::new(&root))?;
    println!(
        "Unsigned source export complete; no signing, installation, or publication performed."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_existing_destination_without_touching_contents() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("sentinel"), b"preserve").unwrap();
        assert!(export(temp.path()).is_err());
        assert_eq!(fs::read(temp.path().join("sentinel")).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn streamed_words_have_exact_deterministic_hash_and_cannot_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let set = Set::from_iter(["hi", "õun", "не"]).unwrap();
        let record = write_words(temp.path(), "words.txt", set.as_fst().as_bytes()).unwrap();
        let expected = "hi\nõun\nне\n".as_bytes();
        assert_eq!(fs::read(temp.path().join("words.txt")).unwrap(), expected);
        assert_eq!(record.bytes, expected.len());
        assert_eq!(record.sha256, hex(&Sha256::digest(expected)));
        assert!(write_words(temp.path(), "words.txt", set.as_fst().as_bytes()).is_err());
        assert!(write_source(temp.path(), "words.txt", b"overwrite").is_err());
        assert_eq!(fs::read(temp.path().join("words.txt")).unwrap(), expected);
    }
}
