//! Local import is separate from Apply and never implicitly migrates a profile.
use super::*;
use autokeyboardlayot::{
    configuration::PackageMode,
    installed_packages::InstalledPackages,
    language_package::PackageTrust,
    language_package::VerifiedLanguagePackage,
    package_install::{PreparedCatalog, PreparedOnlineInstall},
    package_inventory::PackageInventory,
    package_store::{
        PackageStore, PreparedImport, PreparedRollback, PreparedStoreInitialization, StoreSnapshot,
    },
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::mpsc, time::Duration};
mod online;

/// Coarse, path-free reason codes for a failed local package operation, so the
/// status line can name the failing stage instead of a generic message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ImportFailure {
    /// The managed store or its directory cannot be opened.
    Store,
    /// The current inventory snapshot cannot be loaded or trusted.
    Snapshot,
    /// Preparing or verifying the operation failed.
    Prepare,
    /// The chosen file is not a valid package.
    File,
    /// The settings changed outside this window; migration was aborted.
    Migration,
    /// The catalog state is gone or the selection is invalid.
    Catalog,
    /// A write may have happened; never advertise a safe automatic retry.
    OutcomeUnknown,
    /// The worker could not be started (no work was submitted).
    Thread,
}

impl ImportFailure {
    fn key(self) -> &'static str {
        match self {
            Self::Store => "import.failure.store",
            Self::Snapshot => "import.failure.snapshot",
            Self::Prepare => "import.failure.prepare",
            Self::File => "import.failure.file",
            Self::Migration => "import.failure.migration",
            Self::Catalog => "import.failure.catalog",
            Self::OutcomeUnknown => "package.uncertain",
            Self::Thread => "import.failure.thread",
        }
    }
}

enum Completed {
    Prepared(Box<PreparedImport>),
    Rollback(Box<PreparedRollback>),
    RollbackUnavailable(autokeyboardlayot::PackId),
    Removal(Box<Removal>),
    Migration(Box<Migration>),
    Listing(Vec<(String, String, bool)>),
    Catalog(Box<PreparedCatalog>),
    Online(Box<PreparedOnlineInstall>),
    OnlineFailure(String),
    UncertainListing(Option<Vec<(String, String, bool)>>),
    Changed {
        packages: InstalledPackages,
        listing: Vec<(String, String, bool)>,
        removed: bool,
        rolled_back: bool,
        migrated: Option<Box<ConfigurationDocument>>,
        online_installed: bool,
        uncertain: bool,
    },
}
struct Removal {
    expected: StoreSnapshot,
    candidate: PackageInventory,
    package: Arc<VerifiedLanguagePackage>,
}
struct Migration {
    original: ConfigurationDocument,
    prepared: PreparedStoreInitialization,
    required: BTreeSet<autokeyboardlayot::PackId>,
}
enum Pending {
    Import(PreparedImport),
    Rollback(PreparedRollback),
    Remove(Removal),
    Migrate(Box<Migration>),
    Online(Box<PreparedOnlineInstall>),
}
#[derive(Default)]
struct State {
    pending: Option<Pending>,
    receiver: Option<mpsc::Receiver<Result<Completed, ImportFailure>>>,
    catalog: Option<Arc<PreparedCatalog>>,
    cancel: Option<Arc<AtomicBool>>,
    progress: Option<Arc<online::Progress>>,
    last_progress: Option<(usize, u64)>,
    inspecting_uncertain: bool,
}

fn listing(snapshot: &StoreSnapshot) -> Vec<(String, String, bool)> {
    snapshot
        .inventory()
        .receipts()
        .map(|receipt| {
            (
                receipt.id().as_str().to_owned(),
                format!("{} / {}", receipt.id().as_str(), receipt.revision()),
                snapshot
                    .inventory()
                    .rollback_target(&receipt.id())
                    .is_some(),
            )
        })
        .collect()
}

fn show_listing(ui: &SettingsWindow, rows: &[(String, String, bool)]) {
    ui.set_installed_package_ids(strings_model(rows.iter().map(|row| row.0.as_str())));
    ui.set_installed_package_names(strings_model(rows.iter().map(|row| row.1.as_str())));
    ui.set_installed_package_rollback(
        Rc::new(slint::VecModel::from(
            rows.iter().map(|row| row.2).collect::<Vec<_>>(),
        ))
        .into(),
    );
    ui.set_installed_package_index(0);
}

