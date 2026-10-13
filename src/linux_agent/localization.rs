//! The display language of the settings window and the indicator menu: the shared catalogs, the
//! user's choice in the configuration, and the system language from the locale environment.

use super::paths;
use autokeyboardlayot::localization::{
    CatalogRegistry, Localizer, TextDirection, UiLanguageChoice, UiLanguagePreference,
};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock, RwLock},
};

/// Where an installed copy keeps the translation catalogs shipped with it.
const SYSTEM_LOCALES: &str = "/usr/share/autokeyboardlayot/locales";

struct UiState {
    registry: CatalogRegistry,
    selected: Arc<Localizer>,
}

fn state() -> &'static RwLock<UiState> {
    static STATE: OnceLock<RwLock<UiState>> = OnceLock::new();
    STATE.get_or_init(|| {
        RwLock::new(UiState {
            registry: CatalogRegistry::default(),
            selected: Arc::new(Localizer::default()),
        })
    })
}

fn snapshot() -> Arc<Localizer> {
    Arc::clone(
        &state()
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .selected,
    )
}

/// The directories with translation catalogs: the user's data directory, then the installed copy,
/// then `locales` next to the executable (a build tree).
fn catalog_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(data) = paths::data_dir() {
        directories.push(data.join("locales"));
    }
    directories.push(PathBuf::from(SYSTEM_LOCALES));
    if let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
    {
        directories.push(directory.join("locales"));
    }
    directories
}

/// The system language as a BCP 47 tag, from the first of `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` and
/// `LANG` that names one: `ru_RU.UTF-8` becomes `ru-RU`.
pub fn system_language(lookup: &dyn Fn(&str) -> Option<String>) -> String {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(lookup)
        .filter_map(|value| {
            let first = value.split(':').next()?.trim().to_owned();
            let tag = first.split(['.', '@']).next()?.replace('_', "-");
            (!tag.is_empty() && tag != "C" && tag != "POSIX").then_some(tag)
        })
        .next()
        .unwrap_or_else(|| "en".to_owned())
}

/// Selects the catalog for the user's choice; `installed` comes from managed language packages.
pub fn initialize(preference: &UiLanguagePreference, installed: Option<&CatalogRegistry>) {
    let mut registry = installed.cloned().unwrap_or_default();
    if installed.is_none() {
        for directory in catalog_directories() {
            let _ = registry.load_directory(&directory);
        }
    }
    let system = system_language(&|name| std::env::var(name).ok());
    let selected = Arc::new(registry.select(preference.requested_locale(&system)));
    *state().write().unwrap_or_else(|error| error.into_inner()) = UiState { registry, selected };
}

/// Changes the chosen language, keeping the loaded catalogs.
pub fn apply_preference(preference: &UiLanguagePreference) {
    let system = system_language(&|name| std::env::var(name).ok());
    let mut state = state().write().unwrap_or_else(|error| error.into_inner());
    state.selected = Arc::new(state.registry.select(preference.requested_locale(&system)));
}

pub fn picker_choices(preference: &UiLanguagePreference) -> (Vec<UiLanguageChoice>, usize) {
    let state = state().read().unwrap_or_else(|error| error.into_inner());
    state
        .registry
        .picker_choices(preference, state.selected.text("ui_language.system"))
}

pub fn active_language_name() -> String {
    snapshot().language_name().to_owned()
}

pub fn is_rtl() -> bool {
    snapshot().direction() == TextDirection::Rtl
}

pub fn tr(id: &str) -> String {
    snapshot().text(id).to_owned()
}

pub fn tr_format(id: &str, arguments: &[(&str, &str)]) -> String {
    let localizer = snapshot();
    localizer
        .format(id, arguments)
        .unwrap_or_else(|_| localizer.text("text.unavailable").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_language_comes_from_the_locale_environment() {
        let environment = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(
            system_language(&environment(&[("LANG", "ru_RU.UTF-8")])),
            "ru-RU"
        );
        assert_eq!(
            system_language(&environment(&[
                ("LANGUAGE", "et:en"),
                ("LANG", "ru_RU.UTF-8")
            ])),
            "et"
        );
        assert_eq!(
            system_language(&environment(&[("LC_ALL", "C"), ("LANG", "de_DE@euro")])),
            "de-DE"
        );
        assert_eq!(system_language(&environment(&[])), "en");
    }
}
