#![cfg(not(feature = "legacy-bundled-input"))]

use autokeyboardlayot::{
    ConfigurationDocument, DictionaryRegistry, Language, installed_packages::InstalledPackages,
    installer_profile::initialize_if_absent,
};

fn publish(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let temporary = autokeyboardlayot::configuration::prepare_configuration_write(path, text)?;
    std::fs::rename(temporary, path)
}

#[test]
fn modular_binary_has_only_the_english_embedded_pack() {
    let registry = DictionaryRegistry::embedded();
    assert_eq!(
        registry.installed_ids().copied().collect::<Vec<_>>(),
        vec![Language::English]
    );
    assert!(
        registry
            .active(&Language::English)
            .unwrap()
            .input_descriptor()
            .is_some()
    );
    assert!(registry.active(&Language::Russian).is_none());
    assert!(registry.active(&Language::Estonian).is_none());
    assert!(InstalledPackages::legacy(&[Language::Russian].into()).is_err());
}

#[test]
fn clean_base_initializes_but_never_rewrites_a_legacy_upgrade() {
    let fresh = tempfile::tempdir().unwrap();
    assert!(initialize_if_absent(fresh.path(), publish).unwrap());
    let document =
        autokeyboardlayot::configuration::load_configuration_directory(fresh.path()).unwrap();
    assert_eq!(document, ConfigurationDocument::english_only_managed());
    let legacy = tempfile::tempdir().unwrap();
    let original = ConfigurationDocument::default().to_text().unwrap();
    std::fs::write(legacy.path().join("config.ini"), &original).unwrap();
    assert_eq!(
        initialize_if_absent(legacy.path(), publish)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
    assert_eq!(
        std::fs::read_to_string(legacy.path().join("config.ini")).unwrap(),
        original
    );
    assert!(!legacy.path().join("packages").exists());
}