fn rollback_preview(prepared: &PreparedRollback) -> String {
    let mut preview = tr_format(
        "rollback.preview",
        &[
            ("from", &prepared.from_revision().to_string()),
            ("to", &prepared.package().revision().to_string()),
        ],
    );
    preview.push_str("\n\n");
    preview.push_str(&package_preview(prepared.package()));
    preview
}

fn package_preview(p: &VerifiedLanguagePackage) -> String {
    let hash: String = p
        .envelope_sha256()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut preview = tr_format(
        "import.preview",
        &[
            ("id", p.id().as_str()),
            ("revision", &p.revision().to_string()),
            ("bytes", &p.envelope_bytes().to_string()),
            ("hash", &hash),
            ("input", p.input().map_or("—", |p| p.id().as_str())),
            ("locale", p.ui_locale().unwrap_or("—")),
        ],
    );
    // Render one independently bounded legal component pair at a time, even
    // when a migration contains many packages. Do not use the format limit.
    preview.push_str(p.license());
    preview.push_str("\n\n");
    preview.push_str(&tr("import.notice"));
    preview.push('\n');
    preview.push_str(p.notice());
    preview
}

fn unchanged_draft(ui: &SettingsWindow, original: &ConfigurationDocument) -> bool {
    document_from_ui(ui, original.clone()).is_ok_and(|draft| &draft == original)
}

fn start(
    ui: &SettingsWindow,
    state: &mut State,
    work: impl FnOnce() -> Result<Completed, ImportFailure> + Send + 'static,
) {
    let (send, receive) = mpsc::channel();
    match std::thread::Builder::new()
        .name("package-import".into())
        .spawn(move || {
            let _ = send.send(work());
        }) {
        Ok(_) => {
            state.receiver = Some(receive);
            ui.set_package_import_busy(true);
        }
        Err(_) => failure(ui, ImportFailure::Thread),
    }
}

fn failure(ui: &SettingsWindow, reason: ImportFailure) {
    ui.set_status_error(true);
    ui.set_status_text(tr(reason.key()).into());
}

fn store() -> Result<PackageStore, ImportFailure> {
    // Recheck the persisted mode, not just the mode when settings opened.
    let document =
        super::super::try_load_configuration_document().map_err(|_| ImportFailure::Store)?;
    if document.package_mode != PackageMode::Managed {
        return Err(ImportFailure::Store);
    }
    let directory = super::super::configuration_directory().ok_or(ImportFailure::Store)?;
    PackageStore::open(&directory.join("packages")).map_err(|_| ImportFailure::Store)
}

// Read once after a failed write, including migration where managed mode may
// not yet be active. Never confirm again and never activate configuration here.
fn inspect_uncertain_write(trust: &PackageTrust) -> Completed {
    let result = inspect_uncertain_snapshot(|| {
        super::super::configuration_directory()
            .and_then(|directory| PackageStore::open(&directory.join("packages")).ok())
            .and_then(|store| store.load(trust).ok())
    });
    notify_agent();
    result
}

fn inspect_uncertain_snapshot(read: impl FnOnce() -> Option<StoreSnapshot>) -> Completed {
    Completed::UncertainListing(read().map(|snapshot| listing(&snapshot)))
}

