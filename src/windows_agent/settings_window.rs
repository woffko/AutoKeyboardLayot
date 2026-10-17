//! Fluent settings UI. It runs as a separate mode of the same executable so
//! the keyboard-hook process never shares or blocks the UI event loop.

use std::{collections::BTreeSet, fs::OpenOptions, mem::size_of, path::Path, sync::Arc};

use autokeyboardlayot::configuration::{EditorEntry, load_configuration_directory};
use autokeyboardlayot::localization::UiLanguagePreference;
use autokeyboardlayot::{BackendRules, ConfigurationDocument, Hotkey, PackId};
use i_slint_backend_winit::winit::platform::windows::WindowAttributesExtWindows;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, WPARAM,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::CreateMutexW,
        },
        UI::{
            Controls::Dialogs::{
                GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
            },
            Shell::ShellExecuteW,
            WindowsAndMessaging::{
                FindWindowW, GA_ROOTOWNER, GetAncestor, PostMessageW, SW_RESTORE, SW_SHOWNORMAL,
                SetForegroundWindow, ShowWindow,
            },
        },
    },
    core::{HSTRING, PCWSTR, PWSTR, w},
};

use super::{
    SETTINGS_APPLIED_MESSAGE, WINDOW_CLASS, configuration_directory_or_error, diagnostic_log_path,
    save_configuration_if_unchanged, try_load_configuration_document,
};

slint::include_modules!();
use super::ui_localization::{tr, tr_format};
mod hotkey_capture;
mod package_import;

pub(super) const SETTINGS_MUTEX: PCWSTR = w!("Local\\AutoKeyboardLayot.Settings.Singleton");
pub(super) const SETTINGS_WINDOW_CLASS: &str = "AutoKeyboardLayot.Settings.Window";

struct SettingsInstanceGuard {
    handle: HANDLE,
    _installation_fence: std::fs::File,
}

impl SettingsInstanceGuard {
    fn acquire() -> Result<Option<Self>, String> {
        let Some(fence) = autokeyboardlayot::installation_fence::shared_for_current_user()
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let handle = unsafe { CreateMutexW(None, false, SETTINGS_MUTEX) }
            .map_err(|error| error.to_string())?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            focus_existing_window();
            Ok(None)
        } else {
            Ok(Some(Self {
                handle,
                _installation_fence: fence,
            }))
        }
    }
}

impl Drop for SettingsInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

pub(super) fn show(owner: HWND) {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let executable = HSTRING::from(executable.as_os_str());
    unsafe {
        let _ = ShellExecuteW(
            Some(owner),
            w!("open"),
            &executable,
            w!("--settings"),
            None,
            SW_SHOWNORMAL,
        );
    }
}

pub(super) fn run() -> Result<(), String> {
    let Some(_guard) = SettingsInstanceGuard::acquire()? else {
        return Ok(());
    };
    let document = try_load_configuration_document()
        .map_err(|error| tr_format("error.read_config", &[("error", &error.to_string())]))?;
    let packages = super::load_installed_packages(&document)
        .map_err(|error| tr_format("error.read_config", &[("error", &error.to_string())]))?;
    super::ui_localization::initialize(&document.ui_language, packages.catalogs.as_deref());
    // A stable native class keeps singleton lookup independent of translations.
    // This backend API is pinned to the same version/features as Slint itself.
    let backend = i_slint_backend_winit::Backend::builder()
        .with_window_attributes_hook(|attributes| attributes.with_class_name(SETTINGS_WINDOW_CLASS))
        .build()
        .map_err(|error| error.to_string())?;
    slint::platform::set_platform(Box::new(backend)).map_err(|error| error.to_string())?;
    let ui = SettingsWindow::new().map_err(|error| error.to_string())?;
    ui.global::<Localization>()
        .on_text(|id, _revision| tr(id.as_str()).into());
    ui.global::<Localization>().set_revision(1);
    ui.set_app_version(super::APP_VERSION.into());
    ui.global::<Localization>()
        .set_rtl(super::ui_localization::is_rtl());
    populate_ui(&ui, &document, &packages.dictionaries);
    let _import_timer = wire_callbacks(&ui, document, packages.dictionaries);
    ui.show().map_err(|error| error.to_string())?;
    let class = HSTRING::from(SETTINGS_WINDOW_CLASS);
    let native_window = unsafe { FindWindowW(&class, None) }.ok().filter(|window| {
        let mut process_id = 0;
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                *window,
                Some(&mut process_id),
            );
        }
        process_id == unsafe { windows::Win32::System::Threading::GetCurrentProcessId() }
    });
    if let Some(window) = native_window {
        super::installer_lifecycle::advertise_safe_close(window);
    }
    let result = ui.run().map_err(|error| error.to_string());
    if let Some(window) = native_window {
        super::installer_lifecycle::remove_safe_close(window);
    }
    hotkey_capture::stop();
    result
}

fn set_ui_hotkey(ui: &SettingsWindow, hotkey: Hotkey) {
    ui.set_hotkey_key_code(i32::from(hotkey.virtual_key));
    ui.set_hotkey_modifier_bits(i32::from(hotkey.modifiers));
    ui.set_hotkey_label(hotkey.display_name().into());
}

