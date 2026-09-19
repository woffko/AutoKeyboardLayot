//! Offline candidate catalog construction; asset existence/publication is NOT checked.
use autokeyboardlayot::{
    language_package::{MAX_PACKAGE_BYTES, PackageTrust, VerifiedLanguagePackage},
    package_catalog::DEFAULT_PACKAGE_REPOSITORY,
    package_signing::{PreparedSigningInput, SigningKind},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        return Err("usage: prepare_release_catalog DIRECTORY TAG REVISION NEW_OUTPUT".into());
    }
    if Path::new(&args[3]).try_exists()? {
        return Err("output already exists".into());
    }
    let tag = args[1].to_str().ok_or("tag encoding")?;
    let revision: u64 = args[2].to_str().ok_or("revision encoding")?.parse()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let trust = PackageTrust::release()?;
    let mut paths = Vec::new();
    for (index, entry) in std::fs::read_dir(&args[0])?.enumerate() {
        if index >= 256 {
            return Err("directory entry limit".into());
        }
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "aklp") {
            paths.push(path);
        }
    }
    if paths.is_empty() || paths.len() > 64 {
        return Err("package count".into());
    }
    paths.sort();
    let mut packages = Vec::new();
    for path in paths {
        let file = std::fs::File::open(&path)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_PACKAGE_BYTES as u64 {
            return Err("package type/size".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_PACKAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let package = VerifiedLanguagePackage::verify(&bytes, &trust)?;
        let sha256 = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        packages.push(json!({"package_id":package.id().to_string(),"revision":package.revision(),
            "bytes":bytes.len(),"sha256":sha256,"tag":tag,"asset":path.file_name().and_then(|s| s.to_str()).ok_or("asset name")?,
            "runtime_api":package.runtime_api(),"input":package.input().is_some(),"ui_locale":package.ui_locale()}));
    }
    let catalog = json!({"format":1,"repository":DEFAULT_PACKAGE_REPOSITORY,"revision":revision,
        "issued_at":now,"expires_at":now.checked_add(7*24*60*60).ok_or("time overflow")?,"packages":packages}).to_string();
    let bytes = serde_json::to_vec(&json!({"format":1,"catalog":catalog}))?;
    let metadata: serde_json::Value =
        serde_json::from_slice(include_bytes!("../data/package-signing/public-key.json"))?;
    PreparedSigningInput::prepare(
        SigningKind::Catalog,
        metadata["signer"].as_str().ok_or("signer")?,
        &bytes,
        now,
    )?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[3])?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    println!(
        "UNSIGNED_CATALOG_VALIDATED {} packages; seven-day candidate, NOT published",
        packages.len()
    );
    Ok(())
}
