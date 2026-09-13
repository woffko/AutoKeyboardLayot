//! Bounded configuration loading and explicit pre-migration backup.

use std::{
    fs::OpenOptions,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use super::ConfigurationDocument;
use crate::Settings;

pub const CONFIGURATION_MAX_BYTES: u64 = 1024 * 1024;

/// Provenance of successfully read files, not a second path-existence probe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfigurationSources(u8);

impl ConfigurationSources {
    pub const UNIFIED: Self = Self(1);

    /// A unified document may replace legacy files. Disappearance of a used
    /// file is not an implicit request to reset operational settings.
    pub fn accepts_reload(self, next: Self) -> bool {
        next == Self::UNIFIED || (self.0 & next.0) == self.0
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfiguration {
    pub document: ConfigurationDocument,
    pub sources: ConfigurationSources,
}

pub fn load_configuration_directory(directory: &Path) -> io::Result<ConfigurationDocument> {
    load_configuration_snapshot(directory).map(|loaded| loaded.document)
}

pub fn load_configuration_snapshot(directory: &Path) -> io::Result<LoadedConfiguration> {
    load_with_sources(|name| read_optional_file(&directory.join(name)))
}

/// Called only during an explicit save, before replacing an older document.
/// Never overwrites an earlier backup or follows a link at its destination.
/// Standalone legacy files are retained in place by unified-config migration.
pub fn backup_before_schema_upgrade(path: &Path) -> io::Result<()> {
    let Some(original) = read_optional_file(path)? else {
        return Ok(());
    };
    ConfigurationDocument::from_text(&original)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let schema = original
        .lines()
        .take_while(|line| !line.trim().starts_with('['))
        .filter_map(|line| line.trim().split_once('='))
        .find_map(|(key, value)| {
            (key.trim() == "schema_version")
                .then(|| value.trim().parse::<u32>().ok())
                .flatten()
        })
        .expect("validated schema header");
    if schema >= super::CONFIGURATION_SCHEMA_VERSION {
        return Ok(());
    }
    let backup = path.with_extension(format!("schema-{schema}.bak"));
    write_exact_backup(&backup, &original)
}

fn write_exact_backup(backup: &Path, original: &str) -> io::Result<()> {
    if let Some(existing) = read_optional_file(backup)? {
        return if existing == original {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a different migration backup already exists; migration was not saved",
            ))
        };
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(backup)?;
    file.write_all(original.as_bytes())?;
    file.sync_all()
}

/// An explicit legacy-to-managed transition also needs an exact backup when
/// both documents already use the current schema. Legacy standalone files are
/// retained in place if no unified document exists.
fn backup_before_package_migration(path: &Path, next: &ConfigurationDocument) -> io::Result<()> {
    if next.package_mode != super::PackageMode::Managed {
        return Ok(());
    }
    let Some(original) = read_optional_file(path)? else {
        return Ok(());
    };
    let document = ConfigurationDocument::from_text(&original)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if document.package_mode == super::PackageMode::LegacyBootstrap {
        write_exact_backup(&path.with_extension("packages-legacy.bak"), &original)?;
    }
    Ok(())
}

/// Prepare an explicit save without touching the destination. The caller must
/// hold its writer lock and perform platform-specific atomic replacement.
/// Existing temporary files (including links) are never truncated or followed.
pub fn prepare_configuration_write(path: &Path, contents: &str) -> io::Result<PathBuf> {
    if contents.len() as u64 > CONFIGURATION_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration exceeds 1 MiB",
        ));
    }
    let next = ConfigurationDocument::from_text(contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let directory = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration path has no parent",
        )
    })?;
    std::fs::create_dir_all(directory)?;
    backup_before_schema_upgrade(path)?;
    backup_before_package_migration(path, &next)?;
    let temporary = path.with_extension("tmp");
    if temporary == path {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "temporary path would replace the destination",
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(temporary)
}

