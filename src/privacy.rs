//! Privacy policy shared by platform adapters and settings code.

use std::collections::BTreeSet;

/// Reasons why the current input context must not be buffered or converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyBlockReason {
    PasswordField,
    ExcludedProcess,
    ElevatedProcess,
    InspectionUnavailable,
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

    pub fn is_excluded(&self, path_or_name: &str) -> bool {
        normalize_executable_name(path_or_name)
            .is_some_and(|name| self.executable_names.contains(&name))
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
