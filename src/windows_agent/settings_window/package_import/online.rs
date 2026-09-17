//! Network callbacks share the local manager's one-worker/confirmation state.
use super::*;
use autokeyboardlayot::{
    package_download::DownloadError, package_install::InstallError, package_store::StoreError,
};
use std::{
    sync::Mutex,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub(super) struct Progress {
    current: Mutex<(usize, u64)>,
    total: usize,
    total_bytes: u64,
}
pub(super) fn now() -> Result<u64, DownloadError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| DownloadError::Clock)
}
fn managed_store() -> Result<PackageStore, InstallError> {
    let document =
        super::super::super::try_load_configuration_document().map_err(StoreError::Io)?;
    if document.package_mode != PackageMode::Managed {
        return Err(autokeyboardlayot::package_inventory::InventoryError::InvalidState.into());
    }
    let directory =
        super::super::super::configuration_directory().ok_or(StoreError::InvalidFile)?;
    Ok(PackageStore::open(&directory.join("packages"))?)
}

fn begin(
    ui: &SettingsWindow,
    state: &mut State,
    work: impl FnOnce(&AtomicBool) -> Result<Completed, InstallError> + Send + 'static,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancel = Some(cancel.clone());
    ui.set_download_active(true);
    ui.set_download_cancelling(false);
    ui.set_status_error(false);
    ui.set_status_text(tr("import.busy").into());
    start(ui, state, move || {
        Ok(work(&cancel).unwrap_or_else(|error| Completed::OnlineFailure(error.to_string())))
    });
    if state.receiver.is_none() {
        state.cancel = None;
        state.progress = None;
        ui.set_download_active(false);
    }
}

pub(super) fn clear_catalog(ui: &SettingsWindow, state: &mut State) {
    state.catalog = None;
    ui.set_download_catalog_ready(false);
    ui.set_download_has_selection(false);
    ui.set_download_rows(ModelRc::new(VecModel::from(Vec::<DownloadRow>::new())));
    ui.set_download_summary(SharedString::default());
}

fn selected(ui: &SettingsWindow) -> Result<BTreeSet<autokeyboardlayot::PackId>, InstallError> {
    let rows = ui.get_download_rows();
    if rows.row_count() > 64 {
        return Err(autokeyboardlayot::package_catalog::ReleaseError::TooLarge.into());
    }
    let mut ids = BTreeSet::new();
    for row in rows.iter().filter(|row| row.selected) {
        let id = autokeyboardlayot::PackId::parse(row.id.as_str())
            .map_err(|_| autokeyboardlayot::package_catalog::ReleaseError::InvalidData)?;
        if !row.available || !ids.insert(id) {
            return Err(autokeyboardlayot::package_catalog::ReleaseError::InvalidData.into());
        }
    }
    Ok(ids)
}

fn summary(ui: &SettingsWindow, catalog: &PreparedCatalog) {
    let ids = selected(ui).unwrap_or_default();
    let bytes: u64 = catalog
        .packages()
        .filter(|pin| ids.contains(&pin.id()))
        .map(|pin| pin.bytes())
        .sum();
    ui.set_download_has_selection(
        !ids.is_empty()
            && ids.len() <= autokeyboardlayot::package_inventory::MAX_OPTIONAL_PACKAGES
            && bytes <= autokeyboardlayot::package_catalog::MAX_SELECTED_DOWNLOAD_BYTES,
    );
    ui.set_download_summary(
        tr_format(
            "download.selection",
            &[
                ("count", &ids.len().to_string()),
                ("bytes", &bytes.to_string()),
            ],
        )
        .into(),
    );
}