fn read_optional_file(path: &Path) -> io::Result<Option<String>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        };
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
        options.share_mode((FILE_SHARE_READ | FILE_SHARE_DELETE).0);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A dangling link is not a clean first launch.
            return match std::fs::symlink_metadata(path) {
                Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(None),
                _ => Err(error),
            };
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_TYPE_DISK, GetFileType},
        };
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            || unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration must be a regular disk file",
            ));
        }
    }
    if !metadata.is_file() || metadata.len() > CONFIGURATION_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration is not a regular file or exceeds 1 MiB",
        ));
    }
    let mut contents = String::new();
    file.take(CONFIGURATION_MAX_BYTES + 1)
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > CONFIGURATION_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration exceeds 1 MiB",
        ));
    }
    Ok(Some(contents))
}

#[cfg(test)]
fn load_with_reader(
    read: impl FnMut(&str) -> io::Result<Option<String>>,
) -> io::Result<ConfigurationDocument> {
    load_with_sources(read).map(|loaded| loaded.document)
}

fn load_with_sources(
    mut read: impl FnMut(&str) -> io::Result<Option<String>>,
) -> io::Result<LoadedConfiguration> {
    if let Some(contents) = read("config.ini")? {
        return ConfigurationDocument::from_text(&contents)
            .map(|document| LoadedConfiguration {
                document,
                sources: ConfigurationSources::UNIFIED,
            })
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }
    let settings_text = read("settings.ini")?;
    let dictionary = read("user_dictionary.txt")?;
    let words = read("word_exclusions.txt")?;
    let processes = read("exclusions.txt")?;
    let sources = ConfigurationSources(
        (u8::from(settings_text.is_some()) << 1)
            | (u8::from(dictionary.is_some()) << 2)
            | (u8::from(words.is_some()) << 3)
            | (u8::from(processes.is_some()) << 4),
    );
    let settings = settings_text
        .map(|text| Settings::try_from_text(&text))
        .transpose()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .unwrap_or_default();
    ConfigurationDocument::try_from_legacy(
        settings,
        dictionary.as_deref().unwrap_or_default(),
        words.as_deref().unwrap_or_default(),
        processes.as_deref().unwrap_or_default(),
    )
    .map(|document| LoadedConfiguration { document, sources })
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_transition_backs_up_current_schema_exactly_without_changing_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        let mut original = ConfigurationDocument::default();
        original
            .process_exclusions
            .push("protected-test.exe".into());
        original.user_dictionary.push("ru-RU: тест".into());
        let source = format!(
            "# preserved comment\r\n{}",
            original.to_text().unwrap().replace('\n', "\r\n")
        );
        std::fs::write(&path, &source).unwrap();
        let mut managed = original.clone();
        managed.package_mode = crate::configuration::PackageMode::Managed;
        let temporary = prepare_configuration_write(&path, &managed.to_text().unwrap()).unwrap();
        let backup = path.with_extension("packages-legacy.bak");
        assert_eq!(std::fs::read(&backup).unwrap(), source.as_bytes());
        assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
        let mut saved =
            ConfigurationDocument::from_text(&std::fs::read_to_string(&temporary).unwrap())
                .unwrap();
        assert_eq!(
            saved.package_mode,
            crate::configuration::PackageMode::Managed
        );
        saved.package_mode = crate::configuration::PackageMode::LegacyBootstrap;
        assert_eq!(saved, original);
        std::fs::remove_file(&temporary).unwrap();
        // An exact existing backup permits an explicit retry, not overwrite.
        let temporary = prepare_configuration_write(&path, &managed.to_text().unwrap()).unwrap();
        std::fs::remove_file(temporary).unwrap();
        original.process_exclusions.push("second-test.exe".into());
        std::fs::write(&path, original.to_text().unwrap()).unwrap();
        assert_eq!(
            prepare_configuration_write(&path, &managed.to_text().unwrap())
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(!path.with_extension("tmp").exists());
        assert_eq!(std::fs::read(&backup).unwrap(), source.as_bytes());
    }

    #[test]
    fn schema_three_upgrade_retains_exact_backup_and_explicit_legacy_mode() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        let original = "# preserve CRLF\r\nschema_version=3\r\n[settings]\r\nenabled_input_packs=en-US,ru-RU\r\n[input_profiles]\r\nen-US=0409:00020409\r\n[process_exclusions]\r\nprivate.exe\r\n";
        std::fs::write(&path, original).unwrap();
        let document = load_configuration_directory(directory.path()).unwrap();
        assert_eq!(
            document.package_mode,
            crate::configuration::PackageMode::LegacyBootstrap
        );
        let saved = document.to_text().unwrap();
        let temporary = prepare_configuration_write(&path, &saved).unwrap();
        assert_eq!(
            std::fs::read(path.with_extension("schema-3.bak")).unwrap(),
            original.as_bytes()
        );
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert_eq!(
            ConfigurationDocument::from_text(&std::fs::read_to_string(&temporary).unwrap())
                .unwrap(),
            document
        );
        std::fs::remove_file(&temporary).unwrap();
        std::fs::write(&path, original.replace("private.exe", "changed.exe")).unwrap();
        assert_eq!(
            prepare_configuration_write(&path, &saved)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(!temporary.exists());
        assert_eq!(
            std::fs::read(path.with_extension("schema-3.bak")).unwrap(),
            original.as_bytes()
        );
    }

    #[test]
    fn save_preparation_stops_on_backup_failure_and_never_overwrites_a_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        let temporary = path.with_extension("tmp");
        let backup = path.with_extension("schema-1.bak");
        let original = "schema_version=1\n[process_exclusions]\nprivate.exe\n";
        let replacement = "schema_version=2\n[process_exclusions]\nprivate-new.exe\n";
        std::fs::write(&path, original).unwrap();
        std::fs::write(&backup, "different backup").unwrap();
        assert!(prepare_configuration_write(&path, replacement).is_err());
        assert!(!temporary.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::write(&backup, original).unwrap();
        std::fs::write(&temporary, "unrelated file").unwrap();
        assert!(prepare_configuration_write(&path, replacement).is_err());
        assert_eq!(
            std::fs::read_to_string(&temporary).unwrap(),
            "unrelated file"
        );
        std::fs::remove_file(&temporary).unwrap();
        #[cfg(unix)]
        {
            let unrelated = directory.path().join("unrelated.txt");
            std::fs::write(&unrelated, "do not overwrite").unwrap();
            std::os::unix::fs::symlink(&unrelated, &temporary).unwrap();
            assert!(prepare_configuration_write(&path, replacement).is_err());
            assert_eq!(
                std::fs::read_to_string(&unrelated).unwrap(),
                "do not overwrite"
            );
            std::fs::remove_file(&temporary).unwrap();
        }
        assert_eq!(
            prepare_configuration_write(&path, replacement).unwrap(),
            temporary
        );
        assert_eq!(std::fs::read_to_string(&temporary).unwrap(), replacement);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
    }

    #[test]
    fn schema_two_upgrade_has_its_own_exact_backup_and_collision_gate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        let backup = path.with_extension("schema-2.bak");
        let original = "schema_version=2\r\n[settings]\r\nenabled_input_packs=custom-missing\r\n[process_exclusions]\r\nprivate.exe\r\n";
        std::fs::write(&path, original).unwrap();
        let loaded = load_configuration_directory(directory.path()).unwrap();
        assert!(!backup.exists());
        let saved = loaded.to_text().unwrap();
        let temporary = prepare_configuration_write(&path, &saved).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&temporary).unwrap(), saved);
        std::fs::remove_file(temporary).unwrap();
        let changed = original.replace("private.exe", "private-new.exe");
        std::fs::write(&path, &changed).unwrap();
        assert_eq!(
            prepare_configuration_write(&path, &saved)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), changed);
        assert!(!path.with_extension("tmp").exists());
        std::fs::write(&path, &saved).unwrap();
        backup_before_schema_upgrade(&path).unwrap();
        assert!(!path.with_extension("schema-3.bak").exists());
    }

    #[test]
    fn schema_upgrade_backup_is_explicit_exact_and_never_overwrites_an_old_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        let backup = path.with_extension("schema-1.bak");
        let original = "# retain original spelling\r\nschema_version=1\r\n[settings]\r\nenable_russian=false\r\n[process_exclusions]\r\nprivate.exe\r\n";
        std::fs::write(&path, original).unwrap();
        let loaded = load_configuration_directory(directory.path()).unwrap();
        assert!(!backup.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        backup_before_schema_upgrade(&path).unwrap();
        backup_before_schema_upgrade(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(loaded.to_text().unwrap().contains("schema_version=4"));
        let newer = original.replace("private.exe", "private-new.exe");
        std::fs::write(&path, &newer).unwrap();
        assert_eq!(
            backup_before_schema_upgrade(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    }

    #[test]
    fn upgrade_backup_skips_new_files_and_rejects_invalid_sources_or_link_destinations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        let backup = path.with_extension("schema-1.bak");
        backup_before_schema_upgrade(&path).unwrap();
        assert!(!path.exists());
        std::fs::write(&path, "schema_version=2\n[settings]\nschema_version=1\n").unwrap();
        backup_before_schema_upgrade(&path).unwrap();
        assert!(!backup.exists());
        std::fs::write(&path, "schema_version=999\n").unwrap();
        assert!(backup_before_schema_upgrade(&path).is_err());
        assert!(!backup.exists());
        #[cfg(unix)]
        {
            let original = "schema_version=1\n[process_exclusions]\nprivate.exe\n";
            std::fs::write(&path, original).unwrap();
            let target = directory.path().join("unrelated.txt");
            std::fs::write(&target, "unrelated").unwrap();
            std::os::unix::fs::symlink(&target, &backup).unwrap();
            assert!(backup_before_schema_upgrade(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "unrelated");
        }
    }
    use std::fs::File;

    #[test]
    fn reload_source_policy_preserves_every_legacy_component_and_allows_unified_migration() {
        for previous in 0u8..16 {
            let previous = ConfigurationSources(previous << 1);
            assert!(previous.accepts_reload(ConfigurationSources::UNIFIED));
            for next in 0u8..16 {
                let next = ConfigurationSources(next << 1);
                assert_eq!(
                    previous.accepts_reload(next),
                    (previous.0 & next.0) == previous.0
                );
                assert!(!ConfigurationSources::UNIFIED.accepts_reload(next));
            }
        }
        assert!(ConfigurationSources::UNIFIED.accepts_reload(ConfigurationSources::UNIFIED));
    }

    #[test]
    fn sources_record_successful_reads_including_empty_existing_files() {
        assert_eq!(
            load_with_sources(|_| Ok(None)).unwrap().sources,
            ConfigurationSources::default()
        );
        for (index, selected) in [
            "settings.ini",
            "user_dictionary.txt",
            "word_exclusions.txt",
            "exclusions.txt",
        ]
        .into_iter()
        .enumerate()
        {
            let loaded =
                load_with_sources(|name| Ok((name == selected).then(String::new))).unwrap();
            assert_eq!(loaded.sources, ConfigurationSources(1 << (index + 1)));
        }
        let loaded = load_with_sources(|name| {
            assert_eq!(name, "config.ini");
            Ok(Some("schema_version=1\n".to_owned()))
        })
        .unwrap();
        assert_eq!(loaded.sources, ConfigurationSources::UNIFIED);
    }

    #[test]
    fn removed_exclusion_and_unified_files_do_not_authorize_a_default_reload() {
        let directory = tempfile::tempdir().unwrap();
        let exclusions = directory.path().join("exclusions.txt");
        std::fs::write(&exclusions, "private.exe").unwrap();
        let original = load_configuration_snapshot(directory.path()).unwrap();
        std::fs::remove_file(exclusions).unwrap();
        let missing = load_configuration_snapshot(directory.path()).unwrap();
        assert!(!original.sources.accepts_reload(missing.sources));
        let unified = directory.path().join("config.ini");
        std::fs::write(&unified, original.document.to_text().unwrap()).unwrap();
        let migrated = load_configuration_snapshot(directory.path()).unwrap();
        assert!(original.sources.accepts_reload(migrated.sources));
        assert_eq!(migrated.document.process_exclusions, ["private.exe"]);
        std::fs::remove_file(unified).unwrap();
        assert!(
            !migrated.sources.accepts_reload(
                load_configuration_snapshot(directory.path())
                    .unwrap()
                    .sources
            )
        );
    }

    #[test]
    fn readable_malformed_legacy_values_and_rows_are_not_silently_discarded() {
        for (failed, contents) in [
            ("settings.ini", "pause_break_undo=bad"),
            ("user_dictionary.txt", "../unknown-language: word"),
            ("word_exclusions.txt", "en: two words"),
            ("exclusions.txt", "\"\""),
        ] {
            let result = load_with_reader(|name| Ok((name == failed).then(|| contents.to_owned())));
            assert_eq!(
                result.unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{failed}"
            );
        }
    }

    #[test]
    fn checked_legacy_migration_preserves_valid_existing_data() {
        let settings_text =
            "pause_break_undo=no\nforce_hotkey_key=F12\nforce_hotkey_modifiers=Ctrl";
        let dictionary = "# comment\nen: firefox\nru: привет";
        let words = "en: example";
        let processes = "# comment\nPrivate.exe\nC:\\Tools\\Secret.exe";
        let expected = ConfigurationDocument::from_legacy(
            Settings::from_text(settings_text),
            dictionary,
            words,
            processes,
        );
        let loaded = load_with_reader(|name| {
            Ok(match name {
                "settings.ini" => Some(settings_text.to_owned()),
                "user_dictionary.txt" => Some(dictionary.to_owned()),
                "word_exclusions.txt" => Some(words.to_owned()),
                "exclusions.txt" => Some(processes.to_owned()),
                _ => None,
            })
        })
        .unwrap();
        assert_eq!(loaded, expected);
    }

    #[test]
    fn disk_load_is_read_only_and_preserves_invalid_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        std::fs::write(&path, "schema_version=999\n").unwrap();
        assert!(load_configuration_directory(directory.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "schema_version=999\n"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn disk_load_rejects_oversized_invalid_utf8_and_non_file_sources() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.ini");
        std::fs::write(&path, [0xff]).unwrap();
        assert_eq!(
            load_configuration_directory(directory.path())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        File::create(&path)
            .unwrap()
            .set_len(CONFIGURATION_MAX_BYTES + 1)
            .unwrap();
        assert_eq!(
            load_configuration_directory(directory.path())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(load_configuration_directory(directory.path()).is_err());
    }

    #[test]
    fn disk_load_does_not_create_files_on_first_launch() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            load_configuration_directory(directory.path()).unwrap(),
            ConfigurationDocument::default()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_configuration_link_is_not_treated_as_first_launch() {
        let directory = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("missing"),
            directory.path().join("config.ini"),
        )
        .unwrap();
        assert!(load_configuration_directory(directory.path()).is_err());
    }

    #[test]
    fn absent_configuration_is_a_clean_first_launch() {
        assert_eq!(
            load_with_reader(|_| Ok(None)).unwrap(),
            ConfigurationDocument::default()
        );
    }

    #[test]
    fn unified_errors_never_fall_back_to_legacy() {
        for contents in [
            "",
            "schema_version=999\n[process_exclusions]\nprivate.exe\n",
            "schema_version=1\n[unknown]\n",
        ] {
            let result = load_with_reader(|name| {
                assert_eq!(name, "config.ini");
                Ok(Some(contents.to_owned()))
            });
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn unreadable_unified_and_each_legacy_file_fail_closed() {
        for failed in [
            "config.ini",
            "settings.ini",
            "user_dictionary.txt",
            "word_exclusions.txt",
            "exclusions.txt",
        ] {
            for kind in [
                io::ErrorKind::PermissionDenied,
                io::ErrorKind::InvalidData,
                io::ErrorKind::Interrupted,
            ] {
                let result = load_with_reader(|name| {
                    if name == failed {
                        Err(io::Error::new(kind, "test failure"))
                    } else {
                        Ok(None)
                    }
                });
                assert_eq!(result.unwrap_err().kind(), kind, "{failed}");
            }
        }
    }

    #[test]
    fn unified_document_preserves_every_value_and_ignores_legacy() {
        let document = ConfigurationDocument::from_legacy(
            Settings::default(),
            "en:hello",
            "en:world",
            "private.exe",
        );
        let text = document.to_text().unwrap();
        let loaded = load_with_reader(|name| {
            assert_eq!(name, "config.ini");
            Ok(Some(text.clone()))
        })
        .unwrap();
        assert_eq!(loaded, document);
    }

    #[test]
    fn legacy_exclusions_are_preserved() {
        let loaded = load_with_reader(|name| {
            Ok(match name {
                "exclusions.txt" => Some("private.exe\nsecret.exe\n".to_owned()),
                _ => None,
            })
        })
        .unwrap();
        assert_eq!(loaded.process_exclusions, ["private.exe", "secret.exe"]);
    }
}
