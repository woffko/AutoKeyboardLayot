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
/// Default warning lead time: ten days, so a renewal can be reviewed and signed in time.
const DEFAULT_MIN_HOURS: u64 = 240;
/// Clients accept catalogs for at most 31 days, so a longer requirement can never be met.
const MAX_MIN_HOURS: u64 = 31 * 24;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !(1..=2).contains(&args.len()) {
        return Err(format!(
            "usage: check_catalog_expiry CATALOG [MIN_HOURS] \
             (fails when fewer than MIN_HOURS of validity remain; default {DEFAULT_MIN_HOURS}, at most {MAX_MIN_HOURS})"
        )
        .into());
    }
    let min_hours: u64 = match args.get(1) {
        Some(value) => value.to_str().ok_or("MIN_HOURS encoding")?.parse()?,
        None => DEFAULT_MIN_HOURS,
    };
    if min_hours > MAX_MIN_HOURS {
        return Err(format!("MIN_HOURS must be at most {MAX_MIN_HOURS}").into());
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
    if remaining < min_hours * 60 * 60 {
        return Err(format!(
            "signed catalog expires in {} hours (minimum {min_hours}); prepare a reviewed renewal",
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