fn populate_ui(
    ui: &SettingsWindow,
    document: &ConfigurationDocument,
    registry: &autokeyboardlayot::DictionaryRegistry,
) {
    populate_ui_language(ui, &document.ui_language);
    let settings = &document.settings;
    ui.set_automatic_conversion_on_startup(settings.automatic_conversion_on_startup);
    ui.set_hotkey_enabled(settings.pause_break_undo);
    ui.set_offer_word_exclusion_after_undo(settings.offer_word_exclusion_after_undo);
    ui.set_offer_dictionary_after_force(settings.offer_dictionary_after_forced_conversion);
    ui.set_recheck_first_word_after_erasing(settings.recheck_first_word_after_erasing);
    ui.set_single_letter_words(settings.single_letter_words);
    ui.set_layout_model(settings.layout_model);
    ui.set_manual_terminal_uia_fallback(settings.manual_terminal_uia_fallback);
    ui.set_conversion_sound(settings.conversion_sound);
    ui.set_physical_fallback(settings.physical_fallback_for_unsupported_apps);
    ui.set_diagnostics_enabled(settings.diagnostics_enabled);

    ui.set_suppress_after_backspace(settings.suppress_after_backspace);
    ui.set_suppress_after_delete(settings.suppress_after_delete);
    ui.set_suppress_after_left(settings.suppress_after_left);
    ui.set_suppress_after_right(settings.suppress_after_right);
    ui.set_suppress_after_up(settings.suppress_after_up);
    ui.set_suppress_after_down(settings.suppress_after_down);
    ui.set_suppress_after_home_end(settings.suppress_after_home_end);
    ui.set_suppress_after_manual_layout(settings.suppress_after_manual_layout_change);

    populate_input_packs(
        ui,
        &settings.enabled_input_packs,
        &document.input_profiles,
        registry,
    );

    ui.set_user_dictionary_text(lines_to_editor(&document.user_dictionary));
    ui.set_word_exclusions_text(lines_to_editor(&document.word_exclusions));
    ui.set_process_exclusions_text(lines_to_editor(&document.process_exclusions));
    ui.set_backend_rules_text(document.backend_rules.to_text().into());

    let hotkey = settings.force_hotkey();
    set_ui_hotkey(ui, hotkey);
    ui.set_running_processes(strings_model(std::iter::empty::<&str>()));
}

fn populate_ui_language(ui: &SettingsWindow, preference: &UiLanguagePreference) {
    let (choices, selected) = super::ui_localization::picker_choices(preference);
    ui.set_ui_language_ids(strings_model(
        choices.iter().map(|choice| choice.id.as_str()),
    ));
    ui.set_ui_language_names(strings_model(
        choices.iter().map(|choice| choice.name.as_str()),
    ));
    ui.set_ui_language_index(selected as i32);
    ui.set_ui_language_status(
        tr_format(
            "ui_language.active",
            &[("language", &super::ui_localization::active_language_name())],
        )
        .into(),
    );
}

fn pack_display_name(id: &str) -> String {
    match id.to_ascii_lowercase().as_str() {
        "en-us" => "English",
        "ru-ru" => "Русский",
        "et-ee" => "Eesti",
        "de-de" => "Deutsch",
        "es-es" => "Español",
        "fr-fr" => "Français",
        "pt-br" => "Português",
        "ja-jp" => "日本語",
        "ar-sa" => "العربية",
        "zh-cn" => "中文（简体）",
        "hi-in" => "हिन्दी",
        "bn-bd" => "বাংলা",
        "id-id" => "Bahasa Indonesia",
        "ur-pk" => "اردو",
        _ => return id.to_owned(),
    }
    .to_owned()
}

fn input_pack_rows(
    selected: &BTreeSet<autokeyboardlayot::PackId>,
    profiles: &autokeyboardlayot::input_profile_selection::InputProfileSelections,
    registry: &autokeyboardlayot::DictionaryRegistry,
) -> Result<Vec<InputPackRow>, String> {
    use autokeyboardlayot::input_pack_selection::{SelectionStatus, selection_rows_with_profiles};
    // All callbacks use the immutable installed snapshot loaded on opening.
    // These rows do not assert live OS readiness or read package files.
    let rows = selection_rows_with_profiles(registry, selected, profiles)
        .map_err(|_| tr("error.pack_selection"))?;
    let rows: Vec<InputPackRow> = rows
        .into_iter()
        .map(|row| {
            let mut ids = vec![String::new()];
            ids.extend(row.profiles.iter().map(ToString::to_string));
            let chosen = row
                .chosen_profile
                .map(|p| p.to_string())
                .unwrap_or_default();
            let profile_index = ids
                .iter()
                .position(|id| id == &chosen)
                .expect("projection retains chosen profile") as i32;
            let mut names = ids.clone();
            names[0] = tr("packs.profile_default");
            InputPackRow {
                profile_ids: strings_model(ids.iter().map(String::as_str)),
                profile_names: strings_model(names.iter().map(String::as_str)),
                profile_index,
                id: row.id.as_str().into(),
                name: pack_display_name(row.id.as_str()).into(),
                // Projection must never activate an installed but disabled pack.
                selected: row.selected,
                status_key: match row.status {
                    SelectionStatus::MissingData => "packs.missing",
                    SelectionStatus::Disabled => "packs.disabled",
                    SelectionStatus::Unavailable => "packs.unavailable",
                    SelectionStatus::Conservative => "packs.conservative",
                }
                .into(),
                details: row.missing_capabilities.join(", ").into(),
            }
        })
        .collect();
    Ok(rows)
}

