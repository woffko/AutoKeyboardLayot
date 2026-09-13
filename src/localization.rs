//! Application-owned UI catalogs, independent from keyboard/input languages.
//!
//! English is embedded. External catalogs are bounded data, never executable
//! code. Parse/load them outside the keyboard hook, then share an immutable
//! `Localizer` snapshot with UI consumers.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File, OpenOptions},
    io::Read,
    path::Path,
    sync::{Arc, OnceLock},
};

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};

pub const MAX_CATALOG_BYTES: usize = 512 * 1024;
const MAX_MESSAGES: usize = 512;
const MAX_MESSAGE_BYTES: usize = 8192;
const MAX_FORMATTED_BYTES: usize = 64 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 128;
const MAX_SCANNED_ENTRIES: usize = 1024;
const MAX_DIRECTORY_BYTES: usize = 8 * 1024 * 1024;
const MAX_ALIASES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    InvalidJson,
    UnsupportedFormat,
    InvalidLocale,
    InvalidDirection,
    ReservedLocale,
    TooLarge,
    InvalidMessage,
    UnknownMessage,
    PlaceholderMismatch,
    InvalidArguments,
    DuplicateLocale,
    Unreadable,
    UnsafeFile,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Do not echo untrusted catalog strings or interpolation arguments.
        write!(f, "translation catalog error: {self:?}")
    }
}
impl std::error::Error for CatalogError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextDirection {
    Ltr,
    Rtl,
}

/// UI-only preference; it never enables an input language or changes an HKL.
/// The private representation prevents writing unsafe INI values from the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiLanguagePreference(String);

impl Default for UiLanguagePreference {
    fn default() -> Self {
        Self("system".to_owned())
    }
}

impl UiLanguagePreference {
    /// Invalid optional UI data must not invalidate operational configuration
    /// and discard process exclusions. Such a value falls back to English.
    pub fn from_config(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("system") {
            return Self::default();
        }
        Self(normalize_locale(value).unwrap_or_else(|_| "en".to_owned()))
    }

    pub fn as_config(&self) -> &str {
        &self.0
    }
    pub fn is_system(&self) -> bool {
        self.0 == "system"
    }
    pub fn requested_locale<'a>(&'a self, system_locale: &'a str) -> &'a str {
        if self.is_system() {
            system_locale
        } else {
            &self.0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiLanguageChoice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    format: u32,
    locale: String,
    direction: TextDirection,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(deserialize_with = "unique_messages")]
    messages: BTreeMap<String, String>,
}

fn unique_messages<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    struct Messages;
    impl<'de> Visitor<'de> for Messages {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("unique message IDs and string values")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if values.len() >= MAX_MESSAGES || values.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive message IDs",
                    ));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Messages)
}

/// Normalize a bounded BCP-47-style catalog identifier, never a filesystem path.
pub fn normalize_locale(value: &str) -> Result<String, CatalogError> {
    if value.is_empty() || value.len() > 63 || !value.is_ascii() {
        return Err(CatalogError::InvalidLocale);
    }
    let mut parts = value.split('-');
    let language = parts.next().ok_or(CatalogError::InvalidLocale)?;
    if !(2..=8).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(CatalogError::InvalidLocale);
    }
    for part in parts {
        if part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(CatalogError::InvalidLocale);
        }
    }
    Ok(value.to_ascii_lowercase())
}

fn script_subtag(locale: &str) -> Option<&str> {
    locale
        .split('-')
        .nth(1)
        .filter(|part| part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
}

fn placeholders(value: &str) -> Result<BTreeSet<&str>, CatalogError> {
    let mut remaining = value;
    let mut names = BTreeSet::new();
    loop {
        let Some(start) = remaining.find('{') else {
            return if remaining.contains('}') {
                Err(CatalogError::InvalidMessage)
            } else {
                Ok(names)
            };
        };
        if remaining[..start].contains('}') {
            return Err(CatalogError::InvalidMessage);
        }
        let after = &remaining[start + 1..];
        let end = after.find('}').ok_or(CatalogError::InvalidMessage)?;
        let name = &after[..end];
        if name.is_empty()
            || name.len() > 64
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(CatalogError::InvalidMessage);
        }
        names.insert(name);
        remaining = &after[end + 1..];
    }
}

