//! Versioned, human-readable configuration shared by the Windows agent and GUI.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

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
    /// The file the error is in when it is not `config.ini`: a file of an older version.
    pub file: Option<&'static str>,
    pub line: Option<usize>,
    pub message: String,
}

impl ConfigurationError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            file: None,
            line: Some(line),
            message: message.into(),
        }
    }

    fn global(message: impl Into<String>) -> Self {
        Self {
            file: None,
            line: None,
            message: message.into(),
        }
    }

    /// The same error, placed in `file`.
    fn in_file(self, file: &'static str) -> Self {
        Self {
            file: Some(file),
            ..self
        }
    }
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.file, self.line) {
            (Some(file), Some(line)) => write!(formatter, "{file}, line {line}: {}", self.message),
            (Some(file), None) => write!(formatter, "{file}: {}", self.message),
            (None, Some(line)) => write!(formatter, "line {line}: {}", self.message),
            (None, None) => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for ConfigurationError {}

impl From<BackendRuleError> for ConfigurationError {
    fn from(error: BackendRuleError) -> Self {
        Self::at(error.line, error.message)
    }
}

/// What `ConfigurationDocument::merge` made of a draft and the configuration as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedConfiguration {
    /// The configuration as it is now with the draft's changes; in a conflict, the draft's value.
    pub document: ConfigurationDocument,
    /// What both the draft and the configuration changed since the draft's base, differently:
    /// setting keys, `ui_language`, `mode`, a profile's pack ID, a rule's executable, or
    /// `user_dictionary, word_exclusions` when an entry ends up in both lists.
    pub conflicts: Vec<String>,
}

/// One entry of a list as an editor holds it: its stored form and its 1-based line in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorEntry {
    pub line: usize,
    pub entry: String,
}

fn editor_entries(
    text: &str,
    entry: impl Fn(usize, &str) -> Result<String, ConfigurationError>,
) -> Result<Vec<EditorEntry>, ConfigurationError> {
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let stored = entry(index + 1, line)?;
        if seen.insert(stored.clone()) {
            entries.push(EditorEntry {
                line: index + 1,
                entry: stored,
            });
        }
    }
    Ok(entries)
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