fn rows(
    ui: &SettingsWindow,
    catalog: &PreparedCatalog,
    selected: &BTreeSet<autokeyboardlayot::PackId>,
) {
    let rows = catalog
        .packages()
        .map(|pin| DownloadRow {
            id: pin.id().as_str().into(),
            details: tr_format(
                "download.row",
                &[
                    ("revision", &pin.revision().to_string()),
                    ("bytes", &pin.bytes().to_string()),
                    ("locale", pin.ui_locale().unwrap_or("—")),
                ],
            )
            .into(),
            has_input: pin.includes_input(),
            available: pin.compatible() && pin.id() != autokeyboardlayot::Language::English,
            selected: selected.contains(&pin.id()),
        })
        .collect::<Vec<_>>();
    ui.set_download_rows(ModelRc::new(VecModel::from(rows)));
    summary(ui, catalog);
}

pub(super) fn show_catalog(ui: &SettingsWindow, state: &mut State, catalog: Box<PreparedCatalog>) {
    ui.set_download_repository(catalog.repository().into());
    rows(ui, &catalog, &BTreeSet::new());
    state.catalog = Some(Arc::from(catalog));
    ui.set_download_catalog_ready(true);
    ui.set_status_error(false);
    ui.set_status_text(SharedString::default());
}

fn review_text(ui: &SettingsWindow, prepared: &PreparedOnlineInstall) {
    ui.set_online_summary(
        tr_format(
            "download.install_summary",
            &[
                ("count", &prepared.packages().len().to_string()),
                ("bytes", &prepared.plan().total_bytes().to_string()),
            ],
        )
        .into(),
    );
    let index = usize::try_from(ui.get_review_package_index()).unwrap_or(0);
    if let Some(package) = prepared.packages().get(index) {
        ui.set_package_import_preview(package_preview(package.package()).into());
    }
}

pub(super) fn show_prepared(
    ui: &SettingsWindow,
    state: &mut State,
    prepared: Box<PreparedOnlineInstall>,
) {
    clear_catalog(ui, state);
    let names: Vec<_> = prepared
        .packages()
        .iter()
        .map(|p| format!("{} / {}", p.package().id().as_str(), p.package().revision()))
        .collect();
    ui.set_review_package_names(strings_model(names.iter().map(String::as_str)));
    ui.set_review_package_index(0);
    review_text(ui, &prepared);
    state.pending = Some(Pending::Online(prepared));
    ui.set_package_migration(false);
    ui.set_package_removal(false);
    ui.set_package_rollback(false);
    ui.set_package_online(true);
    ui.set_package_import_ready(true);
    ui.set_status_error(false);
    ui.set_status_text(SharedString::default());
}

pub(super) fn poll_progress(ui: &SettingsWindow, state: &mut State) {
    if ui.get_download_cancelling() {
        return;
    }
    let Some(progress) = &state.progress else {
        return;
    };
    let Ok(current) = progress.current.try_lock().map(|value| *value) else {
        return;
    };
    if state.last_progress == Some(current) {
        return;
    }
    state.last_progress = Some(current);
    ui.set_status_text(
        tr_format(
            "download.progress",
            &[
                ("count", &current.0.to_string()),
                ("total", &progress.total.to_string()),
                ("bytes", &current.1.to_string()),
                ("total_bytes", &progress.total_bytes.to_string()),
            ],
        )
        .into(),
    );
}

