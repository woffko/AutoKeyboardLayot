//! The settings window on Linux: the shared Slint interface (ui/settings.slint) with the
//! Windows-only controls hidden. It edits `config.ini` and saves it atomically; the engine reloads
//! the configuration when the next field takes the focus, so no message to the engine is needed.

use super::{
    localization::{self, tr, tr_format},
    paths,
};
use autokeyboardlayot::{
    ConfigurationDocument, DictionaryRegistry, Hotkey, PackId,
    configuration::{load_configuration_snapshot, prepare_configuration_write},
    hotkey::{HOTKEY_MOD_ALT, HOTKEY_MOD_CONTROL, HOTKEY_MOD_SHIFT, HOTKEY_MOD_WIN},
    input_pack_selection::{
        MAX_SELECTION_ROWS, SelectionStatus, parse_selection, selection_rows_with_profiles,
    },
    input_profile_selection::InputProfileSelections,
    installed_packages::InstalledPackages,
    language_package::PackageTrust,
    localization::UiLanguagePreference,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs::File,
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

slint::include_modules!();

/// An exclusive advisory lock on a file, released when dropped.
struct FileLock {
    _file: File,
}

impl FileLock {
    fn acquire(path: &Path, wait: bool) -> io::Result<Option<Self>> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        let operation = if wait {
            libc::LOCK_EX
        } else {
            libc::LOCK_EX | libc::LOCK_NB
        };
        // SAFETY: the descriptor belongs to `file`, which lives as long as the returned lock; flock
        // has no other preconditions and reports failure through its return value.
        let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
        if result == 0 {
            return Ok(Some(Self { _file: file }));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

/// Lets one settings window run at a time.
fn instance_lock_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(paths::state_dir)
        .map(|directory| directory.join("autokeyboardlayot-settings.lock"))
}

fn load_document(config_dir: &Path) -> io::Result<ConfigurationDocument> {
    load_configuration_snapshot(config_dir).map(|loaded| loaded.document)
}

fn load_packages(document: &ConfigurationDocument) -> io::Result<InstalledPackages> {
    let data = paths::data_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let trust = PackageTrust::release().map_err(io::Error::other)?;
    InstalledPackages::load(
        &data.join("packages"),
        document.package_mode,
        &trust,
        &document.settings.enabled_input_packs,
    )
    .map_err(io::Error::other)
}

/// Writes `document` unless the file changed since `original` was read, under the writers' lock.
fn save_if_unchanged(
    config_dir: &Path,
    document: &ConfigurationDocument,
    original: &ConfigurationDocument,
) -> io::Result<()> {
    let _lock = FileLock::acquire(&config_dir.join("config.lock"), true)?;
    if &load_document(config_dir)? != original {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the configuration changed outside this window; reopen settings before saving",
        ));
    }
    let contents = document
        .to_text()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let path = config_dir.join("config.ini");
    let temporary = prepare_configuration_write(&path, &contents)?;
    std::fs::rename(&temporary, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

pub fn run() -> Result<(), String> {
    let lock_path = instance_lock_path().ok_or("HOME is not set")?;
    let Some(_instance) =
        FileLock::acquire(&lock_path, false).map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let config_dir = paths::config_dir().ok_or("HOME is not set")?;
    let document = load_document(&config_dir)
        .map_err(|error| tr_format("error.read_config", &[("error", &error.to_string())]))?;
    let packages = load_packages(&document)
        .map_err(|error| tr_format("error.read_config", &[("error", &error.to_string())]))?;
    localization::initialize(&document.ui_language, packages.catalogs.as_deref());
    let ui = SettingsWindow::new().map_err(|error| error.to_string())?;
    ui.global::<Localization>()
        .on_text(|id, _revision| tr(id.as_str()).into());
    ui.global::<Localization>().set_revision(1);
    ui.global::<Localization>().set_rtl(localization::is_rtl());
    ui.set_windows_features(false);
    ui.set_app_version(super::VERSION.into());
    populate(&ui, &document, &packages.dictionaries);
    wire(&ui, config_dir, document, packages.dictionaries);
    ui.run().map_err(|error| error.to_string())
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

fn editor_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn set_hotkey(ui: &SettingsWindow, hotkey: Hotkey) {
    ui.set_hotkey_key_code(i32::from(hotkey.virtual_key));
    ui.set_hotkey_modifier_bits(i32::from(hotkey.modifiers));
    ui.set_hotkey_label(hotkey.display_name().into());
}

fn populate(ui: &SettingsWindow, document: &ConfigurationDocument, registry: &DictionaryRegistry) {
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
    // Hidden on Linux, kept so that saving never drops them.
    ui.set_process_exclusions_text(lines_to_editor(&document.process_exclusions));
    ui.set_backend_rules_text(document.backend_rules.to_text().into());
    set_hotkey(ui, settings.force_hotkey());
    ui.set_running_processes(strings_model(std::iter::empty::<&str>()));
}

fn populate_ui_language(ui: &SettingsWindow, preference: &UiLanguagePreference) {
    let (choices, selected) = localization::picker_choices(preference);
    ui.set_ui_language_ids(strings_model(
        choices.iter().map(|choice| choice.id.as_str()),
    ));
    ui.set_ui_language_names(strings_model(
        choices.iter().map(|choice| choice.name.as_str()),
    ));
    ui.set_ui_language_index(i32::try_from(selected).unwrap_or(0));
    ui.set_ui_language_status(
        tr_format(
            "ui_language.active",
            &[("language", &localization::active_language_name())],
        )
        .into(),
    );
}

fn pack_display_name(id: &str) -> String {
    match id.to_ascii_lowercase().as_str() {
        "en-us" => "English",
        "ru-ru" => "Русский",
        "et-ee" => "Eesti",
        _ => return id.to_owned(),
    }
    .to_owned()
}

fn populate_input_packs(
    ui: &SettingsWindow,
    selected: &BTreeSet<PackId>,
    profiles: &InputProfileSelections,
    registry: &DictionaryRegistry,
) {
    let rows = match selection_rows_with_profiles(registry, selected, profiles) {
        Ok(rows) => rows,
        Err(_) => {
            ui.set_status_error(true);
            ui.set_status_text(tr("error.pack_selection").into());
            return;
        }
    };
    let mut rows: Vec<InputPackRow> = rows
        .into_iter()
        .map(|row| {
            let mut ids = vec![String::new()];
            ids.extend(row.profiles.iter().map(ToString::to_string));
            let chosen = row
                .chosen_profile
                .map(|profile| profile.to_string())
                .unwrap_or_default();
            let profile_index = ids.iter().position(|id| *id == chosen).unwrap_or(0);
            let mut names = ids.clone();
            names[0] = tr("packs.profile_default");
            InputPackRow {
                profile_ids: strings_model(ids.iter().map(String::as_str)),
                profile_names: strings_model(names.iter().map(String::as_str)),
                profile_index: i32::try_from(profile_index).unwrap_or(0),
                id: row.id.as_str().into(),
                name: pack_display_name(row.id.as_str()).into(),
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
    // A deselected missing row stays visible so that the choice can be undone before saving.
    for mut previous in ui.get_input_packs().iter() {
        if !rows.iter().any(|row| row.id == previous.id) {
            previous.selected = false;
            rows.push(previous);
        }
    }
    rows.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    ui.set_input_packs(Rc::new(VecModel::from(rows)).into());
}

fn selected_input_packs(ui: &SettingsWindow) -> Result<BTreeSet<PackId>, String> {
    let rows = ui.get_input_packs();
    if rows.row_count() > MAX_SELECTION_ROWS {
        return Err(tr("error.pack_selection"));
    }
    parse_selection(rows.iter().map(|row| (row.id.to_string(), row.selected)))
        .map_err(|_| tr("error.pack_selection"))
}

fn selected_input_profiles(ui: &SettingsWindow) -> Result<InputProfileSelections, String> {
    selected_input_packs(ui)?;
    let mut choices = Vec::new();
    for row in ui.get_input_packs().iter() {
        let index = usize::try_from(row.profile_index).map_err(|_| tr("error.pack_selection"))?;
        let profile = row
            .profile_ids
            .row_data(index)
            .ok_or_else(|| tr("error.pack_selection"))?;
        if !profile.is_empty() {
            choices.push((row.id.to_string(), profile.to_string()));
        }
    }
    InputProfileSelections::parse(choices).map_err(|_| tr("error.pack_selection"))
}

fn document_from_ui(
    ui: &SettingsWindow,
    mut document: ConfigurationDocument,
) -> Result<ConfigurationDocument, String> {
    let selected =
        usize::try_from(ui.get_ui_language_index()).map_err(|_| tr("error.ui_language"))?;
    let preference = ui
        .get_ui_language_ids()
        .row_data(selected)
        .ok_or_else(|| tr("error.ui_language"))?;
    document.ui_language = UiLanguagePreference::from_config(preference.as_str());
    document.input_profiles = selected_input_profiles(ui)?;
    let settings = &mut document.settings;
    settings.automatic_conversion_on_startup = ui.get_automatic_conversion_on_startup();
    settings.pause_break_undo = ui.get_hotkey_enabled();
    settings.offer_word_exclusion_after_undo = ui.get_offer_word_exclusion_after_undo();
    settings.offer_dictionary_after_forced_conversion = ui.get_offer_dictionary_after_force();
    settings.recheck_first_word_after_erasing = ui.get_recheck_first_word_after_erasing();
    settings.single_letter_words = ui.get_single_letter_words();
    settings.layout_model = ui.get_layout_model();
    settings.conversion_sound = ui.get_conversion_sound();
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
    document.user_dictionary = editor_lines(ui.get_user_dictionary_text().as_str());
    document.word_exclusions = editor_lines(ui.get_word_exclusions_text().as_str());
    document
        .canonicalize_and_validate()
        .map_err(|error| tr_format("error.settings", &[("error", &error.to_string())]))?;
    Ok(document)
}

/// The hotkey for a key pressed while recording: `None` for a modifier alone (keep recording),
/// `Some(Err)` for a key that cannot be a hotkey, `Some(Ok)` otherwise.
pub fn captured_hotkey(
    text: &str,
    control: bool,
    alt: bool,
    shift: bool,
    meta: bool,
) -> Option<Result<Hotkey, String>> {
    use slint::platform::Key;
    let mut characters = text.chars();
    let (Some(key), None) = (characters.next(), characters.next()) else {
        return Some(Err(String::new()));
    };
    let is = |special: Key| SharedString::from(special).starts_with(key);
    if [
        Key::Shift,
        Key::ShiftR,
        Key::Control,
        Key::ControlR,
        Key::Alt,
        Key::AltGr,
        Key::Meta,
        Key::MetaR,
    ]
    .into_iter()
    .any(is)
    {
        return None;
    }
    let function_keys = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::F13,
        Key::F14,
        Key::F15,
        Key::F16,
        Key::F17,
        Key::F18,
        Key::F19,
        Key::F20,
        Key::F21,
        Key::F22,
        Key::F23,
        Key::F24,
    ];
    let virtual_key = if is(Key::Pause) {
        0x13
    } else if is(Key::Insert) {
        0x2d
    } else if is(Key::ScrollLock) {
        0x91
    } else if let Some(index) = function_keys.into_iter().position(is) {
        0x70 + u16::try_from(index).unwrap_or(0)
    } else if key.is_ascii_alphanumeric() {
        u16::from(key.to_ascii_uppercase() as u8)
    } else {
        return Some(Err(String::new()));
    };
    let mut modifiers = 0;
    for (held, bit) in [
        (control, HOTKEY_MOD_CONTROL),
        (alt, HOTKEY_MOD_ALT),
        (shift, HOTKEY_MOD_SHIFT),
        (meta, HOTKEY_MOD_WIN),
    ] {
        if held {
            modifiers |= bit;
        }
    }
    Some(Hotkey::new(virtual_key, modifiers).map_err(|error| error.to_string()))
}

fn status(ui: &SettingsWindow, error: bool, text: String) {
    ui.set_status_error(error);
    ui.set_status_text(text.into());
}

fn wire(
    ui: &SettingsWindow,
    config_dir: PathBuf,
    document: ConfigurationDocument,
    registry: Arc<DictionaryRegistry>,
) {
    let document = Rc::new(RefCell::new(document));
    let registry = Rc::new(registry);

    let weak = ui.as_weak();
    let close_document = Rc::clone(&document);
    ui.window().on_close_requested(move || {
        let Some(ui) = weak.upgrade() else {
            return slint::CloseRequestResponse::HideWindow;
        };
        let original = close_document.borrow();
        if document_from_ui(&ui, original.clone()).is_ok_and(|candidate| candidate == *original) {
            slint::CloseRequestResponse::HideWindow
        } else {
            status(&ui, false, tr("lifecycle.settings_unsaved"));
            slint::CloseRequestResponse::KeepWindowShown
        }
    });

    let weak = ui.as_weak();
    let toggle_registry = Rc::clone(&registry);
    ui.on_toggle_input_pack(move |id, enabled| {
        let Some(ui) = weak.upgrade() else { return };
        let (Ok(original), Ok(profiles), Ok(id)) = (
            selected_input_packs(&ui),
            selected_input_profiles(&ui),
            PackId::parse(id.as_str()),
        ) else {
            return;
        };
        let mut selected = original.clone();
        if enabled {
            selected.insert(id);
        } else {
            selected.remove(&id);
        }
        populate_input_packs(&ui, &selected, &profiles, &toggle_registry);
        status(&ui, false, tr("general.apply_hint"));
    });

    let weak = ui.as_weak();
    let profile_registry = Rc::clone(&registry);
    ui.on_select_input_profile(move |id, index| {
        let Some(ui) = weak.upgrade() else { return };
        let Ok(selected) = selected_input_packs(&ui) else {
            return;
        };
        let model = ui.get_input_packs();
        let Some((row_index, mut row)) = model.iter().enumerate().find(|(_, row)| row.id == id)
        else {
            return;
        };
        if usize::try_from(index).is_ok_and(|index| index < row.profile_ids.row_count()) {
            row.profile_index = index;
            model.set_row_data(row_index, row);
        }
        if let Ok(profiles) = selected_input_profiles(&ui) {
            populate_input_packs(&ui, &selected, &profiles, &profile_registry);
        }
        status(&ui, false, tr("general.apply_hint"));
    });

    let weak = ui.as_weak();
    ui.on_record_hotkey(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_hotkey_recording(true);
            ui.set_hotkey_capture_hint(tr("hotkey.record_hint").into());
        }
    });
    let weak = ui.as_weak();
    ui.on_cancel_hotkey_recording(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_hotkey_recording(false);
            ui.set_hotkey_capture_hint(tr("hotkey.cancelled").into());
        }
    });
    let weak = ui.as_weak();
    ui.on_reset_hotkey(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_hotkey_recording(false);
            set_hotkey(&ui, Hotkey::default());
            ui.set_hotkey_capture_hint(tr("hotkey.reset_hint").into());
        }
    });
    let weak = ui.as_weak();
    ui.on_key_captured(move |text, control, alt, shift, meta| {
        let Some(ui) = weak.upgrade() else { return };
        if text.as_str() == SharedString::from(slint::platform::Key::Escape).as_str() {
            ui.set_hotkey_recording(false);
            ui.set_hotkey_capture_hint(tr("hotkey.cancelled").into());
            return;
        }
        match captured_hotkey(text.as_str(), control, alt, shift, meta) {
            None => {}
            Some(Ok(hotkey)) => {
                ui.set_hotkey_recording(false);
                set_hotkey(&ui, hotkey);
                ui.set_hotkey_capture_hint(
                    tr_format("hotkey.selected", &[("hotkey", &hotkey.display_name())]).into(),
                );
            }
            Some(Err(_)) => ui.set_hotkey_capture_hint(tr("hotkey.modifier_required").into()),
        }
    });

    let weak = ui.as_weak();
    let save_document = Rc::clone(&document);
    ui.on_save_requested(move |close_after_save| {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_hotkey_recording(false);
        let original = save_document.borrow().clone();
        let candidate = match document_from_ui(&ui, original.clone()) {
            Ok(candidate) => candidate,
            Err(error) => return status(&ui, true, error),
        };
        if let Err(error) = save_if_unchanged(&config_dir, &candidate, &original) {
            return status(
                &ui,
                true,
                tr_format("error.save_config", &[("error", &error.to_string())]),
            );
        }
        let preference = candidate.ui_language.clone();
        *save_document.borrow_mut() = candidate;
        localization::apply_preference(&preference);
        let global = ui.global::<Localization>();
        global.set_rtl(localization::is_rtl());
        global.set_revision(global.get_revision().wrapping_add(1));
        populate_ui_language(&ui, &preference);
        status(&ui, false, tr("settings.saved_notified"));
        if close_after_save {
            let _ = slint::quit_event_loop();
        }
    });

    ui.on_cancel_requested(|| {
        let _ = slint::quit_event_loop();
    });

    let weak = ui.as_weak();
    ui.on_open_log(move || {
        let Some(ui) = weak.upgrade() else { return };
        let opened = paths::state_dir()
            .map(|directory| directory.join("diagnostics.log"))
            .filter(|path| path.exists())
            .and_then(|path| {
                std::process::Command::new("xdg-open")
                    .arg(path)
                    .spawn()
                    .ok()
            });
        match opened {
            Some(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                status(&ui, false, tr("diagnostics.opened"));
            }
            None => status(&ui, true, tr("error.open_log")),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::Key;

    #[test]
    fn a_recorded_key_becomes_the_same_hotkey_as_on_windows() {
        let key = |special: Key| SharedString::from(special).to_string();
        assert_eq!(
            captured_hotkey(&key(Key::Pause), false, false, false, false)
                .unwrap()
                .unwrap(),
            Hotkey::default()
        );
        let control_f9 = captured_hotkey(&key(Key::F9), true, false, false, false)
            .unwrap()
            .unwrap();
        assert_eq!(
            (control_f9.virtual_key, control_f9.modifiers),
            (0x78, HOTKEY_MOD_CONTROL)
        );
        let alt_k = captured_hotkey("k", false, true, false, false)
            .unwrap()
            .unwrap();
        assert_eq!((alt_k.virtual_key, alt_k.modifiers), (0x4b, HOTKEY_MOD_ALT));
        // A modifier alone keeps recording; a letter without a modifier is refused.
        assert!(captured_hotkey(&key(Key::Control), true, false, false, false).is_none());
        assert!(
            captured_hotkey("k", false, false, false, false)
                .unwrap()
                .is_err()
        );
        assert!(
            captured_hotkey(&key(Key::Tab), false, false, false, false)
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn editor_text_round_trips_and_drops_comments_and_blank_lines() {
        let lines = editor_lines("  привет \n\n# note\nhello\n");
        assert_eq!(lines, ["привет", "hello"]);
        assert_eq!(lines_to_editor(&lines).as_str(), "привет\nhello\n");
        assert_eq!(lines_to_editor(&[]).as_str(), "");
    }

    #[test]
    fn saving_refuses_a_file_changed_meanwhile_and_writes_atomically() {
        let root = tempfile::tempdir().unwrap();
        let original = ConfigurationDocument::default();
        let mut edited = original.clone();
        edited.settings.diagnostics_enabled = true;
        save_if_unchanged(root.path(), &edited, &original).unwrap();
        assert_eq!(load_document(root.path()).unwrap(), edited);
        // The window still holds the old original: the newer file is not overwritten.
        let mut other = original.clone();
        other.settings.layout_model = true;
        let error = save_if_unchanged(root.path(), &other, &original).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(load_document(root.path()).unwrap(), edited);
        // The lock of a second window is refused without waiting.
        let path = root.path().join("instance.lock");
        let first = FileLock::acquire(&path, false).unwrap();
        assert!(first.is_some());
        assert!(FileLock::acquire(&path, false).unwrap().is_none());
    }
}