fn populate_input_packs(
    ui: &SettingsWindow,
    selected: &BTreeSet<autokeyboardlayot::PackId>,
    profiles: &autokeyboardlayot::input_profile_selection::InputProfileSelections,
    registry: &autokeyboardlayot::DictionaryRegistry,
) {
    let mut rows = match input_pack_rows(selected, profiles, registry) {
        Ok(rows) => rows,
        Err(error) => {
            ui.set_status_error(true);
            ui.set_status_text(error.into());
            return;
        }
    };
    // Keep a deselected missing row visible for the remainder of this window,
    // so the user can undo that choice before saving.
    for mut previous in ui.get_input_packs().iter() {
        if !rows.iter().any(|row| row.id == previous.id) {
            previous.selected = false;
            rows.push(previous);
        }
    }
    rows.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    ui.set_input_packs(std::rc::Rc::new(VecModel::from(rows)).into());
}

fn selected_input_packs(
    ui: &SettingsWindow,
) -> Result<BTreeSet<autokeyboardlayot::PackId>, String> {
    let rows = ui.get_input_packs();
    if rows.row_count() > autokeyboardlayot::input_pack_selection::MAX_SELECTION_ROWS {
        return Err(tr("error.pack_selection"));
    }
    autokeyboardlayot::input_pack_selection::parse_selection(
        rows.iter().map(|row| (row.id.to_string(), row.selected)),
    )
    .map_err(|_| tr("error.pack_selection"))
}

fn selected_input_profiles(
    ui: &SettingsWindow,
) -> Result<autokeyboardlayot::input_profile_selection::InputProfileSelections, String> {
    // Validate every row and ID before accepting any profile draft.
    selected_input_packs(ui)?;
    let mut choices = Vec::new();
    for row in ui.get_input_packs().iter() {
        if row.profile_ids.row_count() > 34 {
            return Err(tr("error.pack_selection"));
        }
        let index = usize::try_from(row.profile_index).map_err(|_| tr("error.pack_selection"))?;
        let profile = row
            .profile_ids
            .row_data(index)
            .ok_or_else(|| tr("error.pack_selection"))?;
        if !profile.is_empty() {
            choices.push((row.id.to_string(), profile.to_string()));
        }
    }
    autokeyboardlayot::input_profile_selection::InputProfileSelections::parse(choices)
        .map_err(|_| tr("error.pack_selection"))
}

fn refresh_ui_language(
    ui: &SettingsWindow,
    preference: &UiLanguagePreference,
    registry: &autokeyboardlayot::DictionaryRegistry,
) {
    super::ui_localization::apply_preference(preference);
    let global = ui.global::<Localization>();
    global.set_rtl(super::ui_localization::is_rtl());
    global.set_revision(global.get_revision().wrapping_add(1));
    populate_ui_language(ui, preference);
    if let (Ok(selected), Ok(profiles)) = (selected_input_packs(ui), selected_input_profiles(ui)) {
        populate_input_packs(ui, &selected, &profiles, registry);
    }
    ui.set_hotkey_capture_hint(tr("general.apply_hint").into());
    ui.invoke_refresh_download_labels();
    ui.invoke_refresh_package_labels();
}

