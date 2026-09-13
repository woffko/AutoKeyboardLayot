//! Per-user, cross-session startup/install coordination through Windows sharing.
//! Running clients retain read-only handles; setup opens the same file with no
//! sharing after graceful closure. The empty file is never truncated/deleted.
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::Path,
};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        FILE_TYPE_DISK, GetFileType,
    },
};

pub fn shared_for_current_user() -> io::Result<Option<File>> {
    let root = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA unavailable"))?;
    let root = Path::new(&root);
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Absolute profile path required",
        ));
    }
    shared_at(&root.join("AutoKeyboardLayot.installation.lock"))
}

fn validate(file: File) -> io::Result<File> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() != 0
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid installation coordination file",
        ));
    }
    Ok(file)
}

fn shared_at(path: &Path) -> io::Result<Option<File>> {
    let open = || {
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
    };
    let result = match open() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .share_mode(0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(path)
            {
                Ok(file) => {
                    validate(file)?;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => return Ok(None),
                Err(error) => return Err(error),
            }
            open()
        }
        result => result,
    };
    match result {
        Ok(file) => validate(file).map(Some),
        Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clients_coexist_but_exclusive_install_and_startup_exclude_each_other() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("installation.lock");
        let first = shared_at(&path).unwrap().unwrap();
        let second = shared_at(&path).unwrap().unwrap();
        let exclusive = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
        };
        assert!(exclusive().is_err());
        drop(first);
        assert!(exclusive().is_err());
        drop(second);
        let installer = exclusive().unwrap();
        assert!(shared_at(&path).unwrap().is_none());
        drop(installer);
        assert!(shared_at(&path).unwrap().is_some());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    }
    #[test]
    fn unexpected_existing_contents_are_never_reset() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("installation.lock");
        std::fs::write(&path, "unexpected existing data").unwrap();
        assert!(shared_at(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "unexpected existing data"
        );
    }
}
