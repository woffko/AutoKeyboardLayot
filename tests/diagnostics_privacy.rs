//! Privacy guard for the diagnostics log.
//!
//! The Windows agent is compiled only on Windows, but its source is plain text: this test reads
//! it on every platform and inspects every call to `diagnostic(` and `gate_diagnostic(` in the
//! production part of the file. A format string that names key codes, characters, words, text or
//! window titles is a leak, or one step away from one, and fails the test.

const WINDOWS_AGENT: &str = include_str!("../src/windows_agent.rs");

/// Fragments that must not appear in a diagnostic format string (compared case-insensitively).
const FORBIDDEN_IN_FORMAT: &[&str] = &[
    "vk",
    "scan",
    "virtual_key",
    "char=",
    "word=",
    "text=",
    "title",
];

/// Identifiers that carry key identity and must not be passed as diagnostic arguments either.
const FORBIDDEN_IN_ARGUMENTS: &[&str] = &["virtual_key", "scan_code"];

/// The scanner must find at least this many calls, so a broken scanner cannot pass silently.
/// Lower it only when calls were removed on purpose.
const MINIMUM_CALLS: usize = 40;

const TEST_MODULE_MARKER: &str = "#[cfg(test)]\nmod tests";

struct Audit {
    calls: usize,
    violations: Vec<String>,
}

/// Returns the source before the unit-test module; the marker must exist so that test-only code
/// never counts as production code.
fn production_part(source: &str) -> String {
    let normalized = source.replace("\r\n", "\n");
    let end = normalized
        .find(TEST_MODULE_MARKER)
        .expect("the unit-test module marker is missing; update TEST_MODULE_MARKER");
    normalized[..end].to_owned()
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// Splits the text after an opening parenthesis into the string literals and the remaining
/// argument text of the call, up to the matching closing parenthesis.
fn call_parts(text: &str) -> (Vec<String>, String) {
    let characters: Vec<char> = text.chars().collect();
    let mut literals = Vec::new();
    let mut arguments = String::new();
    let mut depth = 1usize;
    let mut index = 0;
    while index < characters.len() && depth > 0 {
        let character = characters[index];
        match character {
            '"' => {
                let mut literal = String::new();
                index += 1;
                while index < characters.len() && characters[index] != '"' {
                    if characters[index] == '\\' && index + 1 < characters.len() {
                        literal.push(characters[index]);
                        index += 1;
                    }
                    literal.push(characters[index]);
                    index += 1;
                }
                literals.push(literal);
            }
            '\'' => {
                // A character literal such as '(' or '\n' must not disturb the depth count; a
                // lifetime tick has no closing quote within three characters and is skipped.
                let escaped = characters.get(index + 1) == Some(&'\\');
                let closing = if escaped { index + 3 } else { index + 2 };
                if characters.get(closing) == Some(&'\'') {
                    index = closing;
                } else {
                    arguments.push(character);
                }
            }
            '(' => {
                depth += 1;
                arguments.push(character);
            }
            ')' => {
                depth -= 1;
                if depth > 0 {
                    arguments.push(character);
                }
            }
            _ => arguments.push(character),
        }
        index += 1;
    }
    (literals, arguments)
}

fn audit(source: &str) -> Audit {
    let production = production_part(source);
    let mut calls = 0;
    let mut violations = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = production[search_from..].find("diagnostic(") {
        let start = search_from + relative;
        search_from = start + "diagnostic(".len();
        // Walk back over `gate_` and any other identifier prefix, then skip definitions.
        let identifier_start = production[..start]
            .rfind(|character: char| !is_identifier_character(character))
            .map_or(0, |position| position + 1);
        if production[..identifier_start].ends_with("fn ") {
            continue;
        }
        calls += 1;
        let line = production[..start].matches('\n').count() + 1;
        let (literals, arguments) = call_parts(&production[search_from..]);
        for literal in &literals {
            let lowered = literal.to_ascii_lowercase();
            for forbidden in FORBIDDEN_IN_FORMAT {
                if lowered.contains(forbidden) {
                    violations.push(format!(
                        "line {line}: format string {literal:?} contains `{forbidden}`"
                    ));
                }
            }
        }
        for forbidden in FORBIDDEN_IN_ARGUMENTS {
            if arguments.contains(forbidden) {
                violations.push(format!(
                    "line {line}: a diagnostic argument uses `{forbidden}`"
                ));
            }
        }
    }
    Audit { calls, violations }
}

#[test]
fn diagnostics_never_log_key_identity() {
    let result = audit(WINDOWS_AGENT);
    assert!(
        result.calls >= MINIMUM_CALLS,
        "the scanner found only {} diagnostic calls (expected at least {MINIMUM_CALLS})",
        result.calls
    );
    assert!(
        result.violations.is_empty(),
        "diagnostics must never record key codes, scan codes, characters, words, text or titles:\n{}",
        result.violations.join("\n")
    );
}

fn synthetic(body: &str) -> String {
    format!("fn run() {{\n{body}\n}}\n#[cfg(test)]\nmod tests {{}}\n")
}

#[test]
fn scanner_flags_key_identity_in_format_strings() {
    let leaks = [
        r#"self.diagnostic("input", format!("result=suppressed vk={}", event.key));"#,
        r#"state.gate_diagnostic("abort", format!("scan_code={}", code));"#,
        r#"self.diagnostic("input", format!("key={virtual_key}"));"#,
        r#"self.diagnostic("word_discard", format!("word={}", text));"#,
        r#"self.diagnostic("context", format!("Title={}", name));"#,
        r#"self.diagnostic("input", format!("key={}", event.virtual_key));"#,
        r#"self.diagnostic("input", format!("code={}", event.scan_code));"#,
    ];
    for leak in leaks {
        let result = audit(&synthetic(leak));
        assert_eq!(result.calls, 1, "{leak}");
        assert!(!result.violations.is_empty(), "not flagged: {leak}");
    }
}

#[test]
fn scanner_accepts_counts_categories_and_ignores_definitions() {
    let clean = r#"
        fn diagnostic(&self, event: &'static str, details: String) {}
        fn gate_diagnostic(&self, phase: &str, details: String) {}
        self.diagnostic("input", format!("result=suppressed reason=unsupported chars={} route={:?}", count, route));
        self.gate_diagnostic("abort", format!("token={token} held={held_count}"));
    "#;
    let result = audit(&synthetic(clean));
    assert_eq!(result.calls, 2);
    assert!(result.violations.is_empty(), "{:?}", result.violations);
}

#[test]
fn scanner_is_not_confused_by_delimiters_inside_literals() {
    let tricky = r#"
        self.diagnostic("a", format!("closing ) and \" quote {}", ')'));
        self.diagnostic("b", format!("vk={}", '('));
    "#;
    let result = audit(&synthetic(tricky));
    assert_eq!(result.calls, 2);
    assert_eq!(result.violations.len(), 1, "{:?}", result.violations);
    assert!(result.violations[0].contains("vk"));
}

#[test]
fn scanner_ignores_calls_in_the_unit_test_module() {
    let source = "fn run() {}\n#[cfg(test)]\nmod tests {\n    fn t() { self.diagnostic(\"x\", format!(\"vk={}\", 1)); }\n}\n";
    let result = audit(source);
    assert_eq!(result.calls, 0);
    assert!(result.violations.is_empty());
}
