//! Settings projection, not an assertion about the live Windows worker.

use crate::{Detector, DictionaryError, DictionaryRegistry, PackId};
use std::{collections::BTreeSet, sync::Arc};

pub const MAX_SELECTION_ROWS: usize = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionStatus {
    MissingData,
    Disabled,
    Unavailable,
    Conservative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPackSelection {
    pub id: PackId,
    pub selected: bool,
    pub status: SelectionStatus,
    pub missing_capabilities: Vec<String>,
    pub profiles: Vec<crate::WindowsKeyboardProfile>,
    pub chosen_profile: Option<crate::WindowsKeyboardProfile>,
}

/// Validate a complete UI projection before replacing a saved selection. Count
/// every row, including unchecked entries; never silently drop invalid IDs.
pub fn parse_selection<S: AsRef<str>>(
    rows: impl IntoIterator<Item = (S, bool)>,
) -> Result<BTreeSet<PackId>, DictionaryError> {
    let mut seen = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for (index, (id, enabled)) in rows.into_iter().enumerate() {
        if index >= MAX_SELECTION_ROWS {
            return Err(DictionaryError::LimitExceeded);
        }
        let id = PackId::parse(id.as_ref())?;
        if !seen.insert(id) {
            return Err(DictionaryError::DuplicateId);
        }
        if enabled {
            selected.insert(id);
            if selected.len() > 64 {
                return Err(DictionaryError::LimitExceeded);
            }
        }
    }
    Ok(selected)
}

/// Include installed data and retained missing selections, never a static list
/// of languages. Construct outside hooks; no filesystem or OS probe is used.
pub fn selection_rows(
    installed: &DictionaryRegistry,
    selected: &BTreeSet<PackId>,
) -> Result<Vec<InputPackSelection>, DictionaryError> {
    selection_rows_with_profiles(installed, selected, &Default::default())
}

/// Use the same immutable profile preference resolution as the worker detector.
pub fn selection_rows_with_profiles(
    installed: &DictionaryRegistry,
    selected: &BTreeSet<PackId>,
    profiles: &crate::input_profile_selection::InputProfileSelections,
) -> Result<Vec<InputPackSelection>, DictionaryError> {
    let installed_ids: BTreeSet<_> = installed.installed_ids().copied().collect();
    let ids: BTreeSet<_> = installed_ids
        .union(selected)
        .copied()
        .chain(profiles.iter().map(|(id, _)| *id))
        .collect();
    let mut snapshot = installed.clone();
    snapshot.set_enabled(selected.iter().copied())?;
    let detector =
        Detector::with_profile_selections(Default::default(), Arc::new(snapshot), profiles);
    Ok(ids
        .into_iter()
        .map(|id| {
            let chosen_profile = profiles.get(&id);
            let mut available: BTreeSet<_> = installed
                .installed(&id)
                .and_then(|pack| pack.input_descriptor())
                .into_iter()
                .flat_map(|descriptor| descriptor.profiles().map(|(profile, _)| *profile))
                .collect();
            available.extend(chosen_profile);
            let selected = selected.contains(&id);
            let status = if !installed_ids.contains(&id) {
                SelectionStatus::MissingData
            } else if !selected {
                SelectionStatus::Disabled
            } else if detector.input_scope(id).is_some() {
                SelectionStatus::Conservative
            } else {
                SelectionStatus::Unavailable
            };
            let missing_capabilities = detector
                .input_profile(id)
                .and_then(|profile| detector.profile_implementation(id, profile))
                .and_then(|status| match status {
                    crate::input_capabilities::ProfileImplementation::MissingCapabilities(
                        missing,
                    ) => Some(missing.into_iter().collect()),
                    _ => None,
                })
                .unwrap_or_default();
            InputPackSelection {
                id,
                selected,
                status,
                missing_capabilities,
                profiles: available.into_iter().collect(),
                chosen_profile,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_disabled_missing_profiles_and_stale_variants_visible() {
        use crate::input_profile_selection::InputProfileSelections;
        let profiles = InputProfileSelections::parse([
            ("en-US", "0409:00020409"),
            ("custom-missing", "0419:00000419"),
        ])
        .unwrap();
        let registry = crate::test_support::registry();
        let selected = BTreeSet::from([crate::Language::English]);
        let rows = selection_rows_with_profiles(&registry, &selected, &profiles).unwrap();
        let en = rows
            .iter()
            .find(|row| row.id == crate::Language::English)
            .unwrap();
        assert_eq!(en.status, SelectionStatus::Unavailable);
        assert_eq!(en.profiles.len(), 2);
        assert!(en.profiles.contains(&en.chosen_profile.unwrap()));
        let missing = rows
            .iter()
            .find(|row| row.id.as_str() == "custom-missing")
            .unwrap();
        assert!(!missing.selected);
        assert_eq!(missing.status, SelectionStatus::MissingData);
        assert_eq!(missing.profiles, [missing.chosen_profile.unwrap()]);
        let recovered = InputProfileSelections::parse(
            rows.iter()
                .filter_map(|row| row.chosen_profile.map(|p| (row.id.as_str(), p.to_string()))),
        )
        .unwrap();
        assert_eq!(profiles, recovered);
        let cleared =
            selection_rows_with_profiles(&registry, &selected, &Default::default()).unwrap();
        assert_eq!(
            cleared
                .iter()
                .find(|row| row.id == crate::Language::English)
                .unwrap()
                .status,
            SelectionStatus::Conservative
        );
        assert!(
            !cleared
                .iter()
                .any(|row| row.id.as_str() == "custom-missing")
        );
    }

    #[test]
    fn projection_bounds_include_disjoint_installed_enabled_and_profile_ids() {
        let mut registry = DictionaryRegistry::default();
        for index in 0..64 {
            registry
                .insert(
                    crate::DictionaryPack::from_words(
                        PackId::parse(&format!("data-{index}")).unwrap(),
                        ["hello"],
                        [],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let selected: BTreeSet<_> = (0..64)
            .map(|i| PackId::parse(&format!("selected-{i}")).unwrap())
            .collect();
        let profiles = crate::input_profile_selection::InputProfileSelections::parse(
            (0..64).map(|i| (format!("profile-{i}"), "0409:00000409")),
        )
        .unwrap();
        let rows = selection_rows_with_profiles(&registry, &selected, &profiles).unwrap();
        assert_eq!(rows.len(), MAX_SELECTION_ROWS);
        assert_eq!(
            parse_selection(rows.iter().map(|row| (row.id.as_str(), row.selected))).unwrap(),
            selected
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.chosen_profile.is_some())
                .count(),
            64
        );
    }

    #[test]
    fn selection_roundtrip_preserves_missing_choices_until_explicit_deselection() {
        let missing = PackId::parse("custom-missing").unwrap();
        let selected = BTreeSet::from([crate::Language::Russian, missing]);
        let rows = selection_rows(&crate::test_support::registry(), &selected).unwrap();
        assert_eq!(
            parse_selection(rows.iter().map(|row| (row.id.as_str(), row.selected))).unwrap(),
            selected
        );
        let next = parse_selection(
            rows.iter()
                .map(|row| (row.id.as_str(), row.selected && row.id != missing)),
        )
        .unwrap();
        assert_eq!(next, BTreeSet::from([crate::Language::Russian]));
        assert!(selected.contains(&missing));
        assert!(
            parse_selection(rows.iter().map(|row| (row.id.as_str(), false)))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn selection_rejects_invalid_and_duplicate_ids_even_when_unchecked() {
        for id in ["", "../en", "en_US", "en--us"] {
            assert_eq!(
                parse_selection([(id, false)]),
                Err(DictionaryError::InvalidId)
            );
        }
        assert_eq!(
            parse_selection([("EN-us", false), ("en-US", true)]),
            Err(DictionaryError::DuplicateId)
        );
    }

    #[test]
    fn selection_limits_count_raw_rows_and_enabled_choices() {
        let rows = |count, enabled| {
            (0..count).map(move |index| (format!("custom-{index}"), index < enabled))
        };
        assert_eq!(parse_selection(rows(192, 64)).unwrap().len(), 64);
        assert_eq!(
            parse_selection(rows(193, 0)),
            Err(DictionaryError::LimitExceeded)
        );
        assert_eq!(
            parse_selection(rows(65, 65)),
            Err(DictionaryError::LimitExceeded)
        );
    }

    #[test]
    fn projection_retains_missing_choices_and_does_not_enable_installed_packs() {
        let missing = PackId::parse("custom-missing").unwrap();
        let selected = BTreeSet::from([missing, crate::Language::Estonian]);
        let registry = crate::test_support::registry();
        let rows = selection_rows(&registry, &selected).unwrap();
        assert_eq!(rows.len(), 4);
        let row = rows.iter().find(|row| row.id == missing).unwrap();
        assert!(row.selected);
        assert_eq!(row.status, SelectionStatus::MissingData);
        let row = rows
            .iter()
            .find(|row| row.id == crate::Language::English)
            .unwrap();
        assert!(!row.selected);
        assert_eq!(row.status, SelectionStatus::Disabled);
        let row = rows
            .iter()
            .find(|row| row.id == crate::Language::Estonian)
            .unwrap();
        assert_eq!(row.status, SelectionStatus::Conservative);
        assert_eq!(row.missing_capabilities, ["altgr-v1", "dead-key-v1"]);
        assert_eq!(selected.len(), 2);
        assert!(registry.active(&crate::Language::English).is_some());
    }

    #[test]
    fn installed_data_without_an_adapter_is_visible_but_unavailable() {
        let id = PackId::parse("custom-data").unwrap();
        let mut registry = DictionaryRegistry::default();
        registry
            .insert(crate::DictionaryPack::from_words(id, ["hello"], []).unwrap())
            .unwrap();
        let rows = selection_rows(&registry, &BTreeSet::from([id])).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, SelectionStatus::Unavailable);
        assert!(rows[0].selected);
        assert!(
            selection_rows(&registry, &BTreeSet::new())
                .unwrap()
                .iter()
                .all(|row| !row.selected)
        );
    }
}
