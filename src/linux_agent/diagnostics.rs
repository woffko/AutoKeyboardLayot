//! The optional diagnostics log of the Linux engine. One line per event, written by a thread of
//! its own so that a slow disk never delays a key. Like the Windows agent's log it never holds
//! typed text or key codes (tests/diagnostics_privacy.rs checks the format strings).

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

/// The log rotates to `diagnostics.1.log` at this size, as on Windows.
const MAX_BYTES: u64 = 1024 * 1024;
const QUEUE: usize = 1024;

pub struct Diagnostics {
    sender: Option<SyncSender<String>>,
}

impl Diagnostics {
    pub const fn disabled() -> Self {
        Self { sender: None }
    }

    /// A log at `path`, appended by a background thread.
    pub fn to_file(path: PathBuf) -> Self {
        let (sender, receiver) = sync_channel::<String>(QUEUE);
        let spawned = thread::Builder::new()
            .name("autokey-diagnostics".to_owned())
            .spawn(move || write_records(&path, &receiver));
        Self {
            sender: spawned.ok().map(|_| sender),
        }
    }

    /// A log whose records the test reads back.
    #[cfg(test)]
    pub fn channel() -> (Self, Receiver<String>) {
        let (sender, receiver) = sync_channel(QUEUE);
        (
            Self {
                sender: Some(sender),
            },
            receiver,
        )
    }

    #[cfg(test)]
    pub const fn enabled(&self) -> bool {
        self.sender.is_some()
    }

    /// Queues `event=<event> <details>`; a full queue drops the record rather than wait.
    pub fn record(&self, event: &str, details: &str) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(format!("event={event} {details}"));
        }
    }
}

fn write_records(path: &Path, receiver: &Receiver<String>) {
    while let Ok(record) = receiver.recv() {
        let _ = append(path, &record);
    }
}

fn append(path: &Path, record: &str) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let line = format!("{timestamp} {}\n", record.replace(['\r', '\n'], " "));
    let size = fs::metadata(path).map_or(0, |metadata| metadata.len());
    if size.saturating_add(line.len() as u64) > MAX_BYTES {
        fs::rename(path, path.with_extension("1.log"))?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_appended_with_a_timestamp_and_rotate_at_the_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state").join("diagnostics.log");
        append(&path, "event=focus result=in\nsecond").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let (timestamp, rest) = text.split_once(' ').unwrap();
        assert!(timestamp.parse::<u128>().is_ok());
        assert_eq!(rest, "event=focus result=in second\n");
        fs::write(&path, vec![b'x'; MAX_BYTES as usize]).unwrap();
        append(&path, "event=layout result=switched").unwrap();
        assert_eq!(
            fs::metadata(path.with_extension("1.log")).unwrap().len(),
            MAX_BYTES
        );
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .ends_with("event=layout result=switched\n")
        );
    }

    #[test]
    fn a_disabled_log_records_nothing() {
        let diagnostics = Diagnostics::disabled();
        assert!(!diagnostics.enabled());
        diagnostics.record("focus", "result=in");
        let (diagnostics, receiver) = Diagnostics::channel();
        diagnostics.record("focus", "result=in");
        assert_eq!(receiver.try_recv().unwrap(), "event=focus result=in");
    }
}
