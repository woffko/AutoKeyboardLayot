//! Offline release-trust verification; never installs or executes a package.
use autokeyboardlayot::language_package::{
    MAX_PACKAGE_BYTES, PackageTrust, VerifiedLanguagePackage,
};
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err("usage: verify_language_package FILE".into());
    }
    let file = std::fs::File::open(&args[0])?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_PACKAGE_BYTES as u64 {
        return Err("invalid input type/size".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PACKAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let package = VerifiedLanguagePackage::verify(&bytes, &PackageTrust::release()?)?;
    println!("RELEASE_SIGNATURE_VERIFIED {}", package.id());
    Ok(())
}
