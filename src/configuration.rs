//! Versioned, human-readable configuration shared by the Windows agent and GUI.

use std::{collections::BTreeSet, fmt};

use crate::{BackendRuleError, BackendRules, ExclusionPolicy, Hotkey, Settings, UserLexicon};

pub const CONFIGURATION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationError {
    pub line: Option<usize>,
    pub message: String,
}

impl ConfigurationError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    fn global(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            write!(formatter, "line {line}: {}", self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for ConfigurationError {}

impl From<BackendRuleError> for ConfigurationError {
    fn from(error: BackendRuleError) -> Self {
        Self {
            line: Some(error.line),
            message: error.message,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigurationDocument {
    pub settings: Settings,
    pub user_dictionary: Vec<String>,
    pub word_exclusions: Vec<String>,
    pub process_exclusions: Vec<String>,
    pub backend_rules: BackendRules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Settings,
    UserDictionary,
    WordExclusions,
    ProcessExclusions,
    BackendRules,
}

impl ConfigurationDocument {
    pub fn from_text(text: &str) -> Result<Self, ConfigurationError> {
        let mut schema_version = None;
        let mut section = None;
        let mut settings = String::new();
        let mut user_dictionary = Vec::new();
        let mut word_exclusions = Vec::new();
        let mut process_exclusions = Vec::new();
        let mut backend_rules = Vec::new();
        let mut backend_section_seen = false;

        for (index, raw_line) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                section = Some(match name.trim().to_ascii_lowercase().as_str() {
                    "settings" => Section::Settings,
                    "user_dictionary" => Section::UserDictionary,
                    "word_exclusions" => Section::WordExclusions,
                    "process_exclusions" => Section::ProcessExclusions,
                    "backend_rules" => {
                        backend_section_seen = true;
                        Section::BackendRules
                    }
                    _ => return Err(ConfigurationError::at(line_number, "unknown section")),
                });
                continue;
            }
            if section.is_none() {
                let Some((key, value)) = line.split_once('=') else {
                    return Err(ConfigurationError::at(
                        line_number,
                        "expected schema_version before the first section",
                    ));
                };
                if key.trim() != "schema_version" || schema_version.is_some() {
                    return Err(ConfigurationError::at(
                        line_number,
                        "only one schema_version is allowed before sections",
                    ));
                }
                schema_version = value.trim().parse::<u32>().ok();
                if schema_version.is_none() {
                    return Err(ConfigurationError::at(
                        line_number,
                        "invalid schema version",
                    ));
                }
                continue;
            }
            match section.expect("section checked above") {
                Section::Settings => {
                    settings.push_str(line);
                    settings.push('\n');
                }
                Section::UserDictionary => user_dictionary.push((line_number, line.to_owned())),
                Section::WordExclusions => word_exclusions.push((line_number, line.to_owned())),
                Section::ProcessExclusions => {
                    process_exclusions.push((line_number, line.to_owned()));
                }
                Section::BackendRules => backend_rules.push(line.to_owned()),
            }
        }

        let Some(schema_version) = schema_version else {
            return Err(ConfigurationError::global("schema_version is missing"));
        };
        if schema_version != CONFIGURATION_SCHEMA_VERSION {
            return Err(ConfigurationError::global(format!(
                "unsupported schema_version {schema_version}; expected {CONFIGURATION_SCHEMA_VERSION}"
            )));
        }

        let mut document = Self {
            settings: Settings::from_text(&settings),
            user_dictionary: canonical_lexicon_entries(user_dictionary)?,
            word_exclusions: canonical_lexicon_entries(word_exclusions)?,
            process_exclusions: canonical_process_entries(process_exclusions)?,
            backend_rules: if backend_section_seen {
                BackendRules::from_lines(backend_rules.iter().map(String::as_str))?
            } else {
                BackendRules::default()
            },
        };
        document.canonicalize_and_validate()?;
        Ok(document)
    }

    pub fn from_legacy(
        settings: Settings,
        user_dictionary: &str,
        word_exclusions: &str,
        process_exclusions: &str,
    ) -> Self {
        let mut document = Self {
            settings,
            user_dictionary: canonical_lexicon_entries_lossy(user_dictionary),
            word_exclusions: canonical_lexicon_entries_lossy(word_exclusions),
            process_exclusions: canonical_process_entries_lossy(process_exclusions),
            backend_rules: BackendRules::default(),
        };
        let _ = document.canonicalize_and_validate();
        document
    }

    pub fn to_text(&self) -> Result<String, ConfigurationError> {
        let mut document = self.clone();
        document.canonicalize_and_validate()?;
        let mut output = format!(
            "# AutoKeyboardLayot configuration\nschema_version={}\n\n[settings]\n{}\n",
            CONFIGURATION_SCHEMA_VERSION,
            document.settings.to_text()
        );
        append_section(&mut output, "user_dictionary", &document.user_dictionary);
        append_section(&mut output, "word_exclusions", &document.word_exclusions);
        append_section(
            &mut output,
            "process_exclusions",
            &document.process_exclusions,
        );
        output.push_str("[backend_rules]\n");
        output.push_str(&document.backend_rules.to_text());
        Ok(output)
    }

    pub fn user_lexicon(&self) -> UserLexicon {
        UserLexicon::from_lines(self.user_dictionary.iter().map(String::as_str))
    }

    pub fn word_exclusion_lexicon(&self) -> UserLexicon {
        UserLexicon::from_lines(self.word_exclusions.iter().map(String::as_str))
    }

    pub fn process_exclusion_policy(&self) -> ExclusionPolicy {
        let mut policy = ExclusionPolicy::default();
        policy.extend_lines(&self.process_exclusions);
        policy
    }

    pub fn canonicalize_and_validate(&mut self) -> Result<(), ConfigurationError> {
        self.user_dictionary = canonical_lexicon_entries(
            self.user_dictionary
                .iter()
                .enumerate()
                .map(|(index, line)| (index + 1, line.clone()))
                .collect(),
        )?;
        self.word_exclusions = canonical_lexicon_entries(
            self.word_exclusions
                .iter()
                .enumerate()
                .map(|(index, line)| (index + 1, line.clone()))
                .collect(),
        )?;
        self.process_exclusions = canonical_process_entries(
            self.process_exclusions
                .iter()
                .enumerate()
                .map(|(index, line)| (index + 1, line.clone()))
                .collect(),
        )?;

        let enabled_languages = [
            self.settings.enable_english,
            self.settings.enable_russian,
            self.settings.enable_estonian,
        ]
        .into_iter()
        .filter(|enabled| *enabled)
        .count();
        if enabled_languages < 2 {
            return Err(ConfigurationError::global(
                "at least two automatic languages must remain enabled",
            ));
        }
        Hotkey::new(
            self.settings.force_hotkey_virtual_key,
            self.settings.force_hotkey_modifiers,
        )
        .map_err(|error| ConfigurationError::global(format!("invalid force hotkey: {error}")))?;

        let user_words: BTreeSet<_> = self.user_dictionary.iter().cloned().collect();
        if let Some(collision) = self
            .word_exclusions
            .iter()
            .find(|entry| user_words.contains(*entry))
        {
            return Err(ConfigurationError::global(format!(
                "the same entry cannot be both a user word and an exclusion: {collision}"
            )));
        }
        Ok(())
    }
}

fn canonical_lexicon_entries(
    lines: Vec<(usize, String)>,
) -> Result<Vec<String>, ConfigurationError> {
    let mut entries = BTreeSet::new();
    for (line_number, line) in lines {
        let Some((language, word)) = UserLexicon::normalize_entry(&line) else {
            return Err(ConfigurationError::at(
                line_number,
                "expected a valid language and one word, for example en-US: firefox",
            ));
        };
        entries.insert(format!("{}: {word}", language.id()));
    }
    Ok(entries.into_iter().collect())
}

fn canonical_process_entries(
    lines: Vec<(usize, String)>,
) -> Result<Vec<String>, ConfigurationError> {
    let mut entries = BTreeSet::new();
    for (line_number, line) in lines {
        let Some(name) = ExclusionPolicy::normalize_entry(&line) else {
            return Err(ConfigurationError::at(
                line_number,
                "expected one executable name or path",
            ));
        };
        entries.insert(name);
    }
    Ok(entries.into_iter().collect())
}

fn canonical_lexicon_entries_lossy(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .filter_map(UserLexicon::normalize_entry)
        .map(|(language, word)| format!("{}: {word}", language.id()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn canonical_process_entries_lossy(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .filter_map(ExclusionPolicy::normalize_entry)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn append_section(output: &mut String, name: &str, entries: &[String]) {
    output.push('[');
    output.push_str(name);
    output.push_str("]\n");
    for entry in entries {
        output.push_str(entry);
        output.push('\n');
    }
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendStrategy;

    #[test]
    fn configuration_round_trips_all_managed_data() {
        let settings = Settings {
            suppress_after_manual_layout_change: false,
            ..Settings::default()
        };
        let document = ConfigurationDocument {
            settings,
            user_dictionary: vec!["en-US: Firefox".to_owned()],
            word_exclusions: vec!["ru-RU: руддщ".to_owned()],
            process_exclusions: vec!["C:\\Private\\Secret.EXE".to_owned()],
            ..ConfigurationDocument::default()
        };
        let text = document.to_text().unwrap();
        let parsed = ConfigurationDocument::from_text(&text).unwrap();
        assert!(!parsed.settings.suppress_after_manual_layout_change);
        assert_eq!(parsed.user_dictionary, ["en-US: firefox"]);
        assert_eq!(parsed.word_exclusions, ["ru-RU: руддщ"]);
        assert_eq!(parsed.process_exclusions, ["secret.exe"]);
        assert_eq!(
            parsed.backend_rules.resolve("firefox.exe"),
            BackendStrategy::PhysicalReplay
        );
    }

    #[test]
    fn legacy_files_migrate_without_persisting_comments() {
        let document = ConfigurationDocument::from_legacy(
            Settings::default(),
            "# user\nen-US: Firefox\n",
            "ru-RU: руддщ\n",
            "# private\nC:\\Apps\\Vault.exe\n",
        );
        assert_eq!(document.user_dictionary, ["en-US: firefox"]);
        assert_eq!(document.word_exclusions, ["ru-RU: руддщ"]);
        assert_eq!(document.process_exclusions, ["vault.exe"]);
    }

    #[test]
    fn validation_rejects_collisions_invalid_rows_and_one_language() {
        let collision = ConfigurationDocument {
            user_dictionary: vec!["en-US: test".to_owned()],
            word_exclusions: vec!["en-US: TEST".to_owned()],
            ..ConfigurationDocument::default()
        };
        assert!(collision.to_text().is_err());

        let invalid = "schema_version=1\n[user_dictionary]\nen-US: two words\n";
        assert!(ConfigurationDocument::from_text(invalid).is_err());

        let mut one_language = ConfigurationDocument::default();
        one_language.settings.enable_russian = false;
        one_language.settings.enable_estonian = false;
        assert!(one_language.to_text().is_err());
    }

    #[test]
    fn rejects_unknown_or_future_schema() {
        assert!(ConfigurationDocument::from_text("[settings]\n").is_err());
        assert!(ConfigurationDocument::from_text("schema_version=2\n[settings]\n").is_err());
        assert!(ConfigurationDocument::from_text("schema_version=1\n[unknown]\n").is_err());
    }
}
