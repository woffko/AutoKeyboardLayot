//! Explicit first-install profile creation, never a startup recovery fallback.
use crate::{
    ConfigurationDocument, configuration::load_configuration_snapshot,
    language_package::PackageTrust, package_store::PreparedStoreInitialization,
};
use std::{io, path::Path};

/// Returns true only when a new English-only profile was published. The caller
/// must hold its configuration writer lock and prevent application startup for
/// the entire call. `publish` must atomically publish to the supplied exact path.
/// Existing profiles (including legacy files) are not migrated or rewritten.
pub fn initialize_if_absent(
    directory: &Path,
    publish: impl FnOnce(&Path, &str) -> io::Result<()>,
) -> io::Result<bool> {
    if !directory.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "absolute profile directory required",
        ));
    }
    let loaded = load_configuration_snapshot(directory)?;
    if loaded.sources != Default::default() {
        #[cfg(not(feature = "legacy-bundled-input"))]
        if loaded.document.package_mode == crate::configuration::PackageMode::LegacyBootstrap
            && loaded
                .document
                .settings
                .enabled_input_packs
                .iter()
                .any(|id| *id != crate::Language::English)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "migrate selected legacy input packs before installing the modular base",
            ));
        }
        return Ok(false);
    }
    std::fs::create_dir_all(directory)?;
    let trust = PackageTrust::default();
    let prepared = PreparedStoreInitialization::from_files(&[], &Default::default(), &trust)
        .map_err(io::Error::other)?;
    prepared
        .confirm(&directory.join("packages"), &trust)
        .map_err(io::Error::other)?;
    let contents = ConfigurationDocument::english_only_managed()
        .to_text()
        .map_err(io::Error::other)?;
    publish(&directory.join("config.ini"), &contents)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn publish(path: &Path, contents: &str) -> io::Result<()> {
        let temporary = crate::configuration::prepare_configuration_write(path, contents)?;
        std::fs::rename(temporary, path)
    }
    #[test]
    fn first_install_is_english_only_and_repair_preserves_exact_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("new profile ü");
        assert!(initialize_if_absent(&root, publish).unwrap());
        let loaded = load_configuration_snapshot(&root).unwrap();
        assert_eq!(
            loaded.document,
            ConfigurationDocument::english_only_managed()
        );
        assert!(!loaded.document.settings.automatic_conversion_on_startup);
        let bytes = std::fs::read(root.join("config.ini")).unwrap();
        assert!(
            !initialize_if_absent(&root, |_, _| panic!("repair rewrote configuration")).unwrap()
        );
        assert_eq!(std::fs::read(root.join("config.ini")).unwrap(), bytes);
    }
    #[test]
    fn legacy_empty_files_and_malformed_profiles_are_not_reset() {
        for name in [
            "settings.ini",
            "user_dictionary.txt",
            "word_exclusions.txt",
            "exclusions.txt",
        ] {
            let temporary = tempfile::tempdir().unwrap();
            std::fs::write(temporary.path().join(name), "").unwrap();
            let result =
                initialize_if_absent(temporary.path(), |_, _| panic!("legacy profile rewritten"));
            #[cfg(feature = "legacy-bundled-input")]
            assert!(!result.unwrap());
            #[cfg(not(feature = "legacy-bundled-input"))]
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Unsupported);
            // Empty legacy files still imply legacy defaults. The English-only
            // installer must demand explicit migration, never rewrite them.
            assert_eq!(std::fs::read(temporary.path().join(name)).unwrap(), b"");
            assert!(!temporary.path().join("packages").exists());
            assert!(!temporary.path().join("config.ini").exists());
        }
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("config.ini");
        std::fs::write(&path, "malformed").unwrap();
        assert!(initialize_if_absent(temporary.path(), publish).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "malformed");
        assert!(!temporary.path().join("packages").exists());
    }
    #[test]
    fn final_write_failure_retries_without_resetting_a_partial_store() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(
            initialize_if_absent(temporary.path(), |_, _| Err(io::Error::other("injected")))
                .is_err()
        );
        assert!(!temporary.path().join("config.ini").exists());
        assert!(initialize_if_absent(temporary.path(), publish).unwrap());
        let partial = tempfile::tempdir().unwrap();
        std::fs::create_dir(partial.path().join("packages")).unwrap();
        assert!(initialize_if_absent(partial.path(), publish).is_err());
        assert!(!partial.path().join("config.ini").exists());
    }
}
