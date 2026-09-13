//! Read-only model for the installer's package-selection page.
//! No settings singleton, downloads, store writes or input activation.
use crate::{
    PackId,
    language_package::PackageTrust,
    package_install::{InstallError, PreparedCatalog, SelectedDownload},
};
use std::collections::BTreeSet;

pub struct PackageRow {
    pub id: PackId,
    pub revision: u64,
    pub bytes: u64,
    pub ui_locale: Option<String>,
    pub includes_input: bool,
    pub compatible: bool,
    pub selected: bool,
}

/// Owns the authenticated snapshot for the whole page lifetime. Refresh creates
/// a new model with no selection; it never silently carries approval forward.
pub struct InstallerPackageSelection {
    catalog: PreparedCatalog,
    selected: BTreeSet<PackId>,
}
impl InstallerPackageSelection {
    pub fn replace_selection(
        &mut self,
        ids: BTreeSet<PackId>,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<(), InstallError> {
        if !ids.is_empty() {
            self.catalog.select(&ids, trust, now)?;
        }
        self.selected = ids;
        Ok(())
    }
    pub fn new(catalog: PreparedCatalog) -> Self {
        Self {
            catalog,
            selected: BTreeSet::new(),
        }
    }
    pub fn rows(&self) -> Vec<PackageRow> {
        self.catalog
            .packages()
            .map(|pin| PackageRow {
                id: pin.id(),
                revision: pin.revision(),
                bytes: pin.bytes(),
                ui_locale: pin.ui_locale().map(str::to_owned),
                includes_input: pin.includes_input(),
                compatible: pin.compatible() && !crate::package_inventory::is_base(pin.id()),
                selected: self.selected.contains(&pin.id()),
            })
            .collect()
    }
    pub fn total_bytes(&self) -> u64 {
        // At most 64 bounded catalog records; selections additionally pass the
        // core 512 MiB whole-plan quota before becoming visible.
        self.catalog
            .packages()
            .filter(|p| self.selected.contains(&p.id()))
            .map(|p| p.bytes())
            .sum()
    }
    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// ASCII-only data bridge for Inno's INI reader. Text labels belong to its
    /// localization catalog, never to executable directives from release data.
    /// This display snapshot is NOT an authorization or an install manifest.
    pub fn page_data(&self) -> Result<String, InstallError> {
        let rows = self.rows();
        let mut result = format!(
            "[catalog]\r\nformat=1\r\ncount={}\r\nselected={}\r\ntotal_bytes={}\r\n",
            rows.len(),
            self.selected_count(),
            self.total_bytes()
        );
        for (index, row) in rows.iter().enumerate() {
            let id = row.id.to_string();
            let locale = row.ui_locale.as_deref().unwrap_or("");
            // Keep each value a single unquoted token even if ID rules evolve.
            if [&id, locale].iter().any(|value| {
                !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }) {
                return Err(crate::package_catalog::ReleaseError::InvalidData.into());
            }
            result.push_str(&format!("\r\n[package{index}]\r\nid={id}\r\nrevision={}\r\nbytes={}\r\nui_locale={locale}\r\ninput={}\r\ncompatible={}\r\nselected={}\r\n", row.revision, row.bytes, u8::from(row.includes_input), u8::from(row.compatible), u8::from(row.selected)));
        }
        Ok(result)
    }

    /// Reject the entire checkbox change if unavailable/stale/incompatible.
    /// Failure leaves the preceding selection and displayed total untouched.
    pub fn set_selected(
        &mut self,
        id: PackId,
        selected: bool,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<(), InstallError> {
        let mut candidate = self.selected.clone();
        if selected {
            candidate.insert(id);
        } else {
            candidate.remove(&id);
        }
        if !candidate.is_empty() {
            self.catalog.select(&candidate, trust, now)?;
        }
        self.selected = candidate;
        Ok(())
    }

    /// Call only after the user confirms download. Empty means English-only:
    /// no download transaction and no catalog receipt/store mutation.
    /// The returned transaction still requires separate installation review.
    pub fn confirm_download(
        self,
        trust: &PackageTrust,
        now: u64,
    ) -> Result<Option<SelectedDownload>, InstallError> {
        if self.selected.is_empty() {
            return Ok(None);
        }
        self.catalog.select(&self.selected, trust, now).map(Some)
    }
}
