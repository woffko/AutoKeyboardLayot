//! Offline verification of a signed catalog and every referenced local artifact.
use autokeyboardlayot::{
    language_package::PackageTrust,
    package_catalog::{DEFAULT_PACKAGE_REPOSITORY, VerifiedReleaseCatalog},
};
use std::{
    io::Read,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > limit {
        return Err("invalid file type/size".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("file too large".into());
    }
    Ok(bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: verify_release_catalog CATALOG ARTIFACT_DIRECTORY".into());
    }
    let trust = PackageTrust::release()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let raw = bounded(Path::new(&args[0]), 1024 * 1024)?;
    let catalog =
        VerifiedReleaseCatalog::verify(&raw, &trust, DEFAULT_PACKAGE_REPOSITORY, None, now)?;
    let mut count = 0;
    for pin in catalog.packages() {
        let asset = pin.url().rsplit('/').next().ok_or("missing asset")?;
        let bytes = bounded(&Path::new(&args[1]).join(asset), pin.bytes())?;
        pin.verify_download(&bytes, &trust, now)?;
        count += 1;
    }
    println!(
        "RELEASE_CATALOG_AND_LOCAL_ASSETS_VERIFIED {count}; no network or historical checkpoint verification"
    );
    Ok(())
}