/// An editor may save a file with a byte order mark; one at the start is not part of the text.
fn without_byte_order_mark(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

impl ConfigurationDocument {
    pub fn from_text(text: &str) -> Result<Self, ConfigurationError> {
        let text = without_byte_order_mark(text);
        let mut schema_version = None;
        let mut section = None;
        let mut settings = String::new();
        let mut settings_lines = Vec::new();
        let mut ui_language = UiLanguagePreference::default();
        let mut ui_language_seen = false;
        let mut user_dictionary = Vec::new();
        let mut word_exclusions = Vec::new();
        let mut process_exclusions = Vec::new();
        let mut backend_rules = Vec::new();
        let mut backend_lines = Vec::new();
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
                        if ui_language_seen {
                            return Err(ConfigurationError::at(line_number, "duplicate setting"));
                        }
                        ui_language_seen = true;
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
                Section::BackendRules => {
                    backend_rules.push(line.to_owned());
                    backend_lines.push(line_number);
                }
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
                // The rules see only their own rows: the error is moved to the document's line.
                BackendRules::from_lines(backend_rules.iter().map(String::as_str)).map_err(
                    |error| ConfigurationError::at(backend_lines[error.line - 1], error.message),
                )?
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
            user_dictionary: canonical_lexicon_entries_lossy(without_byte_order_mark(
                user_dictionary,
            )),
            word_exclusions: canonical_lexicon_entries_lossy(without_byte_order_mark(
                word_exclusions,
            )),
            process_exclusions: canonical_process_entries_lossy(without_byte_order_mark(
                process_exclusions,
            )),
            backend_rules: BackendRules::default(),
        };
        let _ = document.canonicalize_and_validate();
        document
    }

    /// Migration input must not silently lose malformed dictionary/exclusion rows. The texts are
    /// those of `user_dictionary.txt`, `word_exclusions.txt` and `exclusions.txt`; an error in a
    /// row names its file.
    pub fn try_from_legacy(
        settings: Settings,
        user_dictionary: &str,
        word_exclusions: &str,
        process_exclusions: &str,
    ) -> Result<Self, ConfigurationError> {
        fn rows(text: &str) -> Vec<(usize, String)> {
            without_byte_order_mark(text)
                .lines()
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
            user_dictionary: canonical_lexicon_entries(rows(user_dictionary))
                .map_err(|error| error.in_file("user_dictionary.txt"))?,
            word_exclusions: canonical_lexicon_entries(rows(word_exclusions))
                .map_err(|error| error.in_file("word_exclusions.txt"))?,
            process_exclusions: canonical_process_entries(rows(process_exclusions))
                .map_err(|error| error.in_file("exclusions.txt"))?,
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
        // What is written must read back the same: a line taken as a comment or a section would
        // drop an entry without a word.
        if Self::from_text(&output).ok().as_ref() != Some(&document) {
            return Err(ConfigurationError::global(
                "the configuration would not read back unchanged; it was not saved",
            ));
        }
        Ok(output)
    }

    /// The dictionary or word-exclusion entries an editor holds: one per line, blank lines and
    /// `#` comments skipped, each with its 1-based line in the editor, which an error names too.
    /// A repeated entry keeps its first line.
    pub fn lexicon_editor_entries(text: &str) -> Result<Vec<EditorEntry>, ConfigurationError> {
        editor_entries(text, lexicon_entry)
    }

    /// The program exclusions an editor holds, read like `lexicon_editor_entries`.
    pub fn process_editor_entries(text: &str) -> Result<Vec<EditorEntry>, ConfigurationError> {
        editor_entries(text, process_entry)
    }

    /// Merges a draft (`ours`, made from `base`) into the configuration as it is now (`theirs`):
    /// what the draft changed since `base` wins, everything else is taken from `theirs`. Settings,
    /// profiles and rules are merged one by one and the lists entry by entry, so a word the draft
    /// added and one added meanwhile both stay. A value both changed differently is a conflict;
    /// the draft's value is kept for it.
    pub fn merge(base: &Self, ours: &Self, theirs: &Self) -> MergedConfiguration {
        let mut conflicts = Vec::new();
        let package_mode = merge_value(
            "mode",
            &base.package_mode,
            &ours.package_mode,
            &theirs.package_mode,
            &mut conflicts,
        );
        let ui_language = merge_value(
            "ui_language",
            &base.ui_language,
            &ours.ui_language,
            &theirs.ui_language,
            &mut conflicts,
        );
        let settings_text: String = merge_map(
            &settings_values(&base.settings),
            &settings_values(&ours.settings),
            &settings_values(&theirs.settings),
            &mut conflicts,
        )
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect();
        let profiles = merge_map(
            &profile_values(&base.input_profiles),
            &profile_values(&ours.input_profiles),
            &profile_values(&theirs.input_profiles),
            &mut conflicts,
        );
        let rules: String = merge_map(
            &rule_values(&base.backend_rules),
            &rule_values(&ours.backend_rules),
            &rule_values(&theirs.backend_rules),
            &mut conflicts,
        )
        .iter()
        .map(|(pattern, strategy)| format!("{pattern} = {strategy}\n"))
        .collect();
        let document = Self {
            package_mode,
            // Every value comes from one of three valid documents.
            settings: Settings::try_from_text(&settings_text)
                .unwrap_or_else(|_| ours.settings.clone()),
            input_profiles: crate::input_profile_selection::InputProfileSelections::parse(
                profiles.iter(),
            )
            .unwrap_or_else(|_| ours.input_profiles.clone()),
            ui_language,
            user_dictionary: merge_entries(
                &base.user_dictionary,
                &ours.user_dictionary,
                &theirs.user_dictionary,
            ),
            word_exclusions: merge_entries(
                &base.word_exclusions,
                &ours.word_exclusions,
                &theirs.word_exclusions,
            ),
            process_exclusions: merge_entries(
                &base.process_exclusions,
                &ours.process_exclusions,
                &theirs.process_exclusions,
            ),
            backend_rules: BackendRules::from_lines(rules.lines())
                .unwrap_or_else(|_| ours.backend_rules.clone()),
        };
        if document
            .word_exclusions
            .iter()
            .any(|entry| document.user_dictionary.contains(entry))
        {
            conflicts.push("user_dictionary, word_exclusions".to_owned());
        }
        MergedConfiguration {
            document,
            conflicts,
        }
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

/// A three-way merge of one value: the draft's change wins, otherwise the configuration's.
fn merge_value<T: PartialEq + Clone>(
    name: &str,
    base: &T,
    ours: &T,
    theirs: &T,
    conflicts: &mut Vec<String>,
) -> T {
    if ours == base {
        theirs.clone()
    } else {
        if theirs != base && theirs != ours {
            conflicts.push(name.to_owned());
        }
        ours.clone()
    }
}

/// A three-way merge of named values, one by one; a missing name is a value too (removed).
fn merge_map(
    base: &BTreeMap<String, String>,
    ours: &BTreeMap<String, String>,
    theirs: &BTreeMap<String, String>,
    conflicts: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let names: BTreeSet<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut merged = BTreeMap::new();
    for name in names {
        if let Some(value) = merge_value(
            name,
            &base.get(name),
            &ours.get(name),
            &theirs.get(name),
            conflicts,
        ) {
            merged.insert(name.clone(), value.clone());
        }
    }
    merged
}

/// A three-way merge of a list, entry by entry: an entry both keep stays, one either side added
/// stays, one either side removed goes.
fn merge_entries(base: &[String], ours: &[String], theirs: &[String]) -> Vec<String> {
    let kept = |entry: &&String| !base.contains(entry);
    ours.iter()
        .filter(|entry| theirs.contains(entry) || kept(entry))
        .chain(theirs.iter().filter(kept))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn settings_values(settings: &Settings) -> BTreeMap<String, String> {
    settings
        .to_text()
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

fn profile_values(
    profiles: &crate::input_profile_selection::InputProfileSelections,
) -> BTreeMap<String, String> {
    profiles
        .iter()
        .map(|(id, profile)| (id.to_string(), profile.to_string()))
        .collect()
}

fn rule_values(rules: &BackendRules) -> BTreeMap<String, String> {
    rules
        .to_text()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(pattern, strategy)| (pattern.trim().to_owned(), strategy.trim().to_owned()))
        .collect()
}

fn canonical_lexicon_entries(
    lines: Vec<(usize, String)>,
) -> Result<Vec<String>, ConfigurationError> {
    let mut entries = BTreeSet::new();
    for (line_number, line) in lines {
        entries.insert(lexicon_entry(line_number, &line)?);
    }
    Ok(entries.into_iter().collect())
}

/// The stored form of one dictionary or word-exclusion row.
fn lexicon_entry(line_number: usize, line: &str) -> Result<String, ConfigurationError> {
    UserLexicon::normalize_pack_entry(line)
        .and_then(|(id, word)| UserLexicon::format_pack_entry(&id, &word))
        .map(|entry| entry.trim_end().to_owned())
        .ok_or_else(|| {
            ConfigurationError::at(
                line_number,
                "expected a language, a colon and one word, for example en-US: firefox",
            )
        })
}

fn canonical_process_entries(
    lines: Vec<(usize, String)>,
) -> Result<Vec<String>, ConfigurationError> {
    let mut entries = BTreeSet::new();
    for (line_number, line) in lines {
        entries.insert(process_entry(line_number, &line)?);
    }
    Ok(entries.into_iter().collect())
}

/// The stored form of one program-exclusion row.
fn process_entry(line_number: usize, line: &str) -> Result<String, ConfigurationError> {
    let Some(name) = ExclusionPolicy::normalize_entry(line) else {
        return Err(ConfigurationError::at(
            line_number,
            "expected one executable name or path",
        ));
    };
    if !process_name_reads_back(&name) {
        return Err(ConfigurationError::at(
            line_number,
            "a program name cannot start with # or be in square brackets",
        ));
    }
    Ok(name)
}

/// Whether a program name, written on a line of its own, reads back as that name: a line that
/// starts with `#` is a comment, and one in square brackets starts a section.
fn process_name_reads_back(name: &str) -> bool {
    !name.starts_with('#') && !(name.starts_with('[') && name.ends_with(']'))
}

fn canonical_lexicon_entries_lossy(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .filter_map(UserLexicon::normalize_pack_entry)
        .filter_map(|(id, word)| UserLexicon::format_pack_entry(&id, &word))
        .map(|entry| entry.trim_end().to_owned())
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
        .filter(|name| process_name_reads_back(name))
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
    fn a_word_that_lowercasing_lengthens_past_the_limit_is_a_line_error_not_a_panic() {
        let latin = "a".repeat(60);
        for word in ["İ".repeat(64), format!("{latin}{}", "İ".repeat(4))] {
            for section in ["user_dictionary", "word_exclusions"] {
                let text = format!(
                    "schema_version=4\n[packages]\nmode=legacy\n[{section}]\nen-US: {word}\n"
                );
                assert_eq!(
                    ConfigurationDocument::from_text(&text).unwrap_err().line,
                    Some(5)
                );
            }
            let row = format!("en-US: {word}\n");
            assert!(
                ConfigurationDocument::try_from_legacy(Settings::default(), &row, "", "").is_err()
            );
            let lossy = ConfigurationDocument::from_legacy(Settings::default(), &row, &row, "");
            assert!(lossy.user_dictionary.is_empty() && lossy.word_exclusions.is_empty());
        }
    }

    #[test]
    fn a_byte_order_mark_at_the_start_of_a_file_is_ignored() {
        let document = ConfigurationDocument {
            user_dictionary: vec!["en-US: firefox".to_owned()],
            process_exclusions: vec!["private.exe".to_owned()],
            ..ConfigurationDocument::default()
        };
        let text = document.to_text().unwrap();
        assert_eq!(
            ConfigurationDocument::from_text(&format!("\u{feff}{text}")).unwrap(),
            document
        );
        // Only one: a second one is text on the first line.
        assert_eq!(
            ConfigurationDocument::from_text(&format!("\u{feff}\u{feff}{text}"))
                .unwrap_err()
                .line,
            Some(1)
        );
        let legacy = ConfigurationDocument::try_from_legacy(
            Settings::default(),
            "\u{feff}en-US: firefox\n",
            "\u{feff}ru-RU: руддщ\n",
            "\u{feff}private.exe\n",
        )
        .unwrap();
        assert_eq!(legacy.user_dictionary, ["en-US: firefox"]);
        assert_eq!(legacy.word_exclusions, ["ru-RU: руддщ"]);
        assert_eq!(legacy.process_exclusions, ["private.exe"]);
        let lossy = ConfigurationDocument::from_legacy(
            Settings::default(),
            "\u{feff}en-US: firefox\n",
            "",
            "\u{feff}private.exe\n",
        );
        assert_eq!(lossy.user_dictionary, ["en-US: firefox"]);
        assert_eq!(lossy.process_exclusions, ["private.exe"]);
    }

    #[test]
    fn a_setting_given_twice_is_an_error_at_its_document_line() {
        let head = "schema_version=4\n[packages]\nmode=legacy\n[settings]\n";
        for (body, line) in [
            ("layout_model=true\n\nlayout_model=false\n", 7),
            (
                "layout_model=true\n[user_dictionary]\n[settings]\nlayout_model=false\n",
                8,
            ),
            ("ui_language=ru\nui_language=et\n", 6),
        ] {
            let error = ConfigurationDocument::from_text(&format!("{head}{body}")).unwrap_err();
            assert_eq!(error.line, Some(line), "{body}");
            assert_eq!(error.message, "duplicate setting");
        }
    }

    #[test]
    fn a_program_name_that_would_read_as_a_comment_or_a_section_is_refused() {
        for name in [
            "C:\\Tools\\#vault.exe",
            "#vault.exe",
            "[settings]",
            "D:\\[x]",
        ] {
            let document = ConfigurationDocument {
                process_exclusions: vec![name.to_owned()],
                ..ConfigurationDocument::default()
            };
            assert!(document.to_text().is_err(), "{name}");
            let text = format!(
                "schema_version=4\n[packages]\nmode=legacy\n[process_exclusions]\n\"{name}\"\n"
            );
            assert_eq!(
                ConfigurationDocument::from_text(&text).unwrap_err().line,
                Some(5),
                "{name}"
            );
            assert!(
                ConfigurationDocument::from_legacy(Settings::default(), "", "", name)
                    .process_exclusions
                    .is_empty()
            );
        }
        // Brackets or # inside a name read back as they are.
        for name in ["[x].exe", "a#b.exe"] {
            let document = ConfigurationDocument {
                process_exclusions: vec![name.to_owned()],
                ..ConfigurationDocument::default()
            };
            let text = document.to_text().unwrap();
            assert_eq!(
                ConfigurationDocument::from_text(&text)
                    .unwrap()
                    .process_exclusions,
                [name]
            );
        }
    }

    #[test]
    fn a_document_that_would_not_read_back_unchanged_is_not_written() {
        // A line break inside a name would read back as two names.
        let document = ConfigurationDocument {
            process_exclusions: vec!["evil\nname.exe".to_owned()],
            ..ConfigurationDocument::default()
        };
        assert_eq!(
            document.to_text().unwrap_err().message,
            "the configuration would not read back unchanged; it was not saved"
        );
    }

    #[test]
    fn a_bad_backend_rule_is_reported_at_its_document_line() {
        let text = "schema_version=4\n[packages]\nmode=legacy\n[backend_rules]\n# note\nnotepad.exe = protected-paste\n\nfirefox.exe = magic\n";
        let error = ConfigurationDocument::from_text(text).unwrap_err();
        assert_eq!(error.line, Some(8));
        assert!(error.message.starts_with("unknown backend"));
    }

    #[test]
    fn a_dictionary_row_without_the_colon_is_an_error_at_its_line() {
        for row in ["hello world", "firefox en-US", "ru-RU привет"] {
            let text = format!(
                "schema_version=4\n[packages]\nmode=legacy\n[user_dictionary]\nen-US: firefox\n{row}\n"
            );
            assert_eq!(
                ConfigurationDocument::from_text(&text).unwrap_err().line,
                Some(6),
                "{row}"
            );
        }
    }

    #[test]
    fn an_editor_list_names_the_editor_line_of_a_bad_entry() {
        let dictionary = "# my words\nen-US: Firefox\n\nen-US: two words\n";
        let error = ConfigurationDocument::lexicon_editor_entries(dictionary).unwrap_err();
        assert_eq!(error.line, Some(4));
        let entries = ConfigurationDocument::lexicon_editor_entries(
            "\nru-RU: Привет\nen-US: x\nru-RU: привет\n",
        )
        .unwrap();
        assert_eq!(
            entries,
            [
                EditorEntry {
                    line: 2,
                    entry: "ru-RU: привет".to_owned()
                },
                EditorEntry {
                    line: 3,
                    entry: "en-US: x".to_owned()
                },
            ]
        );
        let error = ConfigurationDocument::process_editor_entries(
            "private.exe\n\n#a comment\n\"#vault.exe\"\n",
        )
        .unwrap_err();
        assert_eq!(error.line, Some(4));
        assert_eq!(
            ConfigurationDocument::process_editor_entries("C:\\Apps\\Vault.exe\n").unwrap()[0]
                .entry,
            "vault.exe"
        );
    }

    impl ConfigurationDocument {
        /// The document as it is stored: lists sorted and normalized.
        fn canonical(mut self) -> Self {
            self.canonicalize_and_validate().unwrap();
            self
        }
    }

    #[test]
    fn a_draft_merges_into_a_configuration_changed_meanwhile() {
        // The owner's case: a word added from the tray while the window was open, a box
        // unticked and a word typed in the window.
        let base = ConfigurationDocument {
            user_dictionary: vec!["en-US: old".to_owned(), "en-US: gone".to_owned()],
            process_exclusions: vec!["private.exe".to_owned()],
            ..ConfigurationDocument::default()
        }
        .canonical();
        let mut theirs = base.clone();
        theirs.user_dictionary.push("en-US: firefox".to_owned());
        theirs
            .user_dictionary
            .retain(|entry| entry != "en-US: gone");
        theirs.settings.layout_model = !base.settings.layout_model;
        let mut ours = base.clone();
        ours.settings.suppress_after_manual_layout_change =
            !base.settings.suppress_after_manual_layout_change;
        ours.user_dictionary.push("ru-RU: привет".to_owned());
        ours.process_exclusions.clear();
        let merged = ConfigurationDocument::merge(&base, &ours.canonical(), &theirs.canonical());
        assert!(merged.conflicts.is_empty(), "{:?}", merged.conflicts);
        let document = merged.document;
        assert_eq!(
            document.settings.suppress_after_manual_layout_change,
            !base.settings.suppress_after_manual_layout_change
        );
        assert_eq!(document.settings.layout_model, !base.settings.layout_model);
        assert_eq!(
            document.user_dictionary,
            ["en-US: firefox", "en-US: old", "ru-RU: привет"]
        );
        assert!(document.process_exclusions.is_empty());
        assert!(document.to_text().is_ok());
    }

    #[test]
    fn a_value_both_sides_changed_differently_is_a_conflict_that_keeps_the_draft() {
        let base = ConfigurationDocument::default();
        let mut theirs = base.clone();
        theirs.ui_language = UiLanguagePreference::from_config("et");
        theirs.settings.layout_model = !base.settings.layout_model;
        theirs.backend_rules = BackendRules::from_lines(["notepad.exe = observe-only"]).unwrap();
        let mut ours = base.clone();
        ours.ui_language = UiLanguagePreference::from_config("ru");
        ours.settings.layout_model = !base.settings.layout_model;
        ours.backend_rules = BackendRules::from_lines(["notepad.exe = physical-replay"]).unwrap();
        let merged = ConfigurationDocument::merge(&base, &ours, &theirs);
        // The same new value on both sides agrees; the rule for notepad.exe and the interface
        // language do not. The catch-all rule, unchanged by the draft, is theirs.
        assert_eq!(merged.conflicts, ["ui_language", "notepad.exe"]);
        assert_eq!(merged.document.ui_language.as_config(), "ru");
        assert_eq!(
            merged.document.settings.layout_model,
            !base.settings.layout_model
        );
        assert_eq!(
            merged.document.backend_rules.resolve("notepad.exe"),
            BackendStrategy::PhysicalReplay
        );
        // An entry that ends up in both lists.
        let mut theirs = base.clone();
        theirs.word_exclusions.push("en-US: firefox".to_owned());
        let mut ours = base.clone();
        ours.user_dictionary.push("en-US: firefox".to_owned());
        assert_eq!(
            ConfigurationDocument::merge(&base, &ours, &theirs).conflicts,
            ["user_dictionary, word_exclusions"]
        );
    }

    #[test]
    fn rejects_unknown_or_future_schema() {
        assert!(ConfigurationDocument::from_text("[settings]\n").is_err());
        assert!(ConfigurationDocument::from_text("schema_version=4\n[settings]\n").is_err());
        assert!(ConfigurationDocument::from_text("schema_version=1\n[unknown]\n").is_err());
    }
}