fn wire_callbacks(
    ui: &SettingsWindow,
    document: ConfigurationDocument,
    registry: Arc<autokeyboardlayot::DictionaryRegistry>,
) -> slint::Timer {
    let document = std::rc::Rc::new(std::cell::RefCell::new(document));
    let close_document = std::rc::Rc::clone(&document);
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        let Some(ui) = weak.upgrade() else {
            return slint::CloseRequestResponse::HideWindow;
        };
        if ui.get_package_import_busy() {
            ui.set_status_error(false);
            ui.set_status_text(tr("lifecycle.settings_busy").into());
            return slint::CloseRequestResponse::KeepWindowShown;
        }
        let original = close_document.borrow();
        if !settings_edits_are_saved(document_from_ui(&ui, original.clone()), &original) {
            ui.set_status_error(false);
            ui.set_status_text(tr("lifecycle.settings_unsaved").into());
            return slint::CloseRequestResponse::KeepWindowShown;
        }
        hotkey_capture::stop();
        slint::CloseRequestResponse::HideWindow
    });
    let registry = std::rc::Rc::new(std::cell::RefCell::new(registry));
    let import_timer = package_import::wire(ui, document.clone(), registry.clone());

    let weak = ui.as_weak();
    let toggle_registry = registry.clone();
    ui.on_toggle_input_pack(move |id, enabled| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(original) = selected_input_packs(&ui) else {
            return;
        };
        let Ok(profiles) = selected_input_profiles(&ui) else {
            return;
        };
        let Ok(id) = autokeyboardlayot::PackId::parse(id.as_str()) else {
            return;
        };
        if !ui
            .get_input_packs()
            .iter()
            .any(|row| row.id.as_str() == id.as_str())
        {
            return;
        }
        let mut selected = original.clone();
        if enabled {
            selected.insert(id);
        } else {
            selected.remove(&id);
        }
        if selected.len() > 64 {
            populate_input_packs(&ui, &original, &profiles, &toggle_registry.borrow());
            ui.set_status_error(true);
            ui.set_status_text(tr("error.pack_selection").into());
        } else {
            populate_input_packs(&ui, &selected, &profiles, &toggle_registry.borrow());
            ui.set_status_error(false);
            ui.set_status_text(tr("general.apply_hint").into());
        }
    });

    let weak = ui.as_weak();
    let profile_registry = registry.clone();
    ui.on_select_input_profile(move |id, index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(selected) = selected_input_packs(&ui) else {
            return;
        };
        let Ok(original) = selected_input_profiles(&ui) else {
            return;
        };
        let model = ui.get_input_packs();
        let Some((row_index, mut row)) = model.iter().enumerate().find(|(_, row)| row.id == id)
        else {
            return;
        };
        let Ok(profile_index) = usize::try_from(index) else {
            return;
        };
        if profile_index >= row.profile_ids.row_count() {
            return;
        }
        row.profile_index = index;
        model.set_row_data(row_index, row);
        match selected_input_profiles(&ui) {
            Ok(profiles) => {
                populate_input_packs(&ui, &selected, &profiles, &profile_registry.borrow());
                ui.set_status_error(false);
                ui.set_status_text(tr("general.apply_hint").into());
            }
            Err(error) => {
                populate_input_packs(&ui, &selected, &original, &profile_registry.borrow());
                ui.set_status_error(true);
                ui.set_status_text(error.into());
            }
        }
    });

    let weak = ui.as_weak();
    ui.on_record_hotkey(move || {
        if let Some(ui) = weak.upgrade() {
            hotkey_capture::start(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_cancel_hotkey_recording(move || {
        hotkey_capture::stop();
        if let Some(ui) = weak.upgrade() {
            ui.set_hotkey_capture_hint(tr("hotkey.cancelled").into());
        }
    });
    let weak = ui.as_weak();
    ui.on_reset_hotkey(move || {
        hotkey_capture::stop();
        if let Some(ui) = weak.upgrade() {
            set_ui_hotkey(&ui, Hotkey::default());
            ui.set_hotkey_capture_hint(tr("hotkey.reset_hint").into());
        }
    });

    let weak = ui.as_weak();
    let save_document = std::rc::Rc::clone(&document);
    ui.on_save_requested(move |close_after_save| {
        hotkey_capture::stop();
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_package_import_busy() {
            return;
        }
        let original = save_document.borrow().clone();
        let document = match document_from_ui(&ui, original.clone()) {
            Ok(document) => document,
            Err(error) => {
                if let Some(page) = error.page {
                    ui.set_current_page(page);
                }
                ui.set_status_error(true);
                ui.set_status_text(error.message.into());
                return;
            }
        };
        let saved = configuration_directory_or_error()
            .and_then(|directory| save_or_merge(&directory, &document, &original));
        let document = match saved {
            Ok(Saved::AsDrafted) => document,
            Ok(Saved::Merged(merged)) => {
                // The window shows what was saved: the draft with the changes made meanwhile.
                populate_ui(&ui, &merged, &registry.borrow());
                merged
            }
            Ok(Saved::Conflict {
                document,
                current,
                conflicts,
            }) => {
                // The next Apply saves the window's values over the configuration as it is now.
                *save_document.borrow_mut() = *current;
                populate_ui(&ui, &document, &registry.borrow());
                ui.set_status_error(true);
                ui.set_status_text(
                    tr_format(
                        "settings.merge_conflict",
                        &[("fields", &conflicts.join(", "))],
                    )
                    .into(),
                );
                return;
            }
            Err(error) => {
                ui.set_status_error(true);
                ui.set_status_text(
                    tr_format("error.save_config", &[("error", &error.to_string())]).into(),
                );
                return;
            }
        };
        let preference = document.ui_language.clone();
        let unknown = unknown_language_entry(
            &document,
            ui.get_user_dictionary_text().as_str(),
            ui.get_word_exclusions_text().as_str(),
            |id| {
                registry
                    .borrow()
                    .installed_ids()
                    .any(|installed| installed == id)
            },
        );
        *save_document.borrow_mut() = document;
        refresh_ui_language(&ui, &preference, &registry.borrow());
        let notified = notify_agent();
        if let Some(unknown) = unknown {
            ui.set_current_page(unknown.page);
            ui.set_status_error(true);
            ui.set_status_text(
                tr_format(
                    "settings.saved_unknown_language",
                    &[
                        ("page", &tr(unknown.page_name)),
                        ("line", &unknown.line.to_string()),
                        ("id", &unknown.id),
                    ],
                )
                .into(),
            );
            return;
        }
        ui.set_status_error(false);
        ui.set_status_text(if notified {
            tr("settings.saved_notified").into()
        } else {
            tr("settings.saved_agent_unavailable").into()
        });
        if close_after_save {
            let _ = slint::quit_event_loop();
        }
    });

    let weak = ui.as_weak();
    ui.on_cancel_requested(move || {
        if weak
            .upgrade()
            .is_some_and(|ui| ui.get_package_import_busy())
        {
            return;
        }
        hotkey_capture::stop();
        let _ = slint::quit_event_loop();
    });

    let weak = ui.as_weak();
    ui.on_browse_executable(move || {
        hotkey_capture::stop();
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_package_import_busy() {
            return;
        }
        ui.set_package_import_busy(true);
        let selected = browse_for_executable();
        ui.set_package_import_busy(false);
        if let Some(path) = selected {
            ui.set_process_exclusions_text(
                append_unique_line(ui.get_process_exclusions_text().as_str(), &path).into(),
            );
            ui.set_status_error(false);
            ui.set_status_text(tr("process_exclusions.added").into());
        }
    });

    let weak = ui.as_weak();
    ui.on_refresh_processes(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let processes = running_process_names();
        ui.set_running_processes(strings_model(processes.iter().map(String::as_str)));
    });

    let weak = ui.as_weak();
    ui.on_add_running_process(move |process| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        ui.set_process_exclusions_text(
            append_unique_line(ui.get_process_exclusions_text().as_str(), process.as_str()).into(),
        );
        ui.set_process_picker_visible(false);
        ui.set_status_error(false);
        ui.set_status_text(tr("process_exclusions.added").into());
    });

    let weak = ui.as_weak();
    ui.on_open_log(move || {
        if let Some(ui) = weak.upgrade() {
            match open_diagnostic_log() {
                Ok(()) => {
                    ui.set_status_error(false);
                    ui.set_status_text(tr("diagnostics.opened").into());
                }
                Err(error) => {
                    ui.set_status_error(true);
                    ui.set_status_text(error.into());
                }
            }
        }
    });
    import_timer
}

fn settings_edits_are_saved(
    candidate: Result<ConfigurationDocument, DraftError>,
    original: &ConfigurationDocument,
) -> bool {
    candidate.is_ok_and(|document| document == *original)
}

const DICTIONARY_PAGE: i32 = 3;
const WORD_EXCLUSIONS_PAGE: i32 = 4;
const PROCESS_EXCLUSIONS_PAGE: i32 = 5;
const BACKENDS_PAGE: i32 = 6;

/// Why the window's draft cannot be saved, and the page that shows the problem.
#[derive(Debug, PartialEq, Eq)]
struct DraftError {
    message: String,
    page: Option<i32>,
}

impl From<String> for DraftError {
    fn from(message: String) -> Self {
        Self {
            message,
            page: None,
        }
    }
}

/// A problem on a line of the editor on `page`, named `page_name` in the navigation.
fn editor_error(page: i32, page_name: &str, line: usize, message: &str) -> DraftError {
    DraftError {
        message: tr_format(
            "error.editor_line",
            &[
                ("page", &tr(page_name)),
                ("line", &line.to_string()),
                ("error", message),
            ],
        ),
        page: Some(page),
    }
}

/// The dictionary, word-exclusion and program-exclusion editors, each read on its own: a bad
/// entry is named with its page and its line in that editor.
struct EditorLists {
    dictionary: Vec<EditorEntry>,
    word_exclusions: Vec<EditorEntry>,
    process_exclusions: Vec<EditorEntry>,
}

fn editor_lists(
    dictionary: &str,
    word_exclusions: &str,
    process_exclusions: &str,
) -> Result<EditorLists, DraftError> {
    let in_editor = |page, page_name| {
        move |error: autokeyboardlayot::ConfigurationError| {
            editor_error(page, page_name, error.line.unwrap_or(1), &error.message)
        }
    };
    let dictionary = ConfigurationDocument::lexicon_editor_entries(dictionary)
        .map_err(in_editor(DICTIONARY_PAGE, "nav.dictionary"))?;
    let word_exclusions = ConfigurationDocument::lexicon_editor_entries(word_exclusions)
        .map_err(in_editor(WORD_EXCLUSIONS_PAGE, "nav.word_exclusions"))?;
    if let Some(both) = word_exclusions
        .iter()
        .find(|word| dictionary.iter().any(|entry| entry.entry == word.entry))
    {
        return Err(editor_error(
            WORD_EXCLUSIONS_PAGE,
            "nav.word_exclusions",
            both.line,
            "this entry is also in the user dictionary",
        ));
    }
    let process_exclusions = ConfigurationDocument::process_editor_entries(process_exclusions)
        .map_err(in_editor(PROCESS_EXCLUSIONS_PAGE, "nav.process_exclusions"))?;
    Ok(EditorLists {
        dictionary,
        word_exclusions,
        process_exclusions,
    })
}

/// A dictionary or word-exclusion entry whose language is not installed: saved, but without
/// effect until the language is installed.
#[derive(Debug, PartialEq, Eq)]
struct UnknownLanguage {
    page: i32,
    page_name: &'static str,
    line: usize,
    id: String,
}

fn unknown_language_entry(
    document: &ConfigurationDocument,
    dictionary: &str,
    word_exclusions: &str,
    installed: impl Fn(&PackId) -> bool,
) -> Option<UnknownLanguage> {
    let lists = [
        (
            DICTIONARY_PAGE,
            "nav.dictionary",
            dictionary,
            &document.user_dictionary,
        ),
        (
            WORD_EXCLUSIONS_PAGE,
            "nav.word_exclusions",
            word_exclusions,
            &document.word_exclusions,
        ),
    ];
    for (page, page_name, editor, saved) in lists {
        let entries = ConfigurationDocument::lexicon_editor_entries(editor).unwrap_or_default();
        for entry in entries.iter().filter(|entry| saved.contains(&entry.entry)) {
            let Some((id, _)) = entry.entry.split_once(':') else {
                continue;
            };
            if PackId::parse(id).is_ok_and(|pack| !installed(&pack)) {
                return Some(UnknownLanguage {
                    page,
                    page_name,
                    line: entry.line,
                    id: id.to_owned(),
                });
            }
        }
    }
    None
}

/// What came of saving the window's draft.
#[derive(Debug, PartialEq, Eq)]
enum Saved {
    /// Saved as the window holds it.
    AsDrafted,
    /// The configuration changed since the window read it; the draft was merged into it and the
    /// merge, as read back, saved.
    Merged(ConfigurationDocument),
    /// The configuration changed meanwhile in values the draft changed too: nothing was saved.
    /// `document` is the merge with the draft's values, `current` the configuration now.
    Conflict {
        document: Box<ConfigurationDocument>,
        current: Box<ConfigurationDocument>,
        conflicts: Vec<String>,
    },
}

/// Saves the draft (made from `original`) into the configuration in `directory`. When the
/// configuration changed since the window read it (a word added from the tray, a hand edit),
/// the draft is merged into it and that is saved; values both changed are a conflict.
fn save_or_merge(
    directory: &Path,
    draft: &ConfigurationDocument,
    original: &ConfigurationDocument,
) -> std::io::Result<Saved> {
    match save_configuration_if_unchanged(directory, draft, original) {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        other => return other.map(|()| Saved::AsDrafted),
    }
    // Another writer may change it again between the read and the save.
    for _ in 0..3 {
        let current = load_configuration_directory(directory)?;
        let merged = ConfigurationDocument::merge(original, draft, &current);
        if !merged.conflicts.is_empty() {
            return Ok(Saved::Conflict {
                document: Box::new(merged.document),
                current: Box::new(current),
                conflicts: merged.conflicts,
            });
        }
        match save_configuration_if_unchanged(directory, &merged.document, &current) {
            Ok(()) => return load_configuration_directory(directory).map(Saved::Merged),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "Configuration keeps changing outside this window; try applying again",
    ))
}

fn document_from_ui(
    ui: &SettingsWindow,
    mut document: ConfigurationDocument,
) -> Result<ConfigurationDocument, DraftError> {
    let selected =
        usize::try_from(ui.get_ui_language_index()).map_err(|_| tr("error.ui_language"))?;
    let preference = ui
        .get_ui_language_ids()
        .row_data(selected)
        .ok_or_else(|| tr("error.ui_language"))?;
    document.ui_language = UiLanguagePreference::from_config(preference.as_str());
    document.input_profiles = selected_input_profiles(ui)?;
    let mut settings = document.settings;
    settings.automatic_conversion_on_startup = ui.get_automatic_conversion_on_startup();
    settings.pause_break_undo = ui.get_hotkey_enabled();
    settings.offer_word_exclusion_after_undo = ui.get_offer_word_exclusion_after_undo();
    settings.offer_dictionary_after_forced_conversion = ui.get_offer_dictionary_after_force();
    settings.recheck_first_word_after_erasing = ui.get_recheck_first_word_after_erasing();
    settings.single_letter_words = ui.get_single_letter_words();
    settings.layout_model = ui.get_layout_model();
    settings.manual_terminal_uia_fallback = ui.get_manual_terminal_uia_fallback();
    settings.conversion_sound = ui.get_conversion_sound();
    settings.physical_fallback_for_unsupported_apps = ui.get_physical_fallback();
    settings.diagnostics_enabled = ui.get_diagnostics_enabled();

    settings.suppress_after_backspace = ui.get_suppress_after_backspace();
    settings.suppress_after_delete = ui.get_suppress_after_delete();
    settings.suppress_after_left = ui.get_suppress_after_left();
    settings.suppress_after_right = ui.get_suppress_after_right();
    settings.suppress_after_up = ui.get_suppress_after_up();
    settings.suppress_after_down = ui.get_suppress_after_down();
    settings.suppress_after_home_end = ui.get_suppress_after_home_end();
    settings.suppress_after_manual_layout_change = ui.get_suppress_after_manual_layout();

    settings.enabled_input_packs = selected_input_packs(ui)?;

    let virtual_key =
        u16::try_from(ui.get_hotkey_key_code()).map_err(|_| tr("error.invalid_key"))?;
    let modifiers =
        u8::try_from(ui.get_hotkey_modifier_bits()).map_err(|_| tr("error.invalid_shortcut"))?;
    let hotkey = Hotkey::new(virtual_key, modifiers)
        .map_err(|error| tr_format("error.invalid_hotkey", &[("error", &error.to_string())]))?;
    settings.force_hotkey_virtual_key = hotkey.virtual_key;
    settings.force_hotkey_modifiers = hotkey.modifiers;
    document.settings = settings;

    let lists = editor_lists(
        ui.get_user_dictionary_text().as_str(),
        ui.get_word_exclusions_text().as_str(),
        ui.get_process_exclusions_text().as_str(),
    )?;
    let entries = |list: Vec<EditorEntry>| list.into_iter().map(|entry| entry.entry).collect();
    document.user_dictionary = entries(lists.dictionary);
    document.word_exclusions = entries(lists.word_exclusions);
    document.process_exclusions = entries(lists.process_exclusions);
    document.backend_rules = BackendRules::from_lines(ui.get_backend_rules_text().lines())
        .map_err(|error| DraftError {
            message: tr_format("error.backend_rules", &[("error", &error.to_string())]),
            page: Some(BACKENDS_PAGE),
        })?;
    document
        .canonicalize_and_validate()
        .map_err(|error| tr_format("error.settings", &[("error", &error.to_string())]))?;
    Ok(document)
}

fn strings_model<'a>(values: impl IntoIterator<Item = &'a str>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

fn lines_to_editor(lines: &[String]) -> SharedString {
    if lines.is_empty() {
        SharedString::new()
    } else {
        format!("{}\n", lines.join("\n")).into()
    }
}

fn append_unique_line(existing: &str, entry: &str) -> String {
    let entry = entry.trim();
    let mut lines: Vec<_> = existing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if !lines.iter().any(|line| line.eq_ignore_ascii_case(entry)) {
        lines.push(entry.to_owned());
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn browse_for_executable() -> Option<String> {
    let filter: Vec<u16> = format!(
        "{}\0*.exe\0{}\0*.*\0\0",
        tr("dialog.executables"),
        tr("dialog.all_files")
    )
    .encode_utf16()
    .collect();
    let title: Vec<u16> = tr("dialog.choose_executable")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut file = vec![0u16; 32_768];
    let mut dialog = OPENFILENAMEW {
        lStructSize: u32::try_from(size_of::<OPENFILENAMEW>()).ok()?,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: u32::try_from(file.len()).ok()?,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&raw mut dialog) }.as_bool() {
        return None;
    }
    let length = file.iter().position(|character| *character == 0)?;
    let path = String::from_utf16(&file[..length]).ok()?;
    std::path::Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn running_process_names() -> Vec<String> {
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return Vec::new();
    };
    struct Snapshot(HANDLE);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let snapshot = Snapshot(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or_default(),
        ..Default::default()
    };
    if unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_err() {
        return Vec::new();
    }
    let mut names = BTreeSet::new();
    loop {
        let length = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        if let Ok(name) = String::from_utf16(&entry.szExeFile[..length])
            && !name.is_empty()
        {
            names.insert(name);
        }
        if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
            break;
        }
    }
    names.into_iter().collect()
}

fn open_diagnostic_log() -> Result<(), String> {
    let path = diagnostic_log_path().ok_or_else(|| tr("error.local_app_data").to_owned())?;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    let path = HSTRING::from(path.as_os_str());
    let result = unsafe { ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL) };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(tr("error.open_log").to_owned())
    }
}