fn parse_catalog(bytes: &[u8], base: Option<&Catalog>) -> Result<Catalog, CatalogError> {
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(CatalogError::TooLarge);
    }
    let mut catalog: Catalog =
        serde_json::from_slice(bytes).map_err(|_| CatalogError::InvalidJson)?;
    if catalog.format != 1 {
        return Err(CatalogError::UnsupportedFormat);
    }
    catalog.locale = normalize_locale(&catalog.locale)?;
    if catalog.aliases.len() > MAX_ALIASES {
        return Err(CatalogError::TooLarge);
    }
    let mut seen_aliases = BTreeSet::new();
    for alias in &mut catalog.aliases {
        *alias = normalize_locale(alias)?;
        // Data may declare compatible regional names, but must not redirect
        // another language or override an explicitly requested script.
        if alias.split('-').next() != catalog.locale.split('-').next()
            || script_subtag(alias)
                .is_some_and(|script| Some(script) != script_subtag(&catalog.locale))
        {
            return Err(CatalogError::InvalidLocale);
        }
        if *alias == catalog.locale || !seen_aliases.insert(alias.clone()) {
            return Err(CatalogError::DuplicateLocale);
        }
    }
    if catalog.locale == "en" && catalog.direction != TextDirection::Ltr {
        return Err(CatalogError::InvalidDirection);
    }
    for (key, value) in &catalog.messages {
        if key.is_empty()
            || key.len() > 96
            || !key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_')
            || value.trim().is_empty()
            || value.len() > MAX_MESSAGE_BYTES
            || value
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            || value.chars().any(|c| matches!(c, '\u{202a}'..='\u{202e}'))
        {
            return Err(CatalogError::InvalidMessage);
        }
        let names = placeholders(value)?;
        if key == "locale.self_name" && (value.len() > 128 || value.chars().any(char::is_control)) {
            return Err(CatalogError::InvalidMessage);
        }
        if let Some(base) = base {
            let english = base.messages.get(key).ok_or(CatalogError::UnknownMessage)?;
            if names != placeholders(english)? {
                return Err(CatalogError::PlaceholderMismatch);
            }
        }
    }
    Ok(catalog)
}

fn english_catalog() -> Arc<Catalog> {
    static ENGLISH: OnceLock<Arc<Catalog>> = OnceLock::new();
    Arc::clone(ENGLISH.get_or_init(|| {
        Arc::new(
            parse_catalog(include_bytes!("../data/locales/en.json"), None)
                .expect("the embedded English catalog is validated by tests"),
        )
    }))
}

/// Validate the opened handle, not pathname metadata checked before open.
/// Unix opens are nonblocking and do not follow the final symlink. Windows
/// opens reparse points themselves and rejects them before reading. Atomic
/// replacement after open cannot redirect this handle to a different file.
fn open_catalog_file(path: &Path) -> Result<File, CatalogError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        };
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
        // Permit atomic replacement but not concurrent in-place writes while
        // this validated snapshot is being read.
        options.share_mode((FILE_SHARE_READ | FILE_SHARE_DELETE).0);
    }
    let file = options.open(path).map_err(|_| CatalogError::Unreadable)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{FILE_TYPE_DISK, GetFileType},
        };
        if unsafe { GetFileType(HANDLE(file.as_raw_handle())) } != FILE_TYPE_DISK {
            return Err(CatalogError::UnsafeFile);
        }
    }
    let metadata = file.metadata().map_err(|_| CatalogError::Unreadable)?;
    if !metadata.is_file() {
        return Err(CatalogError::UnsafeFile);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(CatalogError::UnsafeFile);
        }
    }
    if metadata.len() > MAX_CATALOG_BYTES as u64 {
        return Err(CatalogError::TooLarge);
    }
    Ok(file)
}

#[derive(Debug, Clone)]
pub struct CatalogRegistry {
    english: Arc<Catalog>,
    catalogs: BTreeMap<String, Arc<Catalog>>,
    aliases: BTreeMap<String, Arc<Catalog>>,
}

impl Default for CatalogRegistry {
    fn default() -> Self {
        Self {
            english: english_catalog(),
            catalogs: BTreeMap::new(),
            aliases: BTreeMap::new(),
        }
    }
}

