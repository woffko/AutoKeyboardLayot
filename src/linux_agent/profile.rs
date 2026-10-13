//! The user's profile on Linux: the lock that serializes the writers of `config.ini`, and the
//! English-only profile the modular base creates the first time it starts for a user. On Windows
//! the installer creates that profile; on Linux no installer runs per user, so the engine and the
//! settings window create it.

use super::paths;
use autokeyboardlayot::{
    configuration::prepare_configuration_write, installer_profile::initialize_profile_if_absent,
};
use std::{fs::File, io, os::fd::AsRawFd, path::Path};

/// An exclusive advisory lock on a file, released when dropped.
pub struct FileLock {
    _file: File,
}

impl FileLock {
    /// Locks `path`, creating the file and its directory when missing. Without `wait`, a lock held
    /// by another process gives `None` at once.
    pub fn acquire(path: &Path, wait: bool) -> io::Result<Option<Self>> {
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

/// Waits for the lock every writer of `config.ini` in `config_dir` holds.
pub fn config_lock(config_dir: &Path) -> io::Result<FileLock> {
    FileLock::acquire(&config_dir.join("config.lock"), true)?
        .ok_or_else(|| io::Error::other("the configuration lock was not granted"))
}

fn publish(path: &Path, contents: &str) -> io::Result<()> {
    let temporary = prepare_configuration_write(path, contents)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

/// Creates the English-only profile of the modular base when the user has none: the profile in
/// `config_dir` and an empty package store in `data_dir/packages`; true when it created one. The
/// shared rules apply: an existing profile, even a legacy or malformed one, is never rewritten, and
/// a partial package store is an error, not a reset. A build with the bundled languages runs on the
/// built-in defaults without a profile, so it creates none.
pub fn initialize_if_absent(config_dir: &Path, data_dir: &Path) -> io::Result<bool> {
    if cfg!(feature = "legacy-bundled-input") {
        return Ok(false);
    }
    let _lock = config_lock(config_dir)?;
    initialize_profile_if_absent(config_dir, &data_dir.join("packages"), publish)
}

/// [`initialize_if_absent`] in the user's XDG directories.
pub fn initialize_user_profile() -> io::Result<bool> {
    let unavailable = || io::Error::new(io::ErrorKind::NotFound, "HOME is not set");
    let config = paths::config_dir().ok_or_else(unavailable)?;
    let data = paths::data_dir().ok_or_else(unavailable)?;
    initialize_if_absent(&config, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "legacy-bundled-input"))]
    #[test]
    fn the_first_start_creates_the_english_only_profile_the_engine_runs_on() {
        use autokeyboardlayot::{
            ConfigurationDocument, Language, configuration::load_configuration_snapshot,
        };
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let data = root.path().join("data");
        assert!(initialize_if_absent(&config, &data).unwrap());
        assert!(data.join("packages/CURRENT").is_file());
        assert_eq!(
            load_configuration_snapshot(&config).unwrap().document,
            ConfigurationDocument::english_only_managed()
        );
        let shared =
            super::super::runtime::load_shared_from(&config, &data, &root.path().join("state"))
                .unwrap();
        assert_eq!(shared.layouts(), [Language::English]);
        // The next start finds the profile and leaves it alone.
        let bytes = std::fs::read(config.join("config.ini")).unwrap();
        assert!(!initialize_if_absent(&config, &data).unwrap());
        assert_eq!(std::fs::read(config.join("config.ini")).unwrap(), bytes);
    }

    #[cfg(not(feature = "legacy-bundled-input"))]
    #[test]
    fn an_existing_or_partial_profile_is_never_rewritten() {
        use autokeyboardlayot::ConfigurationDocument;
        // A profile of a build with the bundled languages selects Russian and Estonian.
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        let legacy = ConfigurationDocument::default().to_text().unwrap();
        std::fs::write(config.join("config.ini"), &legacy).unwrap();
        let error = initialize_if_absent(&config, &root.path().join("data")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            std::fs::read_to_string(config.join("config.ini")).unwrap(),
            legacy
        );
        assert!(!root.path().join("data/packages").exists());
        // A malformed file stays as it is.
        std::fs::write(config.join("config.ini"), "malformed").unwrap();
        assert!(initialize_if_absent(&config, &root.path().join("data")).is_err());
        assert_eq!(
            std::fs::read_to_string(config.join("config.ini")).unwrap(),
            "malformed"
        );
        // A package directory that is not a store is not reset, and no profile is written.
        let partial = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(partial.path().join("data/packages")).unwrap();
        assert!(
            initialize_if_absent(&partial.path().join("config"), &partial.path().join("data"))
                .is_err()
        );
        assert!(!partial.path().join("config/config.ini").exists());
    }

    #[cfg(feature = "legacy-bundled-input")]
    #[test]
    fn a_build_with_the_bundled_languages_creates_no_profile() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let data = root.path().join("data");
        assert!(!initialize_if_absent(&config, &data).unwrap());
        assert!(!config.exists());
        assert!(!data.exists());
    }

    #[test]
    fn a_held_lock_is_refused_without_waiting() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("instance.lock");
        let first = FileLock::acquire(&path, false).unwrap();
        assert!(first.is_some());
        assert!(FileLock::acquire(&path, false).unwrap().is_none());
        drop(first);
        assert!(FileLock::acquire(&path, false).unwrap().is_some());
    }
}