pub(super) fn wire(
    ui: &SettingsWindow,
    document: Rc<RefCell<ConfigurationDocument>>,
    registry: Rc<RefCell<Arc<autokeyboardlayot::DictionaryRegistry>>>,
) -> slint::Timer {
    ui.set_package_import_allowed(document.borrow().package_mode == PackageMode::Managed);
    let state = Rc::new(RefCell::new(State::default()));
    online::wire(ui, state.clone());
    let weak = ui.as_weak();
    let labels_state = state.clone();
    ui.on_refresh_package_labels(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // The result timer may be refreshing localization with State borrowed.
        let Ok(state) = labels_state.try_borrow() else {
            return;
        };
        if let Some(Pending::Rollback(prepared)) = &state.pending {
            ui.set_package_import_preview(rollback_preview(prepared).into());
        }
    });
    let weak = ui.as_weak();
    let rollback_state = state.clone();
    ui.on_prepare_package_rollback(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = rollback_state.borrow_mut();
        if !ui.get_package_import_allowed()
            || ui.get_package_import_busy()
            || state.receiver.is_some()
            || state.pending.is_some()
        {
            return;
        }
        let Some(id) = usize::try_from(ui.get_installed_package_index())
            .ok()
            .filter(|index| ui.get_installed_package_rollback().row_data(*index) == Some(true))
            .and_then(|index| ui.get_installed_package_ids().row_data(index))
            .and_then(|id| autokeyboardlayot::PackId::parse(id.as_str()).ok())
        else {
            return;
        };
        ui.set_package_import_ready(false);
        ui.set_package_import_preview(SharedString::default());
        start(&ui, &mut state, move || {
            match store().and_then(|store| {
                PreparedRollback::from_store(
                    &store,
                    id,
                    &PackageTrust::release().map_err(|_| ImportFailure::Prepare)?,
                )
                .map_err(|_| ImportFailure::Prepare)
            }) {
                Ok(prepared) => Ok(Completed::Rollback(Box::new(prepared))),
                Err(_) => Ok(Completed::RollbackUnavailable(id)),
            }
        });
    });
    let weak = ui.as_weak();
    let migration_state = state.clone();
    let migration_document = document.clone();
    let migration_registry = registry.clone();
    ui.on_prepare_migration(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_package_import_allowed()
            || ui.get_package_import_busy()
            || migration_state.borrow().receiver.is_some()
        {
            return;
        }
        let original = migration_document.borrow().clone();
        if original.package_mode != PackageMode::LegacyBootstrap || !unchanged_draft(&ui, &original)
        {
            ui.set_status_error(true);
            ui.set_status_text(tr("migration.apply_first").into());
            return;
        }
        let required = migration_registry
            .borrow()
            .installed_ids()
            .copied()
            .collect();
        hotkey_capture::stop();
        ui.set_package_import_busy(true);
        let paths = choose_files(true);
        ui.set_package_import_busy(false);
        let Some(paths) = paths else {
            return;
        };
        let mut state = migration_state.borrow_mut();
        state.pending = None;
        ui.set_package_import_ready(false);
        ui.set_package_import_preview(SharedString::default());
        start(&ui, &mut state, move || {
            let current = super::super::try_load_configuration_document()
                .map_err(|_| ImportFailure::Migration)?;
            if current != original {
                return Err(ImportFailure::Migration);
            }
            let prepared = PreparedStoreInitialization::from_files(
                &paths,
                &required,
                &PackageTrust::release().map_err(|_| ImportFailure::Prepare)?,
            )
            .map_err(|_| ImportFailure::Prepare)?;
            Ok(Completed::Migration(Box::new(Migration {
                original,
                prepared,
                required,
            })))
        });
    });

    let weak = ui.as_weak();
    let preview_state = state.clone();
    ui.on_preview_review_package(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_package_import_busy() {
            return;
        }
        let state = preview_state.borrow();
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let package = match &state.pending {
            Some(Pending::Migrate(migration)) => {
                migration.prepared.packages().get(index).map(Arc::as_ref)
            }
            Some(Pending::Online(prepared)) => {
                prepared.packages().get(index).map(|p| p.package().as_ref())
            }
            _ => None,
        };
        let Some(p) = package else {
            return;
        };
        ui.set_package_import_preview(package_preview(p).into());
    });
    let weak = ui.as_weak();
    let list_state = state.clone();
    ui.on_refresh_packages(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = list_state.borrow_mut();
        if !ui.get_package_import_allowed()
            || ui.get_package_import_busy()
            || state.receiver.is_some()
        {
            return;
        }
        state.pending = None;
        ui.set_package_import_ready(false);
        ui.set_package_import_preview(SharedString::default());
        start(&ui, &mut state, || {
            let snapshot = store()?
                .load(&PackageTrust::release().map_err(|_| ImportFailure::Snapshot)?)
                .map_err(|_| ImportFailure::Snapshot)?;
            Ok(Completed::Listing(listing(&snapshot)))
        });
    });

    let weak = ui.as_weak();
    let remove_state = state.clone();
    ui.on_prepare_package_removal(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = remove_state.borrow_mut();
        if !ui.get_package_import_allowed()
            || ui.get_package_import_busy()
            || state.receiver.is_some()
        {
            return;
        }
        let Some(id) = usize::try_from(ui.get_installed_package_index())
            .ok()
            .and_then(|index| ui.get_installed_package_ids().row_data(index))
            .and_then(|id| autokeyboardlayot::PackId::parse(id.as_str()).ok())
        else {
            return;
        };
        state.pending = None;
        ui.set_package_import_ready(false);
        ui.set_package_import_preview(SharedString::default());
        start(&ui, &mut state, move || {
            let expected = store()?
                .load(&PackageTrust::release().map_err(|_| ImportFailure::Snapshot)?)
                .map_err(|_| ImportFailure::Snapshot)?;
            let package = expected
                .inventory()
                .packages()
                .find(|p| p.id() == id)
                .cloned()
                .ok_or(ImportFailure::Snapshot)?;
            let candidate = expected
                .inventory()
                .stage_remove(&BTreeSet::from([id]))
                .map_err(|_| ImportFailure::Prepare)?;
            Ok(Completed::Removal(Box::new(Removal {
                expected,
                candidate,
                package,
            })))
        });
    });
    let weak = ui.as_weak();
    let inspect_state = state.clone();
    ui.on_inspect_package(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_package_import_allowed()
            || ui.get_package_import_busy()
            || inspect_state.borrow().receiver.is_some()
        {
            return;
        }
        hotkey_capture::stop();
        // The native dialog pumps messages. Guard all import callbacks while
        // it is open, even though no worker receiver exists yet.
        ui.set_package_import_busy(true);
        let path = choose_file();
        ui.set_package_import_busy(false);
        let Some(path) = path else {
            return;
        };
        let mut state = inspect_state.borrow_mut();
        state.pending = None;
        ui.set_package_import_ready(false);
        ui.set_package_import_preview(SharedString::default());
        start(&ui, &mut state, move || {
            let store = store()?;
            let trust = PackageTrust::release().map_err(|_| ImportFailure::Prepare)?;
            let snapshot = store.load(&trust).map_err(|_| ImportFailure::Snapshot)?;
            PreparedImport::from_file(&path, &snapshot, &trust)
                .map(|prepared| Completed::Prepared(Box::new(prepared)))
                .map_err(|_| ImportFailure::File)
        });
    });

    let weak = ui.as_weak();
    let cancel_state = state.clone();
    ui.on_cancel_package(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = cancel_state.borrow_mut();
        if ui.get_package_import_busy() || state.receiver.is_some() {
            return;
        }
        state.pending = None;
        ui.set_package_import_ready(false);
        ui.set_package_import_preview(SharedString::default());
    });

    let weak = ui.as_weak();
    let confirm_state = state.clone();
    ui.on_confirm_package(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut state = confirm_state.borrow_mut();
        if ui.get_package_import_busy() || state.receiver.is_some() {
            return;
        }
        let Ok(selected) = selected_input_packs(&ui) else {
            failure(&ui, ImportFailure::Prepare);
            return;
        };
        if selected_input_profiles(&ui).is_err() {
            failure(&ui, ImportFailure::Prepare);
            return;
        }
        if let Some(Pending::Migrate(migration)) = &state.pending
            && !unchanged_draft(&ui, &migration.original)
        {
            ui.set_status_error(true);
            ui.set_status_text(tr("migration.apply_first").into());
            return;
        }
        let Some(prepared) = state.pending.take() else {
            return;
        };
        ui.set_package_import_ready(false);
        ui.set_package_import_preview(SharedString::default());
        start(&ui, &mut state, move || {
            let trust = PackageTrust::release().map_err(|_| ImportFailure::Prepare)?;
            let removed = matches!(&prepared, Pending::Remove(_));
            let rolled_back = matches!(&prepared, Pending::Rollback(_));
            let online_installed = matches!(&prepared, Pending::Online(_));
            let mut migration_documents = None;
            let snapshot = match prepared {
                Pending::Import(prepared) => prepared.confirm(&store()?, &trust),
                Pending::Rollback(prepared) => prepared.confirm(&store()?, &trust),
                Pending::Remove(prepared) => {
                    store()?.commit(&prepared.expected, &prepared.candidate, &[], &trust)
                }
                Pending::Online(prepared) => {
                    match prepared.confirm(
                        &store()?,
                        &trust,
                        online::now().map_err(|_| ImportFailure::Prepare)?,
                    ) {
                        Ok(snapshot) => Ok(snapshot),
                        Err(autokeyboardlayot::package_install::InstallError::Store(error)) => {
                            Err(error)
                        }
                        Err(error) => return Ok(Completed::OnlineFailure(error.to_string())),
                    }
                }
                Pending::Migrate(migration) => {
                    let current = super::super::try_load_configuration_document()
                        .map_err(|_| ImportFailure::Migration)?;
                    if current != migration.original {
                        return Err(ImportFailure::Migration);
                    }
                    let directory =
                        super::super::configuration_directory().ok_or(ImportFailure::Migration)?;
                    // A first explicit save/migration may have no app directory
                    // yet. Create only that child of the existing per-user root;
                    // the store still checks its parent and never repairs it.
                    if let Err(error) = std::fs::create_dir(&directory)
                        && error.kind() != std::io::ErrorKind::AlreadyExists
                    {
                        return Err(ImportFailure::Migration);
                    }
                    let snapshot = migration
                        .prepared
                        .confirm(&directory.join("packages"), &trust);
                    let mut next = migration.original.clone();
                    next.package_mode = PackageMode::Managed;
                    migration_documents = Some((migration.original, next));
                    snapshot
                }
            };
            let (snapshot, uncertain) = match snapshot {
                Ok(snapshot) => (snapshot, false),
                Err(autokeyboardlayot::package_store::StoreError::CommitUncertain)
                    if migration_documents.is_none() =>
                {
                    // Refresh, never repeat a possibly committed operation.
                    // A migration must not activate config after this error.
                    notify_agent();
                    (
                        store()
                            .and_then(|store| {
                                store
                                    .load(&trust)
                                    .map_err(|_| ImportFailure::OutcomeUnknown)
                            })
                            .map_err(|_| ImportFailure::OutcomeUnknown)?,
                        true,
                    )
                }
                Err(_) => return Ok(inspect_uncertain_write(&trust)),
            };
            // Even if settings closes after confirmation, notify the agent of
            // the committed store; a missing receiver must not skip this step.
            if migration_documents.is_none() && !uncertain {
                notify_agent();
            }
            let listing = listing(&snapshot);
            let packages = InstalledPackages::from_store(&snapshot, &selected)
                .map_err(|_| ImportFailure::OutcomeUnknown)?;
            let migrated = if let Some((original, next)) = migration_documents {
                // Saving mode last is the activation point. Failure leaves a
                // prepared store ignored by the old legacy configuration.
                let activation = super::super::configuration_directory()
                    .and_then(|directory| PackageStore::open(&directory.join("packages")).ok())
                    .is_some_and(|store| {
                        store
                            .with_current_snapshot(&snapshot, &trust, || {
                                save_configuration_if_unchanged(&next, &original)
                            })
                            .is_ok()
                    });
                if !activation {
                    return Ok(inspect_uncertain_write(&trust));
                }
                notify_agent();
                Some(Box::new(next))
            } else {
                None
            };
            Ok(Completed::Changed {
                packages,
                listing,
                removed,
                rolled_back,
                migrated,
                online_installed,
                uncertain,
            })
        });
    });

    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = state.borrow_mut();
            online::poll_progress(&ui, &mut state);
            let Some(receiver) = &state.receiver else {
                return;
            };
            let outcome = match receiver.try_recv() {
                Ok(value) => value,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Err(ImportFailure::OutcomeUnknown),
            };
            state.receiver = None;
            let was_uncertain_inspection = state.inspecting_uncertain;
            state.inspecting_uncertain = false;
            ui.set_package_import_busy(false);
            let was_network = ui.get_download_active();
            let cancelled = state
                .cancel
                .take()
                .is_some_and(|flag| flag.load(Ordering::Relaxed));
            state.progress = None;
            state.last_progress = None;
            ui.set_download_active(false);
            ui.set_download_cancelling(false);
            if was_network && cancelled {
                online::clear_catalog(&ui, &mut state);
                state.pending = None;
                ui.set_package_import_ready(false);
                ui.set_package_import_preview(SharedString::default());
                ui.set_status_error(false);
                ui.set_status_text(tr("download.cancelled").into());
                return;
            }
            match outcome {
                Ok(Completed::UncertainListing(rows)) => {
                    // A failed reread is not evidence of an empty inventory.
                    if let Some(rows) = rows {
                        show_listing(&ui, &rows);
                    }
                    state.pending = None;
                    online::clear_catalog(&ui, &mut state);
                    ui.set_package_import_ready(false);
                    failure(&ui, ImportFailure::OutcomeUnknown);
                }
                Ok(Completed::Prepared(prepared)) => {
                    ui.set_package_import_preview(package_preview(prepared.package()).into());
                    state.pending = Some(Pending::Import(*prepared));
                    ui.set_package_online(false);
                    ui.set_package_migration(false);
                    ui.set_package_removal(false);
                    ui.set_package_rollback(false);
                    ui.set_package_import_ready(true);
                    ui.set_status_error(false);
                    ui.set_status_text(SharedString::default());
                }
                Ok(Completed::Rollback(prepared)) => {
                    ui.set_package_import_preview(rollback_preview(&prepared).into());
                    state.pending = Some(Pending::Rollback(*prepared));
                    ui.set_package_online(false);
                    ui.set_package_migration(false);
                    ui.set_package_removal(false);
                    ui.set_package_rollback(true);
                    ui.set_package_import_ready(true);
                    ui.set_status_error(false);
                    ui.set_status_text(SharedString::default());
                }
                Ok(Completed::RollbackUnavailable(id)) => {
                    // Prevent repeated clicks after failed authentication/cache
                    // lookup; a deliberate refresh allows a fresh inspection.
                    let ids = ui.get_installed_package_ids();
                    let flags = ui.get_installed_package_rollback();
                    let flags: Vec<bool> = (0..ids.row_count())
                        .map(|index| {
                            ids.row_data(index)
                                .is_some_and(|row| row.as_str() != id.as_str())
                                && flags.row_data(index).unwrap_or(false)
                        })
                        .collect();
                    ui.set_installed_package_rollback(Rc::new(slint::VecModel::from(flags)).into());
                    ui.set_status_error(true);
                    ui.set_status_text(tr("rollback.unavailable").into());
                }
                Ok(Completed::Removal(prepared)) => {
                    let p = &prepared.package;
                    ui.set_package_import_preview(
                        tr_format(
                            "remove.preview",
                            &[
                                ("id", p.id().as_str()),
                                ("revision", &p.revision().to_string()),
                                ("input", p.input().map_or("—", |p| p.id().as_str())),
                                ("locale", p.ui_locale().unwrap_or("—")),
                            ],
                        )
                        .into(),
                    );
                    state.pending = Some(Pending::Remove(*prepared));
                    ui.set_package_online(false);
                    ui.set_package_migration(false);
                    ui.set_package_removal(true);
                    ui.set_package_rollback(false);
                    ui.set_package_import_ready(true);
                    ui.set_status_error(false);
                    ui.set_status_text(SharedString::default());
                }
                Ok(Completed::Migration(migration)) => {
                    let names: Vec<String> = migration
                        .prepared
                        .packages()
                        .iter()
                        .map(|p| format!("{} / {}", p.id().as_str(), p.revision()))
                        .collect();
                    ui.set_review_package_names(strings_model(names.iter().map(String::as_str)));
                    ui.set_review_package_index(0);
                    let required = migration
                        .required
                        .iter()
                        .map(|id| id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.set_migration_summary(
                        tr_format(
                            "migration.summary",
                            &[
                                ("count", &names.len().to_string()),
                                ("bytes", &migration.prepared.total_bytes().to_string()),
                                ("required", &required),
                            ],
                        )
                        .into(),
                    );
                    ui.set_package_import_preview(
                        migration
                            .prepared
                            .packages()
                            .first()
                            .map(|p| package_preview(p))
                            .unwrap_or_default()
                            .into(),
                    );
                    state.pending = Some(Pending::Migrate(migration));
                    ui.set_package_online(false);
                    ui.set_package_migration(true);
                    ui.set_package_removal(false);
                    ui.set_package_rollback(false);
                    ui.set_package_import_ready(true);
                    ui.set_status_error(false);
                    ui.set_status_text(SharedString::default());
                }
                Ok(Completed::Listing(rows)) => {
                    show_listing(&ui, &rows);
                    ui.set_status_error(false);
                    ui.set_status_text(SharedString::default());
                }
                Ok(Completed::Catalog(catalog)) => online::show_catalog(&ui, &mut state, catalog),
                Ok(Completed::Online(prepared)) => online::show_prepared(&ui, &mut state, prepared),
                Ok(Completed::OnlineFailure(code)) => {
                    online::clear_catalog(&ui, &mut state);
                    ui.set_status_error(true);
                    ui.set_status_text(tr_format("download.failed", &[("code", &code)]).into());
                }
                Ok(Completed::Changed {
                    packages,
                    listing,
                    removed,
                    rolled_back,
                    migrated,
                    online_installed,
                    uncertain,
                }) => {
                    online::clear_catalog(&ui, &mut state);
                    let did_migrate = migrated.is_some();
                    if let Some(next) = migrated {
                        *document.borrow_mut() = *next;
                        ui.set_package_import_allowed(true);
                    }
                    show_listing(&ui, &listing);
                    let selected = selected_input_packs(&ui);
                    let profiles = selected_input_profiles(&ui);
                    let draft_valid = selected.is_ok() && profiles.is_ok();
                    let preference = usize::try_from(ui.get_ui_language_index())
                        .ok()
                        .and_then(|index| ui.get_ui_language_ids().row_data(index))
                        .map(|value| UiLanguagePreference::from_config(value.as_str()))
                        .unwrap_or_else(|| document.borrow().ui_language.clone());
                    super::super::ui_localization::initialize(
                        &preference,
                        packages.catalogs.as_deref(),
                    );
                    *registry.borrow_mut() = packages.dictionaries;
                    if let (Ok(selected), Ok(profiles)) = (selected, profiles) {
                        populate_input_packs(&ui, &selected, &profiles, &registry.borrow());
                    }
                    refresh_ui_language(&ui, &preference, &registry.borrow());
                    ui.set_status_error(uncertain || !draft_valid);
                    let mut status = if uncertain {
                        tr("package.uncertain")
                    } else if did_migrate {
                        tr("migration.done")
                    } else if online_installed {
                        tr("download.done")
                    } else if removed {
                        tr("remove.done")
                    } else if rolled_back {
                        tr("rollback.done")
                    } else {
                        tr("import.done")
                    };
                    if !draft_valid {
                        status.push('\n');
                        status.push_str(&tr("error.pack_selection"));
                    }
                    ui.set_status_text(status.into());
                }
                Err(ImportFailure::OutcomeUnknown) if !was_uncertain_inspection => {
                    state.pending = None;
                    ui.set_package_import_ready(false);
                    online::clear_catalog(&ui, &mut state);
                    start(&ui, &mut state, || {
                        let trust =
                            PackageTrust::release().map_err(|_| ImportFailure::OutcomeUnknown)?;
                        Ok(inspect_uncertain_write(&trust))
                    });
                    state.inspecting_uncertain = state.receiver.is_some();
                    if !state.inspecting_uncertain {
                        failure(&ui, ImportFailure::OutcomeUnknown);
                    }
                }
                Err(reason) => failure(&ui, reason),
            }
        },
    );
    timer
}

