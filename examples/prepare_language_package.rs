//! Build an unsigned package from an explicit local recipe; no keys or network.
use autokeyboardlayot::package_signing::{PreparedSigningInput, SigningKind};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    package_id: String,
    revision: u64,
    input_pack: Option<String>,
    ui_locale: Option<String>,
    // Explicit local file paths, relative to the recipe's directory.
    components: BTreeMap<String, String>,
}

fn read(path: &Path, limit: usize) -> Result<String, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > limit as u64 {
        return Err("source must be a bounded regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err("source too large".into());
    }
    Ok(String::from_utf8(bytes)?)
}

fn prepare(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let recipe: Recipe = serde_json::from_str(&read(path, 32768)?)?;
    let base = path.parent().ok_or("recipe directory")?;
    let mut components = BTreeMap::new();
    let mut integrity = BTreeMap::new();
    for (role, source) in recipe.components {
        let maximum = match role.as_str() {
            "words" => 32 * 1024 * 1024,
            "short_words" => 16 * 1024 * 1024,
            "scoring" => 64 * 1024,
            "input" => 16 * 1024,
            "ui" | "license" | "notice" => 512 * 1024,
            _ => return Err("unknown component role".into()),
        };
        let content = read(&base.join(source), maximum)?;
        let digest = Sha256::digest(content.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        integrity.insert(role.clone(), json!({"bytes":content.len(),"sha256":digest}));
        components.insert(role, content);
    }
    let manifest = json!({"format":1,"package_id":recipe.package_id,"revision":recipe.revision,
        "runtime_api":1,"input_pack":recipe.input_pack,"ui_locale":recipe.ui_locale,"components":integrity}).to_string();
    let bytes =
        serde_json::to_vec(&json!({"format":1,"manifest":manifest,"components":components}))?;
    let metadata: serde_json::Value =
        serde_json::from_slice(include_bytes!("../data/package-signing/public-key.json"))?;
    let signer = metadata["signer"].as_str().ok_or("embedded signer")?;
    PreparedSigningInput::prepare(SigningKind::Package, signer, &bytes, 0)?;
    Ok(bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: prepare_language_package RECIPE NEW_UNSIGNED_OUTPUT".into());
    }
    let bytes = prepare(Path::new(&args[0]))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    println!(
        "UNSIGNED_PACKAGE_VALIDATED {} bytes; not signed, installed or published",
        bytes.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_exact_content_and_requires_licenses_and_matching_hashes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ui.json"), r#"{"format":1,"locale":"ru","direction":"ltr","messages":{"locale.self_name":"Русский"}}"#).unwrap();
        std::fs::write(
            dir.path().join("license.txt"),
            "Public synthetic test fixture, not a release license.\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notice.txt"), "Synthetic test fixture.\n").unwrap();
        let mut recipe = json!({"package_id":"ru-RU","revision":1,"ui_locale":"ru","components":{
            "ui":"ui.json","license":"license.txt","notice":"notice.txt"}});
        let path = dir.path().join("recipe.json");
        std::fs::write(&path, recipe.to_string()).unwrap();
        let bytes = prepare(&path).unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            envelope["components"]["notice"],
            "Synthetic test fixture.\n"
        );
        assert!(envelope.get("signature").is_none());
        recipe["components"]
            .as_object_mut()
            .unwrap()
            .remove("license");
        std::fs::write(&path, recipe.to_string()).unwrap();
        assert!(prepare(&path).is_err());
        recipe["components"]["script"] = json!("notice.txt");
        std::fs::write(&path, recipe.to_string()).unwrap();
        assert!(prepare(&path).is_err());
    }
}
