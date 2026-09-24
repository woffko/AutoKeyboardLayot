use std::fs::File;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

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
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_LEGACY_BUNDLED_INPUT");
    let legacy = std::env::var_os("CARGO_FEATURE_LEGACY_BUNDLED_INPUT").is_some();
    for pack in PACKS {
        if !legacy && !matches!(pack.id, "en-US" | "en-US-short") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", pack.path);
        build_dictionary(*pack, &output);
    }
    println!(
        "cargo:rustc-env=AUTOKEY_BUILD_COMMIT={}",
        build_commit(Path::new(env!("CARGO_MANIFEST_DIR")))
    );
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=ui/settings.slint");
        slint_build::compile("ui/settings.slint").expect("cannot compile settings UI");
        // Embed the reviewed application icon resource into every executable
        // target. The tray icon is drawn separately and is not affected.
        let resource = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/app.res");
        println!("cargo:rerun-if-changed=assets/app.res");
        if resource.is_file() {
            println!("cargo:rustc-link-arg-bins={}", resource.display());
        } else {
            println!("cargo:warning=assets/app.res is missing; executables have no icon");
        }
    }
}

/// Short source commit shown next to the package version. Builds outside a Git
/// checkout can supply it through `AKL_BUILD_COMMIT`; otherwise it is unknown.
fn build_commit(manifest: &Path) -> String {
    println!("cargo:rerun-if-env-changed=AKL_BUILD_COMMIT");
    if let Ok(commit) = std::env::var("AKL_BUILD_COMMIT")
        && !commit.trim().is_empty()
    {
        return commit.trim().to_owned();
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(manifest)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    let mut watched = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={}", manifest.join(path).display());
        }
    }
    git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".to_owned())
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
        let minimum = if matches!(input.kind, InputKind::Plain) {
            1
        } else {
            2
        };
        if normalized.chars().count() >= minimum && normalized.chars().all(char::is_alphabetic) {
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