fn find_window_by_class(class: PCWSTR) -> Option<HWND> {
    // The observer caption is a live tray/status string, not WINDOW_TITLE.
    unsafe { FindWindowW(class, PCWSTR::null()).ok() }.filter(|window| !window.0.is_null())
}

fn notify_agent() -> bool {
    let Some(window) = find_window_by_class(WINDOW_CLASS) else {
        return false;
    };
    unsafe { PostMessageW(Some(window), SETTINGS_APPLIED_MESSAGE, WPARAM(0), LPARAM(0)).is_ok() }
}

fn focus_existing_window() {
    let class = HSTRING::from(SETTINGS_WINDOW_CLASS);
    let Some(window) = find_window_by_class(PCWSTR(class.as_ptr())) else {
        return;
    };
    unsafe {
        // A native popup may share the backend class; focus its owning window.
        let owner = GetAncestor(window, GA_ROOTOWNER);
        let window = if owner.0.is_null() { window } else { owner };
        let _ = ShowWindow(window, SW_RESTORE);
        let _ = SetForegroundWindow(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_and_saved_selection_do_not_reenable_an_unchecked_language() {
        let registry = autokeyboardlayot::DictionaryRegistry::embedded();
        let mut selected = registry.installed_ids().copied().collect::<BTreeSet<_>>();
        selected.remove(&autokeyboardlayot::Language::English);
        for _ in 0..3 {
            let rows = input_pack_rows(&selected, &Default::default(), &registry).unwrap();
            let english = rows.iter().find(|row| row.id == "en-us").unwrap();
            assert!(!english.selected);
            assert_eq!(english.status_key, "packs.disabled");
            selected = autokeyboardlayot::input_pack_selection::parse_selection(
                rows.iter().map(|row| (row.id.to_string(), row.selected)),
            )
            .unwrap();
        }
        let saved = autokeyboardlayot::Settings {
            enabled_input_packs: selected,
            ..Default::default()
        };
        let reopened = autokeyboardlayot::Settings::from_text(&saved.to_text());
        assert!(
            !input_pack_rows(
                &reopened.enabled_input_packs,
                &Default::default(),
                &registry
            )
            .unwrap()
            .iter()
            .find(|row| row.id == "en-us")
            .unwrap()
            .selected
        );
    }

    #[test]
    fn a_bad_editor_line_names_its_page_and_its_line_in_that_editor() {
        let dictionary = "# my words\nen-US: firefox\n\nen-US: two words\n";
        let error = editor_lists(dictionary, "", "").err().unwrap();
        assert_eq!(error.page, Some(DICTIONARY_PAGE));
        assert!(
            error.message.starts_with("User dictionary, line 4: "),
            "{}",
            error.message
        );
        // The same mistake in the word exclusions names that page.
        let error = editor_lists("", dictionary, "").err().unwrap();
        assert_eq!(error.page, Some(WORD_EXCLUSIONS_PAGE));
        assert!(
            error.message.starts_with("Word exclusions, line 4: "),
            "{}",
            error.message
        );
        let error = editor_lists("", "", "private.exe\n\n\"#vault.exe\"\n")
            .err()
            .unwrap();
        assert_eq!(error.page, Some(PROCESS_EXCLUSIONS_PAGE));
        assert!(error.message.contains(", line 3: "), "{}", error.message);
        // An entry in both lists is named at its line in the exclusions.
        let error = editor_lists("en-US: firefox\n", "\nru-RU: x\nen-US: Firefox\n", "")
            .err()
            .unwrap();
        assert_eq!(error.page, Some(WORD_EXCLUSIONS_PAGE));
        assert!(error.message.contains(", line 3: "), "{}", error.message);
        let lists = editor_lists(
            "en-US: Firefox\n",
            "ru-RU: руддщ\n",
            "C:\\Apps\\Vault.exe\n",
        )
        .unwrap();
        assert_eq!(lists.dictionary[0].entry, "en-US: firefox");
        assert_eq!(lists.process_exclusions[0].entry, "vault.exe");
    }

    #[test]
    fn an_entry_for_a_language_that_is_not_installed_is_reported_after_saving() {
        let document = ConfigurationDocument {
            user_dictionary: vec!["de: hallo".to_owned(), "en-US: firefox".to_owned()],
            ..ConfigurationDocument::default()
        };
        let english = PackId::parse("en-US").unwrap();
        let unknown =
            unknown_language_entry(&document, "en-US: firefox\n\nde: Hallo\n", "", |id| {
                *id == english
            });
        assert_eq!(
            unknown,
            Some(UnknownLanguage {
                page: DICTIONARY_PAGE,
                page_name: "nav.dictionary",
                line: 3,
                id: "de".to_owned(),
            })
        );
        assert_eq!(
            unknown_language_entry(&document, "de: Hallo\n", "", |_| true),
            None
        );
    }

    /// A configuration folder holding `document`.
    fn folder_with(document: &ConfigurationDocument) -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            folder.path().join("config.ini"),
            document.to_text().unwrap(),
        )
        .unwrap();
        folder
    }

    #[test]
    fn a_draft_merges_with_a_word_added_from_the_tray_while_the_window_was_open() {
        // The owner's case: the window is open, a word is added from the tray, then a box is
        // unticked in the window and Apply is selected.
        let base = ConfigurationDocument::default();
        let folder = folder_with(&base);
        let offer = super::super::VolatileLexiconCandidate::new(
            super::super::LexiconOfferTarget::UserDictionary,
            autokeyboardlayot::Language::English,
            "firefox".to_owned(),
            std::time::Instant::now(),
        );
        super::super::persist_lexicon_candidate(folder.path(), &offer).unwrap();
        let mut draft = base.clone();
        draft.settings.suppress_after_manual_layout_change =
            !base.settings.suppress_after_manual_layout_change;
        let saved = save_or_merge(folder.path(), &draft, &base).unwrap();
        let on_disk = load_configuration_directory(folder.path()).unwrap();
        assert_eq!(saved, Saved::Merged(on_disk.clone()));
        assert_eq!(
            on_disk.settings.suppress_after_manual_layout_change,
            !base.settings.suppress_after_manual_layout_change
        );
        assert_eq!(on_disk.user_dictionary, ["en-US: firefox"]);
        // Unchanged since, the next draft is saved as it is.
        let mut next = on_disk.clone();
        next.settings.layout_model = !next.settings.layout_model;
        assert_eq!(
            save_or_merge(folder.path(), &next, &on_disk).unwrap(),
            Saved::AsDrafted
        );
    }

    #[test]
    fn a_value_changed_in_the_window_and_outside_it_is_not_saved_over() {
        let base = ConfigurationDocument::default();
        let mut outside = base.clone();
        outside.ui_language = UiLanguagePreference::from_config("et");
        let folder = folder_with(&outside);
        let mut draft = base.clone();
        draft.ui_language = UiLanguagePreference::from_config("ru");
        let Saved::Conflict {
            document,
            current,
            conflicts,
        } = save_or_merge(folder.path(), &draft, &base).unwrap()
        else {
            panic!("no conflict");
        };
        assert_eq!(conflicts, ["ui_language"]);
        assert_eq!(document.ui_language.as_config(), "ru");
        assert_eq!(current.ui_language.as_config(), "et");
        assert_eq!(
            load_configuration_directory(folder.path())
                .unwrap()
                .ui_language
                .as_config(),
            "et"
        );
        // With the configuration as it is now as the base, the window's values are saved.
        assert_eq!(
            save_or_merge(folder.path(), &document, &current).unwrap(),
            Saved::AsDrafted
        );
    }

    #[test]
    fn normal_close_requires_valid_unchanged_settings() {
        let original = ConfigurationDocument::default();
        assert!(settings_edits_are_saved(Ok(original.clone()), &original));
        assert!(!settings_edits_are_saved(
            Err(DraftError::from("invalid".to_owned())),
            &original
        ));
        let mut edited = original.clone();
        edited.settings.automatic_conversion_on_startup =
            !edited.settings.automatic_conversion_on_startup;
        assert!(!settings_edits_are_saved(Ok(edited.clone()), &original));
        // The Apply callback updates the shared baseline after a successful save.
        assert!(settings_edits_are_saved(Ok(edited.clone()), &edited));
        let mut changed_exclusions = original.clone();
        changed_exclusions
            .process_exclusions
            .push("example.exe".into());
        assert!(!settings_edits_are_saved(Ok(changed_exclusions), &original));
    }

    #[test]
    fn agent_lookup_does_not_depend_on_the_live_status_caption() {
        use windows::Win32::{
            Foundation::{HINSTANCE, LRESULT},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowTextW,
                UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
            },
        };
        unsafe extern "system" fn procedure(
            hwnd: HWND,
            message: u32,
            wp: WPARAM,
            lp: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, message, wp, lp) }
        }
        let class = w!("AutoKeyboardLayot.NotifyRegression");
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
            let definition = WNDCLASSW {
                lpszClassName: class,
                hInstance: instance,
                lpfnWndProc: Some(procedure),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&definition), 0);
            let window = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!("AutoKeyboardLayot - ENG - active; live status"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )
            .unwrap();
            assert!(FindWindowW(class, super::super::WINDOW_TITLE).is_err());
            assert_eq!(find_window_by_class(class), Some(window));
            for title in [
                "AutoKeyboardLayot — Settings",
                "AutoKeyboardLayot — Seaded",
                "AutoKeyboardLayot — 設定",
            ] {
                SetWindowTextW(window, &HSTRING::from(title)).unwrap();
                assert_eq!(find_window_by_class(class), Some(window));
            }
            DestroyWindow(window).unwrap();
            UnregisterClassW(class, Some(instance)).unwrap();
        }
    }

    #[test]
    fn appending_processes_is_case_insensitive_and_canonical() {
        let text = append_unique_line("firefox.exe\n", "FIREFOX.EXE");
        assert_eq!(text, "firefox.exe\n");
        let text = append_unique_line(&text, "notepad.exe");
        assert_eq!(text, "firefox.exe\nnotepad.exe\n");
    }
}