pub(super) fn wire(ui: &SettingsWindow, state: Rc<RefCell<State>>) {
    let weak = ui.as_weak();
    let checking = state.clone();
    ui.on_check_download_catalog(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = checking.borrow_mut();
        if !ui.get_package_import_allowed()
            || ui.get_package_import_busy()
            || ui.get_package_import_ready()
            || state.receiver.is_some()
        {
            return;
        }
        let requested = ui.get_download_repository().trim().to_owned();
        clear_catalog(&ui, &mut state);
        begin(&ui, &mut state, move |cancel| {
            let trust = PackageTrust::release()
                .map_err(autokeyboardlayot::package_catalog::ReleaseError::from)?;
            let store = managed_store()?;
            let repository = if requested.is_empty() {
                let snapshot = store.load(&trust)?;
                autokeyboardlayot::package_catalog::resolve_repository_source(
                    "",
                    snapshot.inventory().repository(),
                )?
            } else {
                requested
            };
            let catalog = PreparedCatalog::from_github(&store, &repository, &trust, cancel)?;
            Ok(Completed::Catalog(Box::new(catalog)))
        });
    });

    let weak = ui.as_weak();
    let toggling = state.clone();
    ui.on_toggle_download(move |id, enabled| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_package_import_busy() || ui.get_package_import_ready() {
            return;
        }
        let state = toggling.borrow();
        let Some(catalog) = &state.catalog else {
            return;
        };
        let model = ui.get_download_rows();
        let Some((index, mut row)) = model
            .iter()
            .enumerate()
            .find(|(_, row)| row.id == id && row.available)
        else {
            return;
        };
        row.selected = enabled;
        model.set_row_data(index, row);
        summary(&ui, catalog);
    });

    let weak = ui.as_weak();
    let downloading = state.clone();
    ui.on_start_selected_download(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = downloading.borrow_mut();
        if !ui.get_package_import_allowed()
            || ui.get_package_import_busy()
            || ui.get_package_import_ready()
            || !ui.get_download_has_selection()
            || state.receiver.is_some()
        {
            return;
        }
        let Some(catalog) = state.catalog.clone() else {
            return;
        };
        let Ok(ids) = selected(&ui) else {
            failure(&ui, ImportFailure::Catalog);
            return;
        };
        let total_bytes = catalog
            .packages()
            .filter(|pin| ids.contains(&pin.id()))
            .map(|pin| pin.bytes())
            .sum();
        let progress = Arc::new(Progress {
            current: Mutex::new((0, 0)),
            total: ids.len(),
            total_bytes,
        });
        state.progress = Some(progress.clone());
        state.last_progress = None;
        clear_catalog(&ui, &mut state);
        begin(&ui, &mut state, move |cancel| {
            let trust = PackageTrust::release()
                .map_err(autokeyboardlayot::package_catalog::ReleaseError::from)?;
            let store = managed_store()?;
            let selection = catalog.select(&ids, &trust, now()?)?;
            let deadline = Instant::now() + Duration::from_secs(600);
            let clock = || {
                if Instant::now() >= deadline {
                    Err(DownloadError::Deadline)
                } else {
                    now()
                }
            };
            let prepared = selection.accept_and_download_with_progress(
                &store,
                &trust,
                cancel,
                clock,
                |pin| {
                    clock()?;
                    autokeyboardlayot::package_download::download(pin, &trust, cancel)
                },
                |count, bytes| {
                    if let Ok(mut value) = progress.current.lock() {
                        *value = (count, bytes);
                    }
                },
            )?;
            Ok(Completed::Online(Box::new(prepared)))
        });
    });

    let weak = ui.as_weak();
    let cancelling = state.clone();
    ui.on_cancel_download(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let state = cancelling.borrow();
        if let Some(cancel) = &state.cancel {
            cancel.store(true, Ordering::Relaxed);
            ui.set_download_cancelling(true);
            ui.set_status_text(tr("download.cancelling").into());
        }
    });
    let weak = ui.as_weak();
    let discarding = state.clone();
    ui.on_discard_download_catalog(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_package_import_busy() || ui.get_package_import_ready() {
            return;
        }
        clear_catalog(&ui, &mut discarding.borrow_mut());
    });
    let weak = ui.as_weak();
    ui.on_refresh_download_labels(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // A completed store change also invokes language refresh while the
        // manager timer owns the state borrow; its online catalog is cleared.
        let Ok(state) = state.try_borrow() else {
            return;
        };
        if let Some(catalog) = &state.catalog {
            rows(&ui, catalog, &selected(&ui).unwrap_or_default());
        }
        if let Some(Pending::Online(prepared)) = &state.pending {
            review_text(&ui, prepared);
        }
    });
}