fn choose_file() -> Option<PathBuf> {
    choose_files(false)?.into_iter().next()
}

fn choose_files(multiple: bool) -> Option<Vec<PathBuf>> {
    let filter: Vec<u16> = "*.aklp\0*.aklp\0\0".encode_utf16().collect();
    let title: Vec<u16> = if multiple {
        tr("migration.choose")
    } else {
        tr("import.choose")
    }
    .encode_utf16()
    .chain([0])
    .collect();
    let mut file = vec![0u16; 32_768];
    let mut flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
    if multiple {
        flags |= windows::Win32::UI::Controls::Dialogs::OFN_ALLOWMULTISELECT;
    }
    let mut dialog = OPENFILENAMEW {
        lStructSize: u32::try_from(size_of::<OPENFILENAMEW>()).ok()?,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: u32::try_from(file.len()).ok()?,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: flags,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&raw mut dialog) }.as_bool() {
        return None;
    }
    decode_selected_files(&file, multiple)
}

fn decode_selected_files(file: &[u16], multiple: bool) -> Option<Vec<PathBuf>> {
    if file.len() > 32_768 {
        return None;
    }
    let length = file.iter().position(|value| *value == 0)?;
    use std::os::windows::ffi::OsStringExt;
    let first = PathBuf::from(std::ffi::OsString::from_wide(&file[..length]));
    if !first.is_absolute() {
        return None;
    }
    if !multiple || file.get(length + 1).copied().unwrap_or(0) == 0 {
        return Some(vec![first]);
    }
    let mut paths = Vec::new();
    let mut remaining = &file[length + 1..];
    loop {
        let end = remaining.iter().position(|value| *value == 0)?;
        if end == 0 {
            break;
        }
        let name = PathBuf::from(std::ffi::OsString::from_wide(&remaining[..end]));
        let mut components = name.components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return None;
        }
        paths.push(first.join(name));
        if paths.len() > 64 {
            return None;
        }
        remaining = &remaining[end + 1..];
    }
    Some(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncertain_reread_is_once_and_does_not_turn_failure_into_an_empty_inventory() {
        let mut reads = 0;
        let result = inspect_uncertain_snapshot(|| {
            reads += 1;
            None
        });
        assert_eq!(reads, 1);
        assert!(matches!(result, Completed::UncertainListing(None)));
        let directory = tempfile::tempdir().unwrap();
        let store = PackageStore::initialize(&directory.path().join("packages")).unwrap();
        let trust = PackageTrust::release().unwrap();
        let result = inspect_uncertain_snapshot(|| {
            reads += 1;
            store.load(&trust).ok()
        });
        assert_eq!(reads, 2);
        assert!(matches!(result, Completed::UncertainListing(Some(rows)) if rows.is_empty()));
        assert_eq!(ImportFailure::OutcomeUnknown.key(), "package.uncertain");
    }

    #[test]
    fn native_file_selection_preserves_unicode_spaces_and_single_selection_shape() {
        let buffer: Vec<u16> = "C:\\Языки with spaces\\ru.aklp\0\0"
            .encode_utf16()
            .collect();
        for multiple in [false, true] {
            assert_eq!(
                decode_selected_files(&buffer, multiple).unwrap(),
                vec![PathBuf::from("C:\\Языки with spaces\\ru.aklp")]
            );
        }
        let multiple: Vec<u16> = "C:\\Языки with spaces\0ru.aklp\0et.aklp\0\0"
            .encode_utf16()
            .collect();
        assert_eq!(
            decode_selected_files(&multiple, true).unwrap(),
            vec![
                PathBuf::from("C:\\Языки with spaces\\ru.aklp"),
                PathBuf::from("C:\\Языки with spaces\\et.aklp"),
            ]
        );
    }

    #[test]
    fn native_multiple_selection_refuses_non_child_paths_and_missing_termination() {
        for child in [
            "..\\outside.aklp",
            "D:\\outside.aklp",
            "\\rooted.aklp",
            ".",
            "nested\\file.aklp",
        ] {
            let buffer: Vec<u16> = format!("C:\\packages\0{child}\0\0")
                .encode_utf16()
                .collect();
            assert!(decode_selected_files(&buffer, true).is_none());
        }
        assert!(
            decode_selected_files(&"C:\\packages".encode_utf16().collect::<Vec<_>>(), false)
                .is_none()
        );
        assert!(
            decode_selected_files(&"relative.aklp\0".encode_utf16().collect::<Vec<_>>(), false)
                .is_none()
        );
        for truncated in ["C:\\packages\0ru.aklp", "C:\\packages\0ru.aklp\0"] {
            assert!(
                decode_selected_files(&truncated.encode_utf16().collect::<Vec<_>>(), true)
                    .is_none()
            );
        }
    }

    #[test]
    fn native_multiple_selection_bounds_buffer_and_number_of_paths() {
        assert!(decode_selected_files(&vec![0; 32_769], true).is_none());
        let mut names = String::from("C:\\packages\0");
        for index in 0..65 {
            names.push_str(&format!("{index}.aklp\0"));
        }
        names.push('\0');
        assert!(decode_selected_files(&names.encode_utf16().collect::<Vec<_>>(), true).is_none());
    }
}
