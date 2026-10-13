//! The configuration, packages and detector of the Linux engine, loaded from the XDG directories
//! with the shared loaders. The engine reloads when `config.ini` changes (the settings window saves
//! it atomically), so no channel between the two processes is needed.

use super::{diagnostics::Diagnostics, engine::Shared, keymap::Keymaps, paths};
use autokeyboardlayot::{
    Detector, DetectorConfig, Language, configuration::load_configuration_snapshot,
    installed_packages::InstalledPackages, language_package::PackageTrust,
};
use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

fn not_found(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{what} is unavailable (HOME is not set)"),
    )
}

pub fn config_path() -> Option<PathBuf> {
    paths::config_dir().map(|directory| directory.join("config.ini"))
}

/// The modification time of `config.ini`, or `None` when it does not exist yet.
pub fn config_stamp() -> Option<SystemTime> {
    config_path()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
}

/// Loads everything the engine needs from `config_dir` and `data_dir`.
pub fn load_shared_from(
    config_dir: &Path,
    data_dir: &Path,
    state_dir: &Path,
) -> io::Result<Shared> {
    let loaded = load_configuration_snapshot(config_dir)?;
    let document = loaded.document;
    let settings = document.settings.clone();
    let trust = PackageTrust::release().map_err(io::Error::other)?;
    let packages = InstalledPackages::load(
        &data_dir.join("packages"),
        document.package_mode,
        &trust,
        &settings.enabled_input_packs,
    )
    .map_err(io::Error::other)?;
    // The indicator menu speaks the language chosen in the settings.
    super::localization::initialize(&document.ui_language, packages.catalogs.as_deref());
    let mut detector = Detector::with_profile_selections(
        DetectorConfig {
            single_letter_words: settings.single_letter_words,
            ..Default::default()
        },
        packages.dictionaries,
        &document.input_profiles,
    );
    detector.replace_user_lexicons(document.user_lexicon(), document.word_exclusion_lexicon());
    detector.set_layout_model(
        settings
            .layout_model
            .then(autokeyboardlayot::layout_model::embedded)
            .flatten(),
    );
    // Every enabled language with an XKB layout is typed, whether or not it converts.
    let languages: Vec<Language> = settings.enabled_input_packs.iter().copied().collect();
    let keymaps = Keymaps::new(&languages).map_err(io::Error::other)?;
    let diagnostics = if settings.diagnostics_enabled {
        Diagnostics::to_file(state_dir.join("diagnostics.log"))
    } else {
        Diagnostics::disabled()
    };
    Shared::new(detector, keymaps, settings, diagnostics).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "no enabled language has a keyboard layout the engine can type in",
        )
    })
}

/// Loads from the user's XDG directories.
pub fn load_shared() -> io::Result<Shared> {
    let config = paths::config_dir().ok_or_else(|| not_found("the configuration directory"))?;
    let data = paths::data_dir().ok_or_else(|| not_found("the data directory"))?;
    let state = paths::state_dir().ok_or_else(|| not_found("the state directory"))?;
    load_shared_from(&config, &data, &state)
}

// These tests type Russian and Estonian, whose dictionaries the modular base downloads.
#[cfg(all(test, feature = "legacy-bundled-input"))]
mod tests {
    use super::*;

    #[test]
    fn a_missing_configuration_gives_the_default_languages_and_layouts() {
        let root = tempfile::tempdir().unwrap();
        let shared = load_shared_from(
            &root.path().join("config"),
            &root.path().join("data"),
            &root.path().join("state"),
        )
        .unwrap();
        assert_eq!(
            shared.layouts(),
            [Language::English, Language::Russian, Language::Estonian]
        );
        assert_eq!(shared.layout(), Language::English);
        assert!(shared.automatic());
        assert!(!shared.diagnostics.enabled());
    }

    #[test]
    fn the_configuration_chooses_the_languages_and_the_log() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        let mut document = autokeyboardlayot::ConfigurationDocument::default();
        document.settings.enabled_input_packs =
            [Language::English, Language::Russian].into_iter().collect();
        document.settings.diagnostics_enabled = true;
        document.settings.automatic_conversion_on_startup = false;
        std::fs::write(config.join("config.ini"), document.to_text().unwrap()).unwrap();
        let shared = load_shared_from(
            &config,
            &root.path().join("data"),
            &root.path().join("state"),
        )
        .unwrap();
        assert_eq!(shared.layouts(), [Language::English, Language::Russian]);
        assert!(!shared.automatic());
        assert!(shared.diagnostics.enabled());
    }
}
