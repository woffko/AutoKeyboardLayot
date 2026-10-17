//! Privacy policy shared by platform adapters and settings code.

use std::collections::BTreeSet;

/// Reasons why the current input context must not be buffered or converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyBlockReason {
    PasswordField,
    ExcludedProcess,
    ElevatedProcess,
    /// The focused field cannot expose its password property. Process identity,
    /// integrity and input-context checks are independent requirements.
    FieldInspectionUnavailable,
    InspectionUnavailable,
}

/// The opt-in exception is only for a known terminal's field-inspection failure.
/// Transport, process and context failures remain denied even for manual input.
pub fn manual_conversion_allowed(
    reason: Option<PrivacyBlockReason>,
    terminal_opt_in: bool,
    verified_terminal: bool,
) -> bool {
    reason.is_none()
        || (reason == Some(PrivacyBlockReason::FieldInspectionUnavailable)
            && terminal_opt_in
            && verified_terminal)
}

/// Case-insensitive executable-name exclusion policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExclusionPolicy {
    executable_names: BTreeSet<String>,
}

impl Default for ExclusionPolicy {
    fn default() -> Self {
        Self::from_lines([
            "1Password.exe",
            "AutoKeyboardLayot.exe",
            "Bitwarden.exe",
            "Consent.exe",
            "CredentialUIBroker.exe",
            "KeePass.exe",
            "KeePassXC.exe",
            "LogonUI.exe",
        ])
    }
}

impl ExclusionPolicy {
    pub fn from_lines<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut policy = Self {
            executable_names: BTreeSet::new(),
        };
        policy.extend_lines(lines);
        policy
    }

    pub fn extend_lines<I, S>(&mut self, lines: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for line in lines {
            let line = line.as_ref().trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = normalize_executable_name(line) {
                self.executable_names.insert(name);
            }
        }
    }

    /// An entry without an extension ("Signal", "KeePass") names the program by its file stem:
    /// a Windows image name always has an extension, so the entry alone would never match.
    pub fn is_excluded(&self, path_or_name: &str) -> bool {
        normalize_executable_name(path_or_name).is_some_and(|name| {
            self.executable_names.contains(&name)
                || name.rsplit_once('.').is_some_and(|(stem, _)| {
                    !stem.is_empty() && self.executable_names.contains(stem)
                })
        })
    }

    pub fn len(&self) -> usize {
        self.executable_names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.executable_names.is_empty()
    }

    pub fn normalize_entry(path_or_name: &str) -> Option<String> {
        normalize_executable_name(path_or_name)
    }
}

fn normalize_executable_name(path_or_name: &str) -> Option<String> {
    let trimmed = path_or_name.trim().trim_matches('"');
    let name = trimmed.rsplit(['/', '\\']).next()?.trim();
    (!name.is_empty()).then(|| name.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_exception_never_authorizes_transport_or_process_failures() {
        for opt_in in [false, true] {
            for terminal in [false, true] {
                assert!(manual_conversion_allowed(None, opt_in, terminal));
                assert_eq!(
                    manual_conversion_allowed(
                        Some(PrivacyBlockReason::FieldInspectionUnavailable),
                        opt_in,
                        terminal
                    ),
                    opt_in && terminal
                );
                for reason in [
                    PrivacyBlockReason::PasswordField,
                    PrivacyBlockReason::ExcludedProcess,
                    PrivacyBlockReason::ElevatedProcess,
                    PrivacyBlockReason::InspectionUnavailable,
                ] {
                    assert!(!manual_conversion_allowed(Some(reason), opt_in, terminal));
                }
            }
        }
    }

    #[test]
    fn an_entry_without_an_extension_matches_the_program_by_its_stem() {
        let policy = ExclusionPolicy::from_lines(["Signal", "keepass.exe", "my.app"]);
        assert!(policy.is_excluded(r"C:\Users\u\AppData\Local\Programs\signal-desktop\Signal.exe"));
        assert!(policy.is_excluded("KeePass.exe"));
        assert!(policy.is_excluded("my.app.exe"));
        assert!(!policy.is_excluded("SignalR.exe"));
        assert!(!policy.is_excluded("keepass"));
        assert!(!policy.is_excluded(".exe"));
    }

    #[test]
    fn default_policy_matches_names_and_full_paths_case_insensitively() {
        let policy = ExclusionPolicy::default();
        assert!(policy.is_excluded("KeePassXC.exe"));
        assert!(policy.is_excluded("C:\\Tools\\BITWARDEN.EXE"));
        assert!(policy.is_excluded("C:\\Tools\\AutoKeyboardLayot.exe"));
        assert!(!policy.is_excluded("notepad.exe"));
    }

    #[test]
    fn user_lines_ignore_comments_and_accept_windows_or_unix_paths() {
        let policy = ExclusionPolicy::from_lines([
            "# private editor",
            " C:\\Apps\\PrivateEditor.exe ",
            "/opt/tools/secret-terminal",
            "",
        ]);
        assert_eq!(policy.len(), 2);
        assert!(policy.is_excluded("privateeditor.exe"));
        assert!(policy.is_excluded("SECRET-TERMINAL"));
    }
}
