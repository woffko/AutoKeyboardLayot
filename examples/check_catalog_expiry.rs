//! Read-only signed-catalog freshness gate. Never renews or publishes a catalog.
use autokeyboardlayot::{
    language_package::PackageTrust,
    package_catalog::{DEFAULT_PACKAGE_REPOSITORY, VerifiedReleaseCatalog},
};
use std::{
    io::Read,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_CATALOG_BYTES: usize = 1024 * 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err(
            "usage: check_catalog_expiry CATALOG (requires at least 72 hours remaining)".into(),
        );
    }
    let file = std::fs::File::open(&args[0])?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_CATALOG_BYTES as u64 {
        return Err("invalid catalog file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_CATALOG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let catalog = VerifiedReleaseCatalog::verify(
        &bytes,
        &PackageTrust::release()?,
        DEFAULT_PACKAGE_REPOSITORY,
        None,
        now,
    )?;
    let remaining = catalog.expires_at().saturating_sub(now);
    if remaining < 72 * 60 * 60 {
        return Err(format!(
            "signed catalog expires in {} hours; prepare a reviewed renewal",
            remaining / 3600
        )
        .into());
    }
    println!(
        "SIGNED_CATALOG_CURRENT remaining_hours={}",
        remaining / 3600
    );
    Ok(())
}
