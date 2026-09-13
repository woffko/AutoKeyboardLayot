//! Versioned, human-readable configuration shared by the Windows agent and GUI.

use std::{collections::BTreeSet, fmt};

use crate::localization::UiLanguagePreference;
use crate::{BackendRuleError, BackendRules, ExclusionPolicy, Hotkey, Settings, UserLexicon};

mod storage;
pub use storage::{
    CONFIGURATION_MAX_BYTES, ConfigurationSources, LoadedConfiguration,
    backup_before_schema_upgrade, load_configuration_directory, load_configuration_snapshot,
    prepare_configuration_write,
};

pub const CONFIGURATION_SCHEMA_VERSION: u32 = 4;

/// Persisted installation mode, never inferred from a directory's disappearance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PackageMode {
    #[default]
    LegacyBootstrap,
    Managed,
}
impl PackageMode {
    pub const fn as_config(self) -> &'static str {
        match self {
            Self::LegacyBootstrap => "legacy",
            Self::Managed => "managed",
        }
    }
}

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
    pub package_mode: PackageMode,
    pub settings: Settings,
    pub input_profiles: crate::input_profile_selection::InputProfileSelections,
    pub ui_language: UiLanguagePreference,
    pub user_dictionary: Vec<String>,
    pub word_exclusions: Vec<String>,
    pub process_exclusions: Vec<String>,
    pub backend_rules: BackendRules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Packages,
    Settings,
    InputProfiles,
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
        let mut settings_lines = Vec::new();
        let mut ui_language = UiLanguagePreference::default();
        let mut user_dictionary = Vec::new();
        let mut word_exclusions = Vec::new();
        let mut process_exclusions = Vec::new();
        let mut backend_rules = Vec::new();
        let mut backend_section_seen = false;
        let mut profile_section_seen = false;
        let mut input_profiles = Vec::new();
        let mut package_section_seen = false;
        let mut package_mode = None;

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
                    "packages" => {
                        if package_section_seen {
                            return Err(ConfigurationError::at(
                                line_number,
                                "duplicate packages section",
                            ));
                        }
                        package_section_seen = true;
                        Section::Packages
                    }
                    "settings" => Section::Settings,
                    "input_profiles" => {
                        if profile_section_seen {
                            return Err(ConfigurationError::at(
                                line_number,
                                "duplicate input_profiles section",
                            ));
                        }
                        profile_section_seen = true;
                        Section::InputProfiles
                    }
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
                Section::Packages => {
                    let Some((key, value)) = line.split_once('=') else {
                        return Err(ConfigurationError::at(
                            line_number,
                            "expected mode=legacy or mode=managed",
                        ));
                    };
                    if key.trim() != "mode" || package_mode.is_some() {
                        return Err(ConfigurationError::at(
                            line_number,
                            "only one package mode is allowed",
                        ));
                    }
                    package_mode = Some(match value.trim() {
                        "legacy" => PackageMode::LegacyBootstrap,
                        "managed" => PackageMode::Managed,
                        _ => {
                            return Err(ConfigurationError::at(
                                line_number,
                                "invalid package mode",
                            ));
                        }
                    });
                }
                Section::InputProfiles => {
                    if input_profiles.len() >= 64 {
                        return Err(ConfigurationError::at(
                            line_number,
                            "too many input profile selections",
                        ));
                    }
                    let Some((id, profile)) = line.split_once('=') else {
                        return Err(ConfigurationError::at(
                            line_number,
                            "expected pack-id=LANGID:KLID",
                        ));
                    };
                    input_profiles.push((id.trim(), profile.trim()));
                }
                Section::Settings => {
                    if let Some((key, value)) = line.split_once('=')
                        && key.trim().eq_ignore_ascii_case("ui_language")
                    {
                        ui_language = UiLanguagePreference::from_config(value);
                    }
                    settings.push_str(line);
                    settings.push('\n');
                    settings_lines.push(line_number);
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
        if !(1..=CONFIGURATION_SCHEMA_VERSION).contains(&schema_version) {
            return Err(ConfigurationError::global(format!(
                "unsupported schema_version {schema_version}; expected {CONFIGURATION_SCHEMA_VERSION}"
            )));
        }
        if profile_section_seen && schema_version < 3 {
            return Err(ConfigurationError::global(
                "input_profiles requires schema_version 3",
            ));
        }
        if package_section_seen && schema_version < 4 {
            return Err(ConfigurationError::global(
                "packages requires schema_version 4",
            ));
        }
        if schema_version >= 4 && package_mode.is_none() {
            return Err(ConfigurationError::global(
                "schema_version 4 requires an explicit package mode",
            ));
        }

        let mut document = Self {
            package_mode: package_mode.unwrap_or_default(),
            input_profiles: crate::input_profile_selection::InputProfileSelections::parse(
                input_profiles,
            )
            .map_err(|error| {
                ConfigurationError::global(format!("invalid input profile selections: {error:?}"))
            })?,
            settings: Settings::try_from_text(&settings).map_err(|error| {
                ConfigurationError::at(settings_lines[error.line - 1], error.message)
            })?,
            ui_language,
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
            package_mode: PackageMode::LegacyBootstrap,
            settings,
            input_profiles: Default::default(),
            ui_language: UiLanguagePreference::default(),
            user_dictionary: canonical_lexicon_entries_lossy(user_dictionary),
            word_exclusions: canonical_lexicon_entries_lossy(word_exclusions),
            process_exclusions: canonical_process_entries_lossy(process_exclusions),
            backend_rules: BackendRules::default(),
        };
        let _ = document.canonicalize_and_validate();
        document
    }

    /// Migration input must not silently lose malformed dictionary/exclusion rows.
    pub fn try_from_legacy(
        settings: Settings,
        user_dictionary: &str,
        word_exclusions: &str,
        process_exclusions: &str,
    ) -> Result<Self, ConfigurationError> {
        fn rows(text: &str) -> Vec<(usize, String)> {
            text.lines()
                .enumerate()
                .filter_map(|(index, line)| {
                    let line = line.trim();
                    (!line.is_empty() && !line.starts_with('#'))
                        .then(|| (index + 1, line.to_owned()))
                })
                .collect()
        }
        let mut document = Self {
            package_mode: PackageMode::LegacyBootstrap,
            settings,
            input_profiles: Default::default(),
            ui_language: UiLanguagePreference::default(),
            user_dictionary: canonical_lexicon_entries(rows(user_dictionary))?,
            word_exclusions: canonical_lexicon_entries(rows(word_exclusions))?,
            process_exclusions: canonical_process_entries(rows(process_exclusions))?,
            backend_rules: BackendRules::default(),
        };
        document.canonicalize_and_validate()?;
        Ok(document)
    }

    pub fn to_text(&self) -> Result<String, ConfigurationError> {
        let mut document = self.clone();
        document.canonicalize_and_validate()?;
        let mut output = format!(
            "# AutoKeyboardLayot configuration\nschema_version={}\n\n[packages]\nmode={}\n\n[settings]\nui_language={}\n{}\n",
            CONFIGURATION_SCHEMA_VERSION,
            document.package_mode.as_config(),
            document.ui_language.as_config(),
            document.settings.to_text()
        );
        append_section(&mut output, "user_dictionary", &document.user_dictionary);
        output.push_str("[input_profiles]\n");
        for (id, profile) in document.input_profiles.iter() {
            output.push_str(&format!("{id}={profile}\n"));
        }
        output.push('\n');
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

    /// Fresh installer document only; never use this to replace a user's config.
    /// The caller must initialize/verify the empty managed store before saving it.
    pub fn english_only_managed() -> Self {
        let mut document = Self {
            package_mode: PackageMode::Managed,
            ..Self::default()
        };
        document.settings.enabled_input_packs = [crate::Language::English].into();
        document
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

        if self.settings.enabled_input_packs.len() > 64 {
            return Err(ConfigurationError::global("too many input pack selections"));
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
        let Some((id, word)) = UserLexicon::normalize_pack_entry(&line) else {
            return Err(ConfigurationError::at(
                line_number,
                "expected a valid language and one word, for example en-US: firefox",
            ));
        };
        entries.insert(
            UserLexicon::format_pack_entry(&id, &word)
                .expect("normalized word")
                .trim_end()
                .to_owned(),
        );
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
        .filter_map(UserLexicon::normalize_pack_entry)
        .map(|(id, word)| {
            UserLexicon::format_pack_entry(&id, &word)
                .expect("normalized word")
                .trim_end()
                .to_owned()
        })
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
    #[test]
    fn package_mode_roundtrips_and_fresh_managed_base_does_not_enable_conversion() {
        use super::{ConfigurationDocument, PackageMode};
        let document = ConfigurationDocument::english_only_managed();
        let serialized = document.to_text().unwrap();
        assert_eq!(
            ConfigurationDocument::from_text(&serialized).unwrap(),
            document
        );
        assert!(serialized.contains("[packages]\nmode=managed\n"));
        assert_eq!(
            document.settings.enabled_input_packs,
            [crate::Language::English].into()
        );
        assert!(document.settings.automatic_conversion_on_startup);
        for version in 1..=3 {
            let old = ConfigurationDocument::from_text(&format!("schema_version={version}\n[settings]\nautomatic_conversion_on_startup=true\n[process_exclusions]\nprivate.exe\n[word_exclusions]\nru: привет\n")).unwrap();
            assert_eq!(old.package_mode, PackageMode::LegacyBootstrap);
            assert_eq!(
                ConfigurationDocument::from_text(&old.to_text().unwrap()).unwrap(),
                old
            );
            assert!(old.settings.automatic_conversion_on_startup);
            assert_eq!(old.process_exclusions, ["private.exe"]);
            assert!(
                old.word_exclusion_lexicon()
                    .contains(crate::Language::Russian, "привет")
            );
        }
    }

    #[test]
    fn package_mode_is_mandatory_in_v4_and_rejects_unknown_duplicate_or_old_schema_fields() {
        use super::ConfigurationDocument;
        for body in [
            "",
            "[packages]\n",
            "[packages]\nmode=auto\n",
            "[packages]\nmode=Managed\n",
            "[packages]\nunknown=managed\n",
            "[packages]\nmode=managed\nmode=legacy\n",
            "[packages]\nmode=managed\n[packages]\n",
            "[packages]\nmanaged\n",
            "[packages]\nmode=managed=legacy\n",
        ] {
            assert!(
                ConfigurationDocument::from_text(&format!("schema_version=4\n{body}")).is_err(),
                "{body}"
            );
        }
        for version in 1..=3 {
            assert!(
                ConfigurationDocument::from_text(&format!(
                    "schema_version={version}\n[packages]\nmode=managed\n"
                ))
                .is_err()
            );
        }
        assert!(
            ConfigurationDocument::from_text("schema_version=5\n[packages]\nmode=managed\n")
                .is_err()
        );
    }
    #[test]
    fn profile_choices_roundtrip_without_enabling_missing_or_disabled_packs() {
        let text = "schema_version=3\n[settings]\nenabled_input_packs=ru-RU\n[input_profiles]\nEN-us=0409:00020409\ncustom-missing=0419:00000419\n[process_exclusions]\nprivate.exe\n[word_exclusions]\ncustom-missing: hello\n";
        let document = super::ConfigurationDocument::from_text(text).unwrap();
        let saved = document.to_text().unwrap();
        assert_eq!(
            super::ConfigurationDocument::from_text(&saved).unwrap(),
            document
        );
        assert_eq!(document.input_profiles.iter().count(), 2);
        assert_eq!(document.settings.enabled_input_packs.len(), 1);
        assert_eq!(document.process_exclusions, ["private.exe"]);
        assert!(saved.contains("en-us=0409:00020409"));
        for version in [1, 2] {
            assert!(
                super::ConfigurationDocument::from_text(
                    &text.replace("schema_version=3", &format!("schema_version={version}"))
                )
                .is_err()
            );
            let old = super::ConfigurationDocument::from_text(&format!(
                "schema_version={version}\n[process_exclusions]\nprivate.exe\n"
            ))
            .unwrap();
            assert_eq!(old.input_profiles.iter().count(), 0);
            assert_eq!(old.process_exclusions, ["private.exe"]);
        }
    }

    #[test]
    fn profile_section_rejects_duplicates_malformed_rows_and_overflow() {
        for rows in [
            "en-US=0409:00000409\nEN-us=0409:00020409",
            "en-US=0409:409",
            "../en=0409:00000409",
            "en-US:0409:00000409",
            "[input_profiles]",
        ] {
            assert!(
                super::ConfigurationDocument::from_text(&format!(
                    "schema_version=3\n[input_profiles]\n{rows}\n"
                ))
                .is_err()
            );
        }
        let rows: String = (0..64)
            .map(|i| format!("custom-{i}=0409:00000409\n"))
            .collect();
        let text = format!("schema_version=3\n[input_profiles]\n{rows}");
        assert!(super::ConfigurationDocument::from_text(&text).is_ok());
        assert!(
            super::ConfigurationDocument::from_text(&format!("{text}custom-64=0409:00000409\n"))
                .is_err()
        );
    }
    use super::*;

    #[test]
    fn schema_migration_preserves_missing_pack_overlays_and_operational_values() {
        for schema in [1, 2] {
            let text = format!(
                "schema_version={schema}\n[settings]\nautomatic_conversion_on_startup=true\nforce_hotkey_key=F12\nforce_hotkey_modifiers=Ctrl\nenable_russian=false\nui_language=et\n[user_dictionary]\nde-DE: Hallo\nru-RU: привет\n[word_exclusions]\nzh-Hans: 你好\n[process_exclusions]\nprivate.exe\n"
            );
            let original = ConfigurationDocument::from_text(&text).unwrap();
            let serialized = original.to_text().unwrap();
            assert!(serialized.contains("schema_version=4\n"));
            assert!(!serialized.contains("enable_russian="));
            let migrated = ConfigurationDocument::from_text(&serialized).unwrap();
            assert_eq!(migrated, original);
            assert!(
                migrated
                    .user_lexicon()
                    .contains_pack(&crate::PackId::parse("de-DE").unwrap(), "hallo")
            );
            assert!(
                migrated
                    .word_exclusion_lexicon()
                    .contains_pack(&crate::PackId::parse("zh-Hans").unwrap(), "你好")
            );
            assert!(
                migrated
                    .user_lexicon()
                    .contains(crate::Language::Russian, "привет")
            );
            assert!(!migrated.settings.language_enabled(crate::Language::Russian));
            assert_eq!(migrated.process_exclusions, ["private.exe"]);
        }
    }

    #[test]
    fn schema_two_accepts_zero_one_and_missing_selections_without_dropping_exclusions() {
        for selection in ["", "en-US", "de-DE", "en-US,de-DE"] {
            let text = format!(
                "schema_version=2\n[settings]\nenabled_input_packs={selection}\n[process_exclusions]\nprivate.exe\n[word_exclusions]\nde-DE: hallo\n"
            );
            let original = ConfigurationDocument::from_text(&text).unwrap();
            assert_eq!(
                ConfigurationDocument::from_text(&original.to_text().unwrap()).unwrap(),
                original
            );
            assert_eq!(original.process_exclusions, ["private.exe"]);
        }
    }
    use crate::BackendStrategy;

    #[test]
    fn malformed_known_settings_report_the_original_document_line() {
        let error = ConfigurationDocument::from_text("schema_version=1\n\n[settings]\n# note\npause_break_undo=bad\n[process_exclusions]\nprivate.exe\n").unwrap_err();
        assert_eq!(error.line, Some(5));
        assert_eq!(error.message, "invalid boolean setting");
    }
    #[test]
    fn configuration_round_trips_all_managed_data() {
        let settings = Settings {
            suppress_after_manual_layout_change: false,
            ..Settings::default()
        };
        let document = ConfigurationDocument {
            settings,
            ui_language: UiLanguagePreference::from_config("et-EE"),
            user_dictionary: vec!["en-US: Firefox".to_owned()],
            word_exclusions: vec!["ru-RU: руддщ".to_owned()],
            process_exclusions: vec!["C:\\Private\\Secret.EXE".to_owned()],
            ..ConfigurationDocument::default()
        };
        let text = document.to_text().unwrap();
        let parsed = ConfigurationDocument::from_text(&text).unwrap();
        assert_eq!(parsed.ui_language.as_config(), "et-ee");
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
        assert!(document.ui_language.is_system());
    }

    #[test]
    fn optional_ui_language_preserves_v1_operational_settings_and_lexicons() {
        let base = "schema_version=1\n[settings]\nautomatic_conversion_on_startup=true\nforce_hotkey_key=F9\nforce_hotkey_modifiers=CTRL\n[process_exclusions]\nprivate-editor.exe\n[user_dictionary]\nen-US: customword\n[word_exclusions]\nru-RU: руддщ\n";
        let original = ConfigurationDocument::from_text(base).unwrap();
        assert!(original.ui_language.is_system());
        let with_locale = base.replace("[settings]\n", "[settings]\nui_language=ja-JP\n");
        let parsed = ConfigurationDocument::from_text(&with_locale).unwrap();
        assert_eq!(parsed.ui_language.as_config(), "ja-jp");
        assert_eq!(parsed.settings, original.settings);
        assert_eq!(parsed.user_dictionary, original.user_dictionary);
        assert_eq!(parsed.word_exclusions, original.word_exclusions);
        assert_eq!(parsed.process_exclusions, original.process_exclusions);
        assert_eq!(parsed.backend_rules, original.backend_rules);
        // The previous Settings parser ignores the optional non-input key.
        assert_eq!(
            Settings::from_text("ui_language=ja-JP\nautomatic_conversion_on_startup=true\n"),
            Settings::from_text("automatic_conversion_on_startup=true\n")
        );
        let serialized = parsed.to_text().unwrap();
        assert!(serialized.contains("schema_version=4\n"));
        assert_eq!(
            ConfigurationDocument::from_text(&serialized).unwrap(),
            parsed
        );
    }

    #[test]
    fn invalid_optional_ui_locale_does_not_discard_process_exclusions() {
        let document = ConfigurationDocument::from_text("schema_version=1\n[settings]\nui_language=../../bad\n[process_exclusions]\nprivate-editor.exe\n").unwrap();
        assert_eq!(document.ui_language.as_config(), "en");
        assert!(
            document
                .process_exclusion_policy()
                .is_excluded("private-editor.exe")
        );
    }

    #[test]
    fn missing_valid_ui_locale_is_preserved_for_later_pack_installation() {
        let document =
            ConfigurationDocument::from_text("schema_version=1\n[settings]\nui_language=de-DE\n")
                .unwrap();
        assert_eq!(document.ui_language.as_config(), "de-de");
        assert_eq!(
            ConfigurationDocument::from_text(&document.to_text().unwrap())
                .unwrap()
                .ui_language,
            document.ui_language
        );
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
        one_language.settings.enabled_input_packs = [crate::PackId::parse("en-US").unwrap()]
            .into_iter()
            .collect();
        assert!(one_language.to_text().is_ok());
    }

    #[test]
    fn rejects_unknown_or_future_schema() {
        assert!(ConfigurationDocument::from_text("[settings]\n").is_err());
        assert!(ConfigurationDocument::from_text("schema_version=4\n[settings]\n").is_err());
        assert!(ConfigurationDocument::from_text("schema_version=1\n[unknown]\n").is_err());
    }
}
