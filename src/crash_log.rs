//! A crash log for a GUI process that has no console.
//!
//! Every panic appends one line with the time, the thread name, the source location and the
//! application version to `crash.log`. The panic message and payload are never recorded: they can
//! contain text the user typed. The log is capped and rotated, and always on, independent of the
//! optional diagnostics log.

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Size at which `crash.log` is moved to `crash.1.log`.
pub const MAX_LOG_BYTES: u64 = 256 * 1024;

/// One log line. `location` is the file, line and column of the panic.
pub fn format_record(
    unix_ms: u128,
    thread: &str,
    location: Option<(&str, u32, u32)>,
    version: &str,
) -> String {
    let thread = thread.replace(char::is_whitespace, "_");
    let at = location.map_or_else(
        || "unknown".to_owned(),
        |(file, line, column)| format!("{file}:{line}:{column}"),
    );
    format!("{unix_ms} panic thread={thread} at {at} version={version}\r\n")
}

/// Appends `record` to `<directory>/crash.log`, creating the directory and rotating a full log
/// first. Failures are ignored: a crash log must never be able to panic or block.
pub fn append_record(directory: &Path, record: &str) {
    let _ = try_append(directory, record);
}

fn try_append(directory: &Path, record: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let log = directory.join("crash.log");
    if std::fs::metadata(&log).is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES) {
        // Replacing the previous rotation may fail; the new record is written regardless.
        let _ = std::fs::rename(&log, directory.join("crash.1.log"));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(log)?;
    file.write_all(record.as_bytes())
}

/// Replaces the panic hook with one that writes a line to `<directory>/crash.log`. The default
/// hook would print the message to a console that a GUI process does not have.
pub fn install(directory: PathBuf, version: &'static str) {
    std::panic::set_hook(Box::new(move |info| {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let current = std::thread::current();
        let location = info
            .location()
            .map(|location| (location.file(), location.line(), location.column()));
        append_record(
            &directory,
            &format_record(
                unix_ms,
                current.name().unwrap_or("unnamed"),
                location,
                version,
            ),
        );
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_has_the_documented_shape() {
        assert_eq!(
            format_record(
                1_700_000_000_123,
                "input worker",
                Some(("src/x.rs", 10, 5)),
                "0.1.0 (abc1234)"
            ),
            "1700000000123 panic thread=input_worker at src/x.rs:10:5 version=0.1.0 (abc1234)\r\n"
        );
        assert_eq!(
            format_record(1, "main", None, "v"),
            "1 panic thread=main at unknown version=v\r\n"
        );
    }

    #[test]
    fn appending_creates_the_directory_and_keeps_earlier_records() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("nested").join("profile");
        append_record(&directory, "first\r\n");
        append_record(&directory, "second\r\n");
        assert_eq!(
            std::fs::read_to_string(directory.join("crash.log")).unwrap(),
            "first\r\nsecond\r\n"
        );
        assert!(!directory.join("crash.1.log").exists());
    }

    #[test]
    fn a_full_log_is_rotated_and_the_previous_rotation_is_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("crash.log");
        let rotated = directory.path().join("crash.1.log");
        std::fs::write(&rotated, "older rotation").unwrap();
        std::fs::write(&log, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        append_record(directory.path(), "fresh\r\n");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "fresh\r\n");
        assert_eq!(std::fs::metadata(&rotated).unwrap().len(), MAX_LOG_BYTES);
        // One byte under the limit still appends.
        std::fs::write(&log, vec![b'y'; MAX_LOG_BYTES as usize - 1]).unwrap();
        append_record(directory.path(), "z");
        assert_eq!(std::fs::metadata(&log).unwrap().len(), MAX_LOG_BYTES);
    }

    #[test]
    fn an_unwritable_directory_is_ignored_instead_of_panicking() {
        let directory = tempfile::tempdir().unwrap();
        let blocker = directory.path().join("file");
        std::fs::write(&blocker, "not a directory").unwrap();
        append_record(&blocker.join("child"), "record\r\n");
    }
}
