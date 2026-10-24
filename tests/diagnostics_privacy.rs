//! Privacy guard for the diagnostics log.
//!
//! The Windows agent is compiled only on Windows, but its source is plain text: this test reads
//! it on every platform and inspects every call to `diagnostic(` and `gate_diagnostic(` in the
//! production part of the file, and every `format!(` in the functions that build details for
//! them. A format string that names key codes, characters, words, text or window titles is a
//! leak, or one step away from one, and fails the test; so does a text value passed as an
//! argument other than through a count.

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

/// Names of values that hold text (a word, its replacement, the text around the caret). One may
/// reach a diagnostic only through a count or a presence test (`COUNTED`), never by itself, as
/// an argument or inline in a format string.
const TEXT_VALUES: &[&str] = &[
    "original",
    "replacement",
    "insert_text",
    "delete_text",
    "current_text",
    "next_text",
    "candidate_text",
    "expected_text",
    "text",
    "word",
    "typed",
];

/// What may follow a text value in a diagnostic argument.
const COUNTED: &[&str] = &[
    ".chars().count()",
    ".encode_utf16().count()",
    ".len()",
    ".is_empty()",
];

/// Functions and impl blocks that build the details of diagnostic lines. Every `format!(` in
/// them is held to the same rules as a diagnostic call, and each must exist in the agent, so a
/// rename cannot drop one from the scan silently.
const DETAIL_BUILDERS: &[&str] = &[
    "fn log_word_discard(",
    "impl WordDiscardDetail {",
    "fn format_text_barrier_report(",
];

/// The scanner must find at least this many calls, so a broken scanner cannot pass silently.
/// Lower it only when calls were removed on purpose.
const MINIMUM_CALLS: usize = 40;

const TEST_MODULE_MARKER: &str = "#[cfg(test)]\nmod tests";

struct Audit {
    calls: usize,
    builders: usize,
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

/// Names used inline in a format string, such as `word` in `"{word}"` or `"{word:?}"`.
fn inline_arguments(literal: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = literal;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if let Some(escaped) = rest.strip_prefix('{') {
            rest = escaped;
            continue;
        }
        let length = rest
            .find(|character: char| !is_identifier_character(character))
            .unwrap_or(rest.len());
        let name = &rest[..length];
        let closes = rest[length..].starts_with('}') || rest[length..].starts_with(':');
        if closes
            && name
                .starts_with(|character: char| character.is_ascii_alphabetic() || character == '_')
        {
            names.push(name);
        }
        rest = &rest[length..];
    }
    names
}

/// Every identifier in `text` with the text that follows it.
fn identifiers(text: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut previous_is_identifier = false;
    let mut start = None;
    for (index, character) in text.char_indices() {
        let is_identifier = is_identifier_character(character);
        if is_identifier && !previous_is_identifier {
            start = Some(index);
        }
        if !is_identifier && let Some(begin) = start.take() {
            found.push((&text[begin..index], &text[index..]));
        }
        previous_is_identifier = is_identifier;
    }
    if let Some(begin) = start {
        found.push((&text[begin..], ""));
    }
    found
}

