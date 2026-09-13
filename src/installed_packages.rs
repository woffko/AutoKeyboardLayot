//! Installed-data projection used by startup/settings before worker handoff.
//! Loading never initializes a store and never mutates user selections.

use crate::{
    DictionaryRegistry, PackId,
    configuration::PackageMode,
    language_package::PackageTrust,
    localization::CatalogRegistry,
    package_store::{PackageStore, StoreError, StoreSnapshot},
};
use std::{collections::BTreeSet, path::Path, sync::Arc};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PackageSource {
    #[default]
    LegacyBootstrap,
    Managed {
        generation: u64,
        state_sha256: [u8; 32],
    },
}
impl PackageSource {
    /// A reload can skip committed generations, but cannot revert to bootstrap,
    /// an older generation, or a different state at the same generation.
    pub fn accepts_reload(self, next: Self) -> bool {
        match (self, next) {
            (Self::LegacyBootstrap, _) => true,
            (Self::Managed { .. }, Self::LegacyBootstrap) => false,
            (
                Self::Managed {
                    generation: old,
                    state_sha256: old_hash,
                },
                Self::Managed {
                    generation: new,
                    state_sha256: new_hash,
                },
            ) => new > old || (new == old && new_hash == old_hash),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstalledPackages {
    pub source: PackageSource,
    pub dictionaries: Arc<DictionaryRegistry>,
    /// None means the transitional loose-catalog loader is still in use.
    /// Some, even an empty English-only registry, excludes loose catalogs.
    pub catalogs: Option<Arc<CatalogRegistry>>,
}
impl InstalledPackages {
    pub fn load(
        root: &Path,
        mode: PackageMode,
        trust: &PackageTrust,
        selected: &BTreeSet<PackId>,
    ) -> Result<Self, StoreError> {
        match mode {
            PackageMode::LegacyBootstrap => Self::legacy(selected),
            PackageMode::Managed => {
                Self::from_store(&PackageStore::open(root)?.load(trust)?, selected)
            }
        }
    }

    pub fn legacy(selected: &BTreeSet<PackId>) -> Result<Self, StoreError> {
        #[cfg(not(feature = "legacy-bundled-input"))]
        if selected.iter().any(|id| *id != crate::Language::English) {
            return Err(StoreError::MissingMigrationInput);
        }
        let mut dictionaries = DictionaryRegistry::embedded();
        dictionaries
            .set_enabled(selected.iter().copied())
            .map_err(|_| StoreError::InvalidFile)?;
        Ok(Self {
            source: PackageSource::LegacyBootstrap,
            dictionaries: Arc::new(dictionaries),
            catalogs: None,
        })
    }

    pub fn from_store(
        snapshot: &StoreSnapshot,
        selected: &BTreeSet<PackId>,
    ) -> Result<Self, StoreError> {
        let inventory = snapshot.inventory();
        Ok(Self {
            source: PackageSource::Managed {
                generation: inventory.generation(),
                state_sha256: snapshot.state_sha256(),
            },
            dictionaries: Arc::new(
                inventory.dictionary_snapshot(&DictionaryRegistry::english_base(), selected)?,
            ),
            catalogs: Some(Arc::new(inventory.catalog_snapshot()?)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn persisted_managed_intent_cannot_fall_back_after_a_restart_or_store_disappearance() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("packages");
        let path = temporary.path().join("config.ini");
        let document = crate::ConfigurationDocument::english_only_managed();
        fs::write(&path, document.to_text().unwrap()).unwrap();
        let reload = || {
            let document =
                crate::configuration::load_configuration_directory(temporary.path()).unwrap();
            InstalledPackages::load(
                &root,
                document.package_mode,
                &PackageTrust::default(),
                &document.settings.enabled_input_packs,
            )
        };
        assert!(reload().is_err());
        PackageStore::initialize(&root).unwrap();
        assert_eq!(reload().unwrap().dictionaries.installed_ids().count(), 1);
        fs::rename(&root, temporary.path().join("preserved-store")).unwrap();
        assert!(reload().is_err()); // fresh config read; no in-memory source guard
        // A prepared/failed migration directory is not authority to activate it.
        fs::create_dir(&root).unwrap();
        let legacy = InstalledPackages::load(
            &root,
            PackageMode::LegacyBootstrap,
            &PackageTrust::default(),
            &document.settings.enabled_input_packs,
        )
        .unwrap();
        assert_eq!(legacy.source, PackageSource::LegacyBootstrap);
        assert_eq!(
            legacy.dictionaries.installed_ids().count(),
            if cfg!(feature = "legacy-bundled-input") {
                3
            } else {
                1
            }
        );
        assert!(reload().is_err());
    }

    #[test]
    fn managed_empty_store_uses_only_base_and_retains_missing_selections() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("packages");
        let selected: BTreeSet<_> = ["en-US", "ru-RU", "et-EE", "missing-pack"]
            .into_iter()
            .map(|id| PackId::parse(id).unwrap())
            .collect();
        let legacy = InstalledPackages::load(
            &root,
            PackageMode::LegacyBootstrap,
            &PackageTrust::default(),
            &selected,
        );
        #[cfg(feature = "legacy-bundled-input")]
        assert_eq!(legacy.unwrap().dictionaries.installed_ids().count(), 3);
        #[cfg(not(feature = "legacy-bundled-input"))]
        assert!(matches!(legacy, Err(StoreError::MissingMigrationInput)));
        assert!(!root.exists());
        let store = PackageStore::initialize(&root).unwrap();
        let loaded = InstalledPackages::load(
            &root,
            PackageMode::Managed,
            &PackageTrust::default(),
            &selected,
        )
        .unwrap();
        assert_eq!(
            loaded
                .dictionaries
                .installed_ids()
                .copied()
                .collect::<Vec<_>>(),
            vec![crate::Language::English]
        );
        assert_eq!(
            loaded
                .dictionaries
                .enabled_ids()
                .copied()
                .collect::<BTreeSet<_>>(),
            selected
        );
        assert!(loaded.catalogs.is_some());
        assert!(PackageSource::LegacyBootstrap.accepts_reload(loaded.source));
        fs::remove_file(root.join("CURRENT")).unwrap();
        assert!(
            InstalledPackages::load(
                &root,
                PackageMode::Managed,
                &PackageTrust::default(),
                &selected
            )
            .is_err()
        );
        assert!(store.load(&PackageTrust::default()).is_err());
        assert!(!loaded.source.accepts_reload(PackageSource::LegacyBootstrap));
    }

    #[test]
    fn source_reload_rejects_downgrades_and_forks_but_allows_skipped_generations() {
        let old = PackageSource::Managed {
            generation: 4,
            state_sha256: [1; 32],
        };
        assert!(old.accepts_reload(old));
        assert!(!old.accepts_reload(PackageSource::LegacyBootstrap));
        assert!(!old.accepts_reload(PackageSource::Managed {
            generation: 3,
            state_sha256: [1; 32]
        }));
        assert!(!old.accepts_reload(PackageSource::Managed {
            generation: 4,
            state_sha256: [2; 32]
        }));
        assert!(old.accepts_reload(PackageSource::Managed {
            generation: 7,
            state_sha256: [2; 32]
        }));
    }
}
