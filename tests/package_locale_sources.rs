//! Prevent optional source catalogs from silently falling behind the base UI.
use autokeyboardlayot::localization::CatalogRegistry;
use std::{collections::BTreeSet, path::Path};

#[test]
fn all_thirteen_optional_catalogs_are_complete_and_valid() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/package-locales");
    let expected: BTreeSet<String> = [
        "ar", "bn", "de", "es", "et", "fr", "hi", "id", "ja", "pt", "ru", "ur", "zh-Hans",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let files: BTreeSet<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect();
    assert_eq!(files, expected, "source locale membership changed");
    let mut registry = CatalogRegistry::default();
    let errors = registry.load_directory(&root);
    assert!(errors.is_empty(), "catalog validation: {errors:?}");
    let mut expected_runtime: BTreeSet<_> =
        expected.iter().map(|s| s.to_ascii_lowercase()).collect();
    expected_runtime.insert("en".to_owned());
    let actual: BTreeSet<_> = registry.locales().into_iter().map(str::to_owned).collect();
    assert_eq!(actual, expected_runtime);
    for locale in registry.locales() {
        assert!(
            registry.select(locale).missing_message_ids().is_empty(),
            "incomplete: {locale}"
        );
    }
}
