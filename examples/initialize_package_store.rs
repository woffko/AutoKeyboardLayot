//! Explicit developer fixture initialization; does not create application config.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err("usage: initialize_package_store NEW_ABSOLUTE_STORE_ROOT".into());
    }
    let store =
        autokeyboardlayot::package_store::PackageStore::initialize(std::path::Path::new(&args[0]))?;
    let snapshot = store.load(&autokeyboardlayot::language_package::PackageTrust::release()?)?;
    if snapshot.inventory().packages().count() != 0 {
        return Err("expected empty fixture".into());
    }
    println!("EMPTY_PACKAGE_STORE_INITIALIZED_NO_CONFIG");
    Ok(())
}
