//! Data-driven foreground-process routing for Windows text replacement.

use std::{collections::BTreeMap, fmt};

/// Text replacement strategy selected before any UIA or clipboard work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendStrategy {
    CapabilityProbe,
    ProtectedPaste,
    PhysicalReplay,
    ObserveOnly,
}

impl BackendStrategy {
    pub const fn id(self) -> &'static str {
        match self {
            Self::CapabilityProbe => "capability-probe",
            Self::ProtectedPaste => "protected-paste",
            Self::PhysicalReplay => "physical-replay",
            Self::ObserveOnly => "observe-only",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "capability" | "capability-probe" => Some(Self::CapabilityProbe),
            "paste" | "protected-paste" | "uia-paste" => Some(Self::ProtectedPaste),
            "replay" | "physical-replay" => Some(Self::PhysicalReplay),
            "disabled" | "observe" | "observe-only" => Some(Self::ObserveOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRuleError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for BackendRuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for BackendRuleError {}

/// Exact path/name rules plus one mandatory catch-all strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendRules {
    exact: BTreeMap<String, BackendStrategy>,
    fallback: BackendStrategy,
}

impl Default for BackendRules {
    fn default() -> Self {
        Self::from_lines([
            "notepad.exe = protected-paste",
            "viber.exe = protected-paste",
            "windowsterminal.exe = physical-replay",
            "openconsole.exe = physical-replay",
            "conhost.exe = physical-replay",
            "firefox.exe = physical-replay",
            "* = capability-probe",
        ])
        .expect("built-in backend rules must be valid")
    }
}

impl BackendRules {
    pub fn from_lines<'a>(
        lines: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, BackendRuleError> {
        let mut exact = BTreeMap::new();
        let mut fallback = None;
        for (index, raw_line) in lines.into_iter().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((pattern, strategy)) = line.split_once('=') else {
                return Err(BackendRuleError {
                    line: line_number,
                    message: "expected executable = backend".to_owned(),
                });
            };
            let pattern = normalize_rule_pattern(pattern).ok_or_else(|| BackendRuleError {
                line: line_number,
                message: "invalid executable name or path".to_owned(),
            })?;
            let strategy = BackendStrategy::from_id(strategy).ok_or_else(|| BackendRuleError {
                line: line_number,
                message: "unknown backend; use capability-probe, protected-paste, physical-replay, or observe-only".to_owned(),
            })?;
            if pattern == "*" {
                if fallback.replace(strategy).is_some() {
                    return Err(BackendRuleError {
                        line: line_number,
                        message: "duplicate catch-all rule".to_owned(),
                    });
                }
            } else if exact.insert(pattern, strategy).is_some() {
                return Err(BackendRuleError {
                    line: line_number,
                    message: "duplicate executable rule".to_owned(),
                });
            }
        }
        Ok(Self {
            exact,
            fallback: fallback.unwrap_or(BackendStrategy::CapabilityProbe),
        })
    }

    pub fn resolve(&self, path_or_name: &str) -> BackendStrategy {
        let Some(full) = normalize_process_path(path_or_name) else {
            return BackendStrategy::ObserveOnly;
        };
        if let Some(strategy) = self.exact.get(&full) {
            return *strategy;
        }
        let name = full.rsplit('\\').next().unwrap_or(&full);
        self.exact.get(name).copied().unwrap_or(self.fallback)
    }

    pub fn to_text(&self) -> String {
        let mut output = String::new();
        for (pattern, strategy) in &self.exact {
            output.push_str(pattern);
            output.push_str(" = ");
            output.push_str(strategy.id());
            output.push('\n');
        }
        output.push_str("* = ");
        output.push_str(self.fallback.id());
        output.push('\n');
        output
    }

    pub fn len(&self) -> usize {
        self.exact.len() + 1
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

fn normalize_rule_pattern(pattern: &str) -> Option<String> {
    let pattern = pattern.trim().trim_matches('"');
    if pattern == "*" {
        return Some("*".to_owned());
    }
    normalize_process_path(pattern)
}

fn normalize_process_path(path_or_name: &str) -> Option<String> {
    let value = path_or_name.trim().trim_matches('"');
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '*' | '?'))
    {
        return None;
    }
    Some(value.replace('/', "\\").to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_rules_route_physically_validated_hosts() {
        let rules = BackendRules::default();
        assert_eq!(
            rules.resolve("C:\\Windows\\System32\\notepad.exe"),
            BackendStrategy::ProtectedPaste
        );
        assert_eq!(
            rules.resolve("C:\\Program Files\\Mozilla Firefox\\FIREFOX.EXE"),
            BackendStrategy::PhysicalReplay
        );
        assert_eq!(rules.resolve("Viber.exe"), BackendStrategy::ProtectedPaste);
        assert_eq!(
            rules.resolve("unknown-editor.exe"),
            BackendStrategy::CapabilityProbe
        );
    }

    #[test]
    fn full_path_rule_wins_before_basename_and_output_round_trips() {
        let rules = BackendRules::from_lines([
            "editor.exe = capability-probe",
            "C:/Portable/editor.exe = physical-replay",
            "* = observe-only",
        ])
        .unwrap();
        assert_eq!(
            rules.resolve("C:\\Portable\\EDITOR.EXE"),
            BackendStrategy::PhysicalReplay
        );
        assert_eq!(
            rules.resolve("D:\\Apps\\editor.exe"),
            BackendStrategy::CapabilityProbe
        );
        assert_eq!(rules.resolve("other.exe"), BackendStrategy::ObserveOnly);
        assert_eq!(
            BackendRules::from_lines(rules.to_text().lines()).unwrap(),
            rules
        );
    }

    #[test]
    fn rejects_duplicates_wildcards_and_unknown_backends() {
        assert!(BackendRules::from_lines(["app.exe=auto", "APP.EXE=replay"]).is_err());
        assert!(BackendRules::from_lines(["*.exe=replay"]).is_err());
        assert!(BackendRules::from_lines(["app.exe=magic"]).is_err());
    }
}
