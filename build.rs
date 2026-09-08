use std::fs::File;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use fst::SetBuilder;

const PACKS: &[DictionaryInput] = &[
    DictionaryInput::gzip("en-US", "data/language-packs/en-US/words.txt.gz"),
    DictionaryInput::gzip("ru-RU", "data/language-packs/ru-RU/words.txt.gz"),
    DictionaryInput::hunspell("et-EE", "data/language-packs/et-EE/words.dic"),
    DictionaryInput::plain(
        "en-US-short",
        "data/language-packs/en-US/common-short-words.txt",
    ),
    DictionaryInput::plain(
        "ru-RU-short",
        "data/language-packs/ru-RU/common-short-words.txt",
    ),
    DictionaryInput::plain(
        "et-EE-short",
        "data/language-packs/et-EE/common-short-words.txt",
    ),
];

#[derive(Clone, Copy)]
enum InputKind {
    Gzip,
    Hunspell,
    Plain,
}

#[derive(Clone, Copy)]
struct DictionaryInput {
    id: &'static str,
    path: &'static str,
    kind: InputKind,
}

impl DictionaryInput {
    const fn plain(id: &'static str, path: &'static str) -> Self {
        Self {
            id,
            path,
            kind: InputKind::Plain,
        }
    }
    const fn gzip(id: &'static str, path: &'static str) -> Self {
        Self {
            id,
            path,
            kind: InputKind::Gzip,
        }
    }

    const fn hunspell(id: &'static str, path: &'static str) -> Self {
        Self {
            id,
            path,
            kind: InputKind::Hunspell,
        }
    }
}

fn main() {
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is not set"));
    for pack in PACKS {
        println!("cargo:rerun-if-changed={}", pack.path);
        build_dictionary(*pack, &output);
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=ui/settings.slint");
        slint_build::compile("ui/settings.slint").expect("cannot compile settings UI");
    }
}

fn build_dictionary(input: DictionaryInput, output: &Path) {
    let content = read_dictionary(input);

    let mut words = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if matches!(input.kind, InputKind::Hunspell) && index == 0 && line.parse::<usize>().is_ok()
        {
            continue;
        }
        let surface = match input.kind {
            InputKind::Gzip | InputKind::Plain => line,
            InputKind::Hunspell => line.split_once('/').map_or(line, |(word, _)| word),
        };
        let normalized = surface.trim().to_lowercase();
        if normalized.chars().count() >= 2 && normalized.chars().all(char::is_alphabetic) {
            if matches!(input.kind, InputKind::Plain) {
                assert!(
                    normalized.chars().count() <= 3,
                    "common short-word pack {} contains a long word",
                    input.id
                );
            }
            words.push(normalized);
        }
    }
    words.sort_unstable();
    words.dedup();

    let path = output.join(format!("{}.fst", input.id));
    let writer = BufWriter::new(
        File::create(&path)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", path.display())),
    );
    let mut builder = SetBuilder::new(writer)
        .unwrap_or_else(|error| panic!("cannot create FST for {}: {error}", input.id));
    for word in words {
        builder
            .insert(word)
            .unwrap_or_else(|error| panic!("cannot add word to {}: {error}", input.id));
    }
    builder
        .finish()
        .unwrap_or_else(|error| panic!("cannot finish FST for {}: {error}", input.id));
}

fn read_dictionary(input: DictionaryInput) -> String {
    match input.kind {
        InputKind::Plain => std::fs::read_to_string(input.path)
            .unwrap_or_else(|error| panic!("cannot read language pack {}: {error}", input.path)),
        InputKind::Gzip => {
            let file = File::open(input.path).unwrap_or_else(|error| {
                panic!("cannot open language pack {}: {error}", input.path)
            });
            let mut decoder = BufReader::new(GzDecoder::new(file));
            let mut content = String::new();
            decoder
                .read_to_string(&mut content)
                .unwrap_or_else(|error| {
                    panic!("cannot decode UTF-8 language pack {}: {error}", input.path)
                });
            content
        }
        InputKind::Hunspell => {
            let bytes = std::fs::read(input.path).unwrap_or_else(|error| {
                panic!("cannot open language pack {}: {error}", input.path)
            });
            decode_iso_8859_15(&bytes)
        }
    }
}

fn decode_iso_8859_15(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match byte {
            0xA4 => '\u{20ac}',
            0xA6 => '\u{0160}',
            0xA8 => '\u{0161}',
            0xB4 => '\u{017d}',
            0xB8 => '\u{017e}',
            0xBC => '\u{0152}',
            0xBD => '\u{0153}',
            0xBE => '\u{0178}',
            byte => char::from(*byte),
        })
        .collect()
}
