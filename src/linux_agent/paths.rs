//! Where the Linux engine keeps its files, following the XDG base directories: the configuration
//! in `~/.config/autokeyboardlayot`, language packages in `~/.local/share/autokeyboardlayot` and
//! the diagnostics and crash logs in `~/.local/state/autokeyboardlayot`.

use std::path::PathBuf;

const APPLICATION: &str = "autokeyboardlayot";

fn base(variable: &str, fallback: &str) -> Option<PathBuf> {
    base_in(&|name| std::env::var_os(name), variable, fallback)
}

/// The directory for `variable` (an absolute XDG path) or `$HOME/fallback`, with the application
/// name appended; `lookup` reads the environment.
fn base_in(
    lookup: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    variable: &str,
    fallback: &str,
) -> Option<PathBuf> {
    lookup(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            lookup("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(fallback))
        })
        .map(|path| path.join(APPLICATION))
}

/// `config.ini` and the files that belong to it.
pub fn config_dir() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config")
}

/// The package store (`packages/`).
pub fn data_dir() -> Option<PathBuf> {
    base("XDG_DATA_HOME", ".local/share")
}

/// `diagnostics.log` and `crash.log`.
pub fn state_dir() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn xdg_variables_win_and_relative_values_are_ignored() {
        let environment = |name: &str| -> Option<OsString> {
            match name {
                "HOME" => Some("/home/someone".into()),
                "XDG_CONFIG_HOME" => Some("/tmp/config".into()),
                "XDG_STATE_HOME" => Some("relative/state".into()),
                _ => None,
            }
        };
        assert_eq!(
            base_in(&environment, "XDG_CONFIG_HOME", ".config"),
            Some(PathBuf::from("/tmp/config/autokeyboardlayot"))
        );
        assert_eq!(
            base_in(&environment, "XDG_STATE_HOME", ".local/state"),
            Some(PathBuf::from(
                "/home/someone/.local/state/autokeyboardlayot"
            ))
        );
        assert_eq!(
            base_in(&environment, "XDG_DATA_HOME", ".local/share"),
            Some(PathBuf::from(
                "/home/someone/.local/share/autokeyboardlayot"
            ))
        );
        assert_eq!(base_in(&|_| None, "XDG_DATA_HOME", ".local/share"), None);
    }
}