impl CatalogRegistry {
    /// Add one external locale. English and duplicate locales cannot be replaced.
    pub fn add(&mut self, bytes: &[u8]) -> Result<(), CatalogError> {
        let catalog = parse_catalog(bytes, Some(&self.english))?;
        if std::iter::once(&catalog.locale)
            .chain(&catalog.aliases)
            .any(|name| name.split('-').next() == Some("en") || name == "system")
        {
            return Err(CatalogError::ReservedLocale);
        }
        if self.catalogs.contains_key(&catalog.locale)
            || self.aliases.contains_key(&catalog.locale)
            || catalog
                .aliases
                .iter()
                .any(|alias| self.catalogs.contains_key(alias) || self.aliases.contains_key(alias))
        {
            return Err(CatalogError::DuplicateLocale);
        }
        // Preflight all names before inserting anything: a rejected catalog
        // must not leave aliases pointing to an otherwise absent translation.
        let catalog = Arc::new(catalog);
        for alias in &catalog.aliases {
            self.aliases.insert(alias.clone(), Arc::clone(&catalog));
        }
        self.catalogs.insert(catalog.locale.clone(), catalog);
        Ok(())
    }

    /// Load data catalogs from an explicit local directory. Missing directories
    /// are normal; malformed files are isolated and never replace English.
    /// No recursion, symlink following, or unbounded reads are permitted.
    pub fn load_directory(&mut self, directory: &Path) -> Vec<CatalogError> {
        let mut errors = Vec::new();
        let metadata = match fs::symlink_metadata(directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return errors,
            Err(_) => return vec![CatalogError::Unreadable],
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return vec![CatalogError::UnsafeFile];
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return vec![CatalogError::Unreadable];
        };
        let mut catalogs = Vec::new();
        for (index, entry) in entries.take(MAX_SCANNED_ENTRIES + 1).enumerate() {
            if index >= MAX_SCANNED_ENTRIES {
                return vec![CatalogError::TooLarge];
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    errors.push(CatalogError::Unreadable);
                    continue;
                }
            };
            if entry
                .path()
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                catalogs.push(entry);
                if catalogs.len() > MAX_DIRECTORY_ENTRIES {
                    return vec![CatalogError::TooLarge];
                }
            }
        }
        // Stable precedence even if a manually populated directory contains
        // duplicate locale declarations; never depend on filesystem order.
        catalogs.sort_by_key(|entry| entry.file_name());
        let mut consumed = 0;
        for entry in catalogs {
            let path = entry.path();
            let result = (|| {
                let mut bytes = Vec::new();
                open_catalog_file(&path)?
                    .take(MAX_CATALOG_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| CatalogError::Unreadable)?;
                consumed += bytes.len();
                if consumed > MAX_DIRECTORY_BYTES {
                    return Err(CatalogError::TooLarge);
                }
                self.add(&bytes)
            })();
            if let Err(error) = result {
                errors.push(error);
            }
            if consumed > MAX_DIRECTORY_BYTES {
                break;
            }
        }
        errors
    }

    pub fn locales(&self) -> Vec<&str> {
        std::iter::once("en")
            .chain(self.catalogs.keys().map(String::as_str))
            .collect()
    }

    /// Native language names come from each catalog, not a closed language enum.
    pub fn choices(&self) -> Vec<UiLanguageChoice> {
        std::iter::once(&self.english)
            .chain(self.catalogs.values())
            .map(|catalog| UiLanguageChoice {
                id: catalog.locale.clone(),
                name: catalog
                    .messages
                    .get("locale.self_name")
                    .unwrap_or(&catalog.locale)
                    .clone(),
            })
            .collect()
    }

    /// Keep a saved but currently unavailable locale selectable, rather than
    /// silently overwriting the preference when an unrelated setting is saved.
    pub fn picker_choices(
        &self,
        preference: &UiLanguagePreference,
        system_name: &str,
    ) -> (Vec<UiLanguageChoice>, usize) {
        let mut choices = vec![UiLanguageChoice {
            id: "system".to_owned(),
            name: system_name.to_owned(),
        }];
        choices.extend(self.choices());
        let selected = choices
            .iter()
            .position(|choice| choice.id == preference.as_config())
            .unwrap_or_else(|| {
                let index = choices.len();
                choices.push(UiLanguageChoice {
                    id: preference.as_config().to_owned(),
                    name: self
                        .aliases
                        .get(preference.as_config())
                        .and_then(|catalog| catalog.messages.get("locale.self_name"))
                        .cloned()
                        .unwrap_or_else(|| preference.as_config().to_owned()),
                });
                index
            });
        (choices, selected)
    }

    pub fn select(&self, requested: &str) -> Localizer {
        let normalized = normalize_locale(requested).unwrap_or_else(|_| "en".to_owned());
        let parts: Vec<_> = normalized.split('-').collect();
        // Never silently cross an explicit script boundary (e.g. Hant -> Hans).
        let minimum = if script_subtag(&normalized).is_some() {
            2
        } else {
            1
        };
        for length in (minimum..=parts.len()).rev() {
            let candidate = parts[..length].join("-");
            if candidate == "en" {
                break;
            }
            if let Some(catalog) = self
                .catalogs
                .get(&candidate)
                .or_else(|| self.aliases.get(&candidate))
            {
                return Localizer {
                    english: Arc::clone(&self.english),
                    selected: Arc::clone(catalog),
                };
            }
        }
        Localizer {
            english: Arc::clone(&self.english),
            selected: Arc::clone(&self.english),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Localizer {
    english: Arc<Catalog>,
    selected: Arc<Catalog>,
}

impl Default for Localizer {
    fn default() -> Self {
        CatalogRegistry::default().select("en")
    }
}

impl Localizer {
    pub fn locale(&self) -> &str {
        &self.selected.locale
    }
    pub fn direction(&self) -> TextDirection {
        self.selected.direction
    }

    pub fn language_name(&self) -> &str {
        self.selected
            .messages
            .get("locale.self_name")
            .map(String::as_str)
            .unwrap_or(&self.selected.locale)
    }

    /// English keys absent from the selected catalog, before per-message
    /// fallback. This measures structural coverage, not translation quality.
    pub fn missing_message_ids(&self) -> Vec<&str> {
        self.english
            .messages
            .keys()
            .filter(|id| !self.selected.messages.contains_key(*id))
            .map(String::as_str)
            .collect()
    }

    pub fn text(&self, id: &str) -> &str {
        self.selected
            .messages
            .get(id)
            .or_else(|| self.english.messages.get(id))
            .map(String::as_str)
            .unwrap_or("Text unavailable")
    }

    pub fn format(&self, id: &str, arguments: &[(&str, &str)]) -> Result<String, CatalogError> {
        let template = self.text(id);
        let required = placeholders(template)?;
        let supplied: BTreeMap<_, _> = arguments.iter().copied().collect();
        if supplied.len() != arguments.len() || required != supplied.keys().copied().collect() {
            return Err(CatalogError::InvalidArguments);
        }
        let mut output = String::new();
        let mut remaining = template;
        while let Some(start) = remaining.find('{') {
            append_formatted(&mut output, &remaining[..start])?;
            let after = &remaining[start + 1..];
            let end = after.find('}').ok_or(CatalogError::InvalidMessage)?;
            append_formatted(&mut output, supplied[&after[..end]])?;
            remaining = &after[end + 1..];
        }
        append_formatted(&mut output, remaining)?;
        Ok(output)
    }
}

fn append_formatted(output: &mut String, part: &str) -> Result<(), CatalogError> {
    if part.len() > MAX_FORMATTED_BYTES.saturating_sub(output.len()) {
        return Err(CatalogError::TooLarge);
    }
    output.push_str(part);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external(locale: &str, messages: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(
            &serde_json::json!({"format":1,"locale":locale,"direction":"ltr","messages":messages}),
        )
        .unwrap()
    }

    #[test]
    fn english_is_complete_without_any_external_directory() {
        let registry = CatalogRegistry::default();
        assert!(registry.english.messages.len() >= 125);
        assert_eq!(registry.select("zz-ZZ").locale(), "en");
        assert_eq!(registry.select("en-GB").text("button.apply"), "Apply");
        assert_eq!(registry.select("../../ru").locale(), "en");
    }

    #[test]
    fn partial_regional_catalog_falls_back_per_message() {
        let mut registry = CatalogRegistry::default();
        registry
            .add(&external(
                "ru",
                serde_json::json!({"button.apply":"Применить"}),
            ))
            .unwrap();
        let localizer = registry.select("RU-ru");
        assert_eq!(localizer.locale(), "ru");
        assert_eq!(localizer.text("button.apply"), "Применить");
        assert_eq!(localizer.text("button.cancel"), "Cancel");
        assert_eq!(localizer.language_name(), "ru");
        assert_eq!(localizer.text("missing.code"), "Text unavailable");
    }

    #[test]
    fn coverage_does_not_count_english_fallback_as_a_translation() {
        let mut registry = CatalogRegistry::default();
        registry
            .add(&external(
                "es",
                serde_json::json!({"button.apply":"Aplicar"}),
            ))
            .unwrap();
        let selected = registry.select("es-MX");
        let missing = selected.missing_message_ids();
        assert!(missing.contains(&"button.cancel"));
        assert!(!missing.contains(&"button.apply"));
        assert_eq!(missing.len() + 1, registry.english.messages.len());
        assert!(registry.select("en").missing_message_ids().is_empty());

        let mut complete: serde_json::Value =
            serde_json::from_slice(include_bytes!("../data/locales/en.json")).unwrap();
        complete["locale"] = serde_json::json!("de");
        registry
            .add(&serde_json::to_vec(&complete).unwrap())
            .unwrap();
        assert!(registry.select("de").missing_message_ids().is_empty());
        // Identical text may legitimately be used for names or technical terms;
        // completeness must not pretend to establish linguistic acceptance.
    }

    #[test]
    fn text_direction_follows_the_selected_catalog_not_the_requested_locale() {
        let mut registry = CatalogRegistry::default();
        registry
            .add(br#"{"format":1,"locale":"ar","direction":"rtl","messages":{}}"#)
            .unwrap();
        assert_eq!(registry.select("ar-SA").direction(), TextDirection::Rtl);
        // A partially translated RTL catalog retains its direction even when
        // an individual message falls back to English.
        assert_eq!(registry.select("ar-SA").text("button.apply"), "Apply");
        // A missing whole catalog selects the English/LTR interface.
        assert_eq!(registry.select("ur-PK").direction(), TextDirection::Ltr);
        assert_eq!(registry.select("en").direction(), TextDirection::Ltr);
    }

    #[test]
    fn explicit_script_does_not_fall_back_to_a_different_script() {
        let mut registry = CatalogRegistry::default();
        registry
            .add(&external("zh", serde_json::json!({"button.apply":"应用"})))
            .unwrap();
        registry
            .add(&external(
                "zh-Hans",
                serde_json::json!({"button.apply":"应用"}),
            ))
            .unwrap();
        assert_eq!(registry.select("zh-Hans-CN").locale(), "zh-hans");
        assert_eq!(registry.select("zh-Hant-TW").locale(), "en");
    }

    fn aliased_catalog(locale: &str, aliases: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "format": 1, "locale": locale, "direction": "ltr", "aliases": aliases,
            "messages": {"locale.self_name": "中文（简体）", "button.apply": "应用"}
        }))
        .unwrap()
    }

    #[test]
    fn regional_aliases_select_the_declared_script_without_crossing_scripts() {
        let mut registry = CatalogRegistry::default();
        registry
            .add(&aliased_catalog(
                "zh-Hans",
                serde_json::json!(["zh-CN", "zh-SG"]),
            ))
            .unwrap();
        for requested in ["ZH-cn", "zh-SG", "zh-Hans-CN"] {
            assert_eq!(registry.select(requested).locale(), "zh-hans");
            assert_eq!(registry.select(requested).text("button.apply"), "应用");
        }
        for unavailable in ["zh", "zh-TW", "zh-HK", "zh-Hant", "zh-Hant-CN"] {
            assert_eq!(registry.select(unavailable).locale(), "en");
        }
        assert_eq!(registry.locales(), vec!["en", "zh-hans"]);
        let preference = UiLanguagePreference::from_config("zh-CN");
        let (choices, selected) = registry.picker_choices(&preference, "System");
        assert_eq!(choices[selected].id, "zh-cn");
        assert_eq!(choices[selected].name, "中文（简体）");
        assert_eq!(preference.as_config(), "zh-cn");
    }

    #[test]
    fn alias_validation_rejects_cross_language_script_duplicate_and_excessive_names() {
        assert_eq!(
            CatalogRegistry::default()
                .add(&aliased_catalog("system-us", serde_json::json!(["system"]))),
            Err(CatalogError::ReservedLocale)
        );
        for aliases in [
            serde_json::json!(["en-US"]),
            serde_json::json!(["ru-RU"]),
            serde_json::json!(["zh-Hant"]),
            serde_json::json!(["../zh"]),
        ] {
            assert_eq!(
                CatalogRegistry::default().add(&aliased_catalog("zh-Hans", aliases)),
                Err(CatalogError::InvalidLocale)
            );
        }
        for aliases in [
            serde_json::json!(["zh-CN", "ZH-cn"]),
            serde_json::json!(["zh-Hans"]),
        ] {
            assert_eq!(
                CatalogRegistry::default().add(&aliased_catalog("zh-Hans", aliases)),
                Err(CatalogError::DuplicateLocale)
            );
        }
        assert_eq!(
            CatalogRegistry::default().add(&aliased_catalog(
                "zh-Hans",
                serde_json::json!(vec!["zh-CN"; MAX_ALIASES + 1])
            )),
            Err(CatalogError::TooLarge)
        );
        assert_eq!(
            CatalogRegistry::default().add(&aliased_catalog("zh", serde_json::json!(["zh-Hant"]))),
            Err(CatalogError::InvalidLocale)
        );
    }

    #[test]
    fn alias_collisions_are_atomic_in_both_load_orders() {
        let first = aliased_catalog("zh-Hans", serde_json::json!(["zh-CN"]));
        let mut registry = CatalogRegistry::default();
        registry.add(&first).unwrap();
        assert_eq!(
            registry.add(&external("zh-CN", serde_json::json!({}))),
            Err(CatalogError::DuplicateLocale)
        );
        assert_eq!(
            registry.add(&aliased_catalog(
                "zh",
                serde_json::json!(["zh-SG", "zh-CN"])
            )),
            Err(CatalogError::DuplicateLocale)
        );
        assert_eq!(registry.select("zh-SG").locale(), "en");
        assert_eq!(registry.locales(), vec!["en", "zh-hans"]);

        let mut reverse = CatalogRegistry::default();
        reverse
            .add(&external("zh-CN", serde_json::json!({})))
            .unwrap();
        assert_eq!(reverse.add(&first), Err(CatalogError::DuplicateLocale));
        assert_eq!(reverse.select("zh-Hans").locale(), "en");
        assert_eq!(reverse.select("zh-CN").locale(), "zh-cn");
    }

    #[test]
    fn embedded_english_and_duplicate_locales_cannot_be_overwritten() {
        let mut registry = CatalogRegistry::default();
        assert_eq!(
            registry.add(&external("en", serde_json::json!({}))),
            Err(CatalogError::ReservedLocale)
        );
        assert_eq!(
            registry.add(&external("en-US", serde_json::json!({}))),
            Err(CatalogError::ReservedLocale)
        );
        let bytes = external("et", serde_json::json!({"button.apply":"Rakenda"}));
        registry.add(&bytes).unwrap();
        assert_eq!(registry.add(&bytes), Err(CatalogError::DuplicateLocale));
    }

    #[test]
    fn catalog_validation_rejects_unknown_keys_controls_and_wrong_placeholders() {
        let mut registry = CatalogRegistry::default();
        assert_eq!(
            registry.add(&external("ru", serde_json::json!({"not.a.key":"value"}))),
            Err(CatalogError::UnknownMessage)
        );
        assert_eq!(
            registry.add(&external("ru", serde_json::json!({"button.apply":"\u{0}"}))),
            Err(CatalogError::InvalidMessage)
        );
        assert_eq!(
            registry.add(&external(
                "ru",
                serde_json::json!({"button.apply":"\u{202e}Apply"})
            )),
            Err(CatalogError::InvalidMessage)
        );
        assert_eq!(
            registry.add(&external(
                "ru",
                serde_json::json!({"error.settings":"Bad settings"})
            )),
            Err(CatalogError::PlaceholderMismatch)
        );
    }

    #[test]
    fn malformed_duplicate_and_oversized_json_fail_closed() {
        let mut registry = CatalogRegistry::default();
        assert_eq!(registry.add(b"not json"), Err(CatalogError::InvalidJson));
        assert_eq!(registry.add(br#"{"format":1,"locale":"ru","direction":"ltr","messages":{"button.apply":"a","button.apply":"b"}}"#), Err(CatalogError::InvalidJson));
        assert_eq!(
            registry.add(&vec![b' '; MAX_CATALOG_BYTES + 1]),
            Err(CatalogError::TooLarge)
        );
    }

    #[test]
    fn formatting_interpolates_once_and_requires_exact_parameters() {
        let localizer = Localizer::default();
        assert_eq!(
            localizer
                .format("error.settings", &[("error", "literal {count}")])
                .unwrap(),
            "Invalid settings: literal {count}"
        );
        assert_eq!(
            localizer.format("error.settings", &[]),
            Err(CatalogError::InvalidArguments)
        );
        assert_eq!(
            localizer.format("button.apply", &[("error", "unused")]),
            Err(CatalogError::InvalidArguments)
        );
    }

    #[test]
    fn formatting_bounds_argument_size_and_repeated_placeholder_expansion() {
        let mut registry = CatalogRegistry::default();
        registry
            .add(&external(
                "ru",
                serde_json::json!({"error.settings":"{error}".repeat(100)}),
            ))
            .unwrap();
        let argument = "x".repeat(1024);
        assert_eq!(
            registry
                .select("ru")
                .format("error.settings", &[("error", &argument)]),
            Err(CatalogError::TooLarge)
        );
        let oversized = "x".repeat(MAX_FORMATTED_BYTES + 1);
        assert_eq!(
            Localizer::default().format("error.settings", &[("error", &oversized)]),
            Err(CatalogError::TooLarge)
        );
    }

    #[test]
    fn locale_ids_cannot_contain_paths_or_control_characters() {
        for invalid in [
            "",
            "../ru",
            "ru\\RU",
            "ru_RU",
            "ru\n",
            "ru--RU",
            "en-US.json",
            "русский",
        ] {
            assert_eq!(normalize_locale(invalid), Err(CatalogError::InvalidLocale));
        }
        assert_eq!(normalize_locale("pt-BR").unwrap(), "pt-br");
    }

    #[test]
    fn ui_preference_is_independent_and_preserves_uninstalled_valid_locales() {
        let system = UiLanguagePreference::default();
        assert_eq!(system.as_config(), "system");
        assert_eq!(system.requested_locale("ru-RU"), "ru-RU");
        let custom = UiLanguagePreference::from_config("DE-de");
        assert_eq!(custom.as_config(), "de-de");
        assert_eq!(custom.requested_locale("ru-RU"), "de-de");
        assert_eq!(
            CatalogRegistry::default()
                .select(custom.as_config())
                .locale(),
            "en"
        );
        assert_eq!(
            UiLanguagePreference::from_config("../../config").as_config(),
            "en"
        );
    }

    #[test]
    fn picker_names_use_catalog_data_and_system_is_reserved() {
        let mut registry = CatalogRegistry::default();
        assert_eq!(
            registry.add(&external("system", serde_json::json!({}))),
            Err(CatalogError::ReservedLocale)
        );
        registry
            .add(&external(
                "ru",
                serde_json::json!({"locale.self_name":"Русский"}),
            ))
            .unwrap();
        assert_eq!(
            registry.choices(),
            vec![
                UiLanguageChoice {
                    id: "en".to_owned(),
                    name: "English".to_owned()
                },
                UiLanguageChoice {
                    id: "ru".to_owned(),
                    name: "Русский".to_owned()
                },
            ]
        );
        let missing = UiLanguagePreference::from_config("de-DE");
        let (choices, index) = registry.picker_choices(&missing, "System");
        assert_eq!(choices[index].id, "de-de");
        assert_eq!(
            registry
                .picker_choices(&UiLanguagePreference::default(), "System")
                .1,
            0
        );
    }

    #[test]
    fn external_directory_isolates_invalid_files_and_preserves_english() {
        // Embed fixtures so cross-compiled Windows tests do not depend on a
        // Linux CARGO_MANIFEST_DIR path being present on the test machine.
        let directory = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            (
                "ru.json",
                include_bytes!("../tests/fixtures/locales/ru.json").as_slice(),
            ),
            (
                "et.json",
                include_bytes!("../tests/fixtures/locales/et.json").as_slice(),
            ),
            (
                "en.json",
                include_bytes!("../tests/fixtures/locales/en.json").as_slice(),
            ),
            (
                "invalid.json",
                include_bytes!("../tests/fixtures/locales/invalid.json").as_slice(),
            ),
        ] {
            fs::write(directory.path().join(name), bytes).unwrap();
        }
        let mut registry = CatalogRegistry::default();
        let errors = registry.load_directory(directory.path());
        assert!(errors.contains(&CatalogError::InvalidJson));
        assert!(errors.contains(&CatalogError::PlaceholderMismatch));
        assert!(errors.contains(&CatalogError::ReservedLocale));
        assert_eq!(registry.select("ru-RU").text("button.apply"), "Применить");
        assert_eq!(registry.select("et-EE").text("button.apply"), "Apply");
        assert_eq!(registry.select("en-US").text("button.apply"), "Apply");
    }

    #[test]
    fn ignored_files_do_not_consume_the_catalog_quota() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..150 {
            fs::write(
                directory.path().join(format!("ignored-{index}.txt")),
                b"ignored",
            )
            .unwrap();
        }
        fs::write(
            directory.path().join("ru.JSON"),
            include_bytes!("../tests/fixtures/locales/ru.json"),
        )
        .unwrap();
        let mut registry = CatalogRegistry::default();
        assert!(registry.load_directory(directory.path()).is_empty());
        assert_eq!(registry.select("ru-RU").text("button.apply"), "Применить");
    }

    #[test]
    fn an_opened_catalog_stays_bound_to_its_validated_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ru.json");
        let original = include_bytes!("../tests/fixtures/locales/ru.json");
        fs::write(&path, original).unwrap();
        let mut opened = open_catalog_file(&path).unwrap();
        fs::rename(&path, directory.path().join("old.json")).unwrap();
        fs::write(&path, b"replacement is not the opened snapshot").unwrap();
        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, original);
    }

    #[test]
    fn non_regular_catalogs_are_rejected_without_waiting_for_input() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("directory.json");
        fs::create_dir(&path).unwrap();
        assert!(open_catalog_file(&path).is_err());
        #[cfg(unix)]
        {
            use std::{
                ffi::CString,
                os::unix::{ffi::OsStrExt, fs::symlink},
                sync::mpsc::sync_channel,
                time::Duration,
            };
            let fifo = directory.path().join("pipe.json");
            let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let link = directory.path().join("link.json");
            symlink(&fifo, &link).unwrap();
            let (sender, receiver) = sync_channel(1);
            std::thread::spawn(move || {
                let rejected =
                    open_catalog_file(&fifo).is_err() && open_catalog_file(&link).is_err();
                let _ = sender.send(rejected);
            });
            assert!(
                receiver
                    .recv_timeout(Duration::from_secs(2))
                    .expect("catalog file open blocked on a FIFO")
            );
        }
    }

    #[test]
    fn every_literal_ui_message_reference_exists_in_the_english_catalog() {
        let english = english_catalog();
        let sources = [
            include_str!("../ui/settings.slint"),
            include_str!("windows_agent.rs"),
            include_str!("windows_agent/settings_window.rs"),
            include_str!("windows_agent/settings_window/hotkey_capture.rs"),
            include_str!("windows_agent/settings_window/package_import.rs"),
            include_str!("windows_agent/settings_window/package_import/online.rs"),
        ];
        let mut checked = BTreeSet::new();
        for source in sources {
            for marker in ["Localization.text(", "tr(", "tr_format("] {
                for (offset, _) in source.match_indices(marker) {
                    if offset > 0
                        && (source.as_bytes()[offset - 1].is_ascii_alphanumeric()
                            || source.as_bytes()[offset - 1] == b'_')
                    {
                        continue; // e.g. push_str(...) is not a tr(...) lookup.
                    }
                    let Some(rest) = source[offset + marker.len()..]
                        .trim_start()
                        .strip_prefix('"')
                    else {
                        continue;
                    };
                    let id = rest.split('"').next().unwrap();
                    assert!(
                        english.messages.contains_key(id),
                        "missing English message: {id}"
                    );
                    if marker != "tr_format(" {
                        assert!(
                            placeholders(&english.messages[id]).unwrap().is_empty(),
                            "formatted message used without parameters: {id}"
                        );
                    }
                    checked.insert(id.to_owned());
                }
            }
        }
        assert!(
            checked.len() >= 115,
            "catalog reference coverage unexpectedly shrank"
        );
    }
}