fn check_parts(line: usize, literals: &[String], arguments: &str, violations: &mut Vec<String>) {
    for literal in literals {
        let lowered = literal.to_ascii_lowercase();
        for forbidden in FORBIDDEN_IN_FORMAT {
            if lowered.contains(forbidden) {
                violations.push(format!(
                    "line {line}: format string {literal:?} contains `{forbidden}`"
                ));
            }
        }
        for name in inline_arguments(literal) {
            if TEXT_VALUES.contains(&name) {
                violations.push(format!(
                    "line {line}: format string {literal:?} writes the text value `{name}`"
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
    for (name, rest) in identifiers(arguments) {
        if TEXT_VALUES.contains(&name) && !COUNTED.iter().any(|suffix| rest.starts_with(suffix)) {
            violations.push(format!(
                "line {line}: the text value `{name}` is passed without being counted"
            ));
        }
    }
}

/// The byte range of the body of the first item that starts with `signature`, from its opening
/// brace to the matching closing one. Braces inside string and character literals are skipped.
fn item_body(source: &str, signature: &str) -> Option<std::ops::Range<usize>> {
    let start = source.find(signature)?;
    let open = start + source[start..].find('{')?;
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
            }
            b'\'' if bytes.get(index + 2) == Some(&b'\'') => index += 2,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open..index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
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
        check_parts(line, &literals, &arguments, &mut violations);
    }
    let mut builders = 0;
    for builder in DETAIL_BUILDERS {
        let Some(body) = item_body(&production, builder) else {
            continue;
        };
        builders += 1;
        let mut search_from = body.start;
        while let Some(relative) = production[search_from..body.end].find("format!(") {
            let start = search_from + relative;
            search_from = start + "format!(".len();
            let line = production[..start].matches('\n').count() + 1;
            let (literals, arguments) = call_parts(&production[search_from..body.end]);
            check_parts(line, &literals, &arguments, &mut violations);
        }
    }
    Audit {
        calls,
        builders,
        violations,
    }
}

#[test]
fn diagnostics_never_log_key_identity() {
    let result = audit(WINDOWS_AGENT);
    assert!(
        result.calls >= MINIMUM_CALLS,
        "the scanner found only {} diagnostic calls (expected at least {MINIMUM_CALLS})",
        result.calls
    );
    assert_eq!(
        result.builders,
        DETAIL_BUILDERS.len(),
        "a detail builder is missing from the agent; update DETAIL_BUILDERS"
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

#[test]
fn scanner_flags_text_values_unless_they_are_counted() {
    let leaks = [
        r#"self.diagnostic("candidate", format!("source={}", detection.original));"#,
        r#"self.diagnostic("candidate", format!("n={} {}", count, pending.transaction.undo.insert_text));"#,
        r#"self.diagnostic("hotkey", format!("result=ignored current={current_text}"));"#,
        r#"self.diagnostic("hotkey", format!("result=ignored value={next_text:?}"));"#,
        r#"self.diagnostic("word_discard", format!("reason={reason} {}", cycle.text.clone()));"#,
    ];
    for leak in leaks {
        let result = audit(&synthetic(leak));
        assert_eq!(result.calls, 1, "{leak}");
        assert!(!result.violations.is_empty(), "not flagged: {leak}");
    }
    let counted = r#"
        self.diagnostic("candidate", format!("original_chars={} replacement_chars={} expected_utf16={} empty={}", detection.original.chars().count(), detection.replacement.chars().count(), pending.transaction.undo.insert_text.encode_utf16().count(), cycle.text.is_empty()));
        self.diagnostic("hotkey", format!("route={:?} manual_only={} reset={}", self.text_edit_backend, self.manual_only_word, self.last_word_reset));
        self.diagnostic("hotkey", format!("escaped={{text}} positional={}", 1));
    "#;
    let result = audit(&synthetic(counted));
    assert_eq!(result.calls, 3);
    assert!(result.violations.is_empty(), "{:?}", result.violations);
}

#[test]
fn scanner_holds_the_detail_builders_to_the_same_rules() {
    let leaking_builder = r#"
        fn log_word_discard(&self, reason: &'static str, detail: WordDiscardDetail) {
            let extra = format!("text={}", self.word);
            self.diagnostic("word_discard", format!("reason={reason} {extra}"));
        }
        fn format_text_barrier_report(report: TextBarrierReport) -> String {
            format!("stage={:?} vk={}", report.stage, report.original)
        }
    "#;
    let result = audit(&synthetic(leaking_builder));
    assert_eq!(result.builders, 2);
    let joined = result.violations.join("\n");
    for expected in ["`text=`", "`word`", "`vk`", "`original`"] {
        assert!(
            joined.contains(expected),
            "{expected} not flagged: {joined}"
        );
    }
    let clean_builder = r#"
        impl WordDiscardDetail {
            fn describe(self) -> String {
                match self { Self::Rule(rule) => format!("rule={rule}"), Self::None => String::new() }
            }
        }
    "#;
    let result = audit(&synthetic(clean_builder));
    assert_eq!(result.builders, 1);
    assert!(result.violations.is_empty(), "{:?}", result.violations);
}
