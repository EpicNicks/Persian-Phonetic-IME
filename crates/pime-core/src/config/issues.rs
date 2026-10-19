//! Friendly configuration problems (PLAN.md 3.4): plain-language messages with
//! line numbers and hints, never mentioning parser internals.

use std::fmt::Write as _;

/// How serious a [`ConfigIssue`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Something in the file was ignored (the preset value is used instead).
    Error,
    /// Applied, but suspicious (for example an Arabic-only letter).
    Warning,
}

/// What kind of problem a [`ConfigIssue`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IssueKind {
    /// The file could not be read at all; the whole file was skipped.
    Syntax,
    /// A setting name that does not exist.
    UnknownKey,
    /// A section (`[name]`) that does not exist.
    UnknownSection,
    /// A value that is not one of the allowed choices.
    InvalidValue,
    /// A value of the wrong kind (text instead of true/false, ...).
    WrongType,
    /// `preset = "..."` names a preset that is not built in.
    UnknownPreset,
    /// A `[map.*]` key that cannot be used.
    BadMapKey,
    /// A candidate list with no entries.
    EmptyCandidates,
    /// A candidate list naming the same letter twice.
    DuplicateCandidates,
    /// A candidate that is not a single Persian letter.
    BadCandidate,
    /// An Arabic-only letter (warning unless `allow_arabic_forms`).
    ArabicForm,
    /// A 3-letter key whose 2-letter start has no entry (warning).
    UnreachablePrefix,
}

/// One problem found while loading a configuration file.
///
/// `line` and `column` are 1-based (0 when unknown). `snippet` is the offending
/// line of the file, trimmed. `message` and `hint` are complete sentences for
/// non-programmers; `hint` may be empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    /// Error (ignored) or Warning (applied).
    pub severity: Severity,
    /// The category of problem.
    pub kind: IssueKind,
    /// 1-based line number, 0 if unknown.
    pub line: usize,
    /// 1-based column (in characters), 0 if unknown.
    pub column: usize,
    /// The offending line of the file, trimmed.
    pub snippet: String,
    /// What is wrong.
    pub message: String,
    /// How to fix it (may be empty).
    pub hint: String,
}

/// Which part of a user file was dropped, summarising a list of issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// Nothing was skipped (warnings are allowed).
    None,
    /// At least one entry was skipped; the rest of the file applies.
    Entry,
    /// The file could not be read; the built-in preset is in use.
    WholeFile,
}

/// Summarises what was skipped because of `issues`.
pub fn fallback_of(issues: &[ConfigIssue]) -> Fallback {
    if issues.iter().any(|i| i.kind == IssueKind::Syntax) {
        Fallback::WholeFile
    } else if issues.iter().any(|i| i.severity == Severity::Error) {
        Fallback::Entry
    } else {
        Fallback::None
    }
}

/// Formats issues as the user-facing text of PLAN.md 3.4 (one block per issue,
/// separated by blank lines), naming the file `config.toml`. Empty input gives
/// an empty string.
pub fn format_issues(issues: &[ConfigIssue]) -> String {
    format_issues_for("config.toml", issues)
}

/// Like [`format_issues`], but names the file `file_name` (for example a full
/// path) in every location prefix.
pub fn format_issues_for(file_name: &str, issues: &[ConfigIssue]) -> String {
    let mut out = String::new();
    for (n, issue) in issues.iter().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        format_one(&mut out, file_name, issue);
    }
    out
}

fn location(file: &str, line: usize) -> String {
    if line == 0 {
        file.to_string()
    } else {
        format!("{file}, line {line}")
    }
}

fn message_and_hint(issue: &ConfigIssue) -> String {
    if issue.hint.is_empty() {
        issue.message.clone()
    } else {
        format!("{} {}", issue.message, issue.hint)
    }
}

fn format_one(out: &mut String, file: &str, issue: &ConfigIssue) {
    let loc = location(file, issue.line);
    match issue.kind {
        IssueKind::Syntax => {
            let _ = writeln!(out, "{loc}: {}", issue.message);
            let _ = writeln!(
                out,
                "Your whole settings file was skipped and the default settings are in use."
            );
            if !issue.hint.is_empty() {
                let _ = writeln!(out, "{}", issue.hint);
            }
        }
        IssueKind::UnknownKey | IssueKind::UnknownSection => {
            let _ = writeln!(out, "{loc}: {}", message_and_hint(issue));
        }
        _ => {
            if issue.severity == Severity::Warning {
                let _ = writeln!(
                    out,
                    "A note about your Persian Phonetic IME settings ({loc}):"
                );
            } else {
                let _ = writeln!(
                    out,
                    "Your Persian Phonetic IME settings have a problem ({loc}):"
                );
            }
            if !issue.snippet.is_empty() {
                let _ = writeln!(out, "  {}", issue.snippet);
            }
            let _ = writeln!(out, "{}", message_and_hint(issue));
            if issue.severity == Severity::Error {
                let what = match issue.kind {
                    IssueKind::UnknownPreset => {
                        "Everything else in your file is still in use.".to_string()
                    }
                    IssueKind::BadMapKey
                    | IssueKind::EmptyCandidates
                    | IssueKind::DuplicateCandidates
                    | IssueKind::BadCandidate => {
                        "That entry was skipped; everything else in your file is still in use."
                            .to_string()
                    }
                    _ => "That setting was skipped; everything else in your file is still in use."
                        .to_string(),
                };
                let _ = writeln!(out, "{what}");
            }
        }
    }
}

/// Optimal-string-alignment edit distance (Levenshtein plus adjacent swaps),
/// so the common typo "diigts" is one step from "digits".
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[n][m]
}

/// The closest of `known` to `word`, if it is close enough to be a likely typo.
pub(crate) fn suggest<'a>(word: &str, known: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let lower = word.to_lowercase();
    let limit = match lower.chars().count() {
        0..=3 => 1,
        4..=6 => 2,
        _ => 3,
    };
    known
        .into_iter()
        .map(|k| (edit_distance(&lower, k), k))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

/// `"a" or "b"` / `"a", "b" or "c"`.
pub(crate) fn or_list(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("\"{s}\"")).collect();
    match quoted.len() {
        0 => String::new(),
        1 => quoted[0].clone(),
        n => format!("{} or {}", quoted[..n - 1].join(", "), quoted[n - 1]),
    }
}

/// Turns a parser failure into a friendly reason for the offending line.
/// Returns (line, column, reason).
pub(crate) fn syntax_reason(src: &str, err: &toml_edit::TomlError) -> (usize, usize, String) {
    let offset = err.span().map(|s| s.start).unwrap_or(0).min(src.len());
    let (mut line, mut column) = line_col(src, offset);
    let raw = err.message();

    if let Some(key) = raw
        .strip_prefix("duplicate key `")
        .and_then(|r| r.split('`').next())
    {
        return (line, column, format!("\"{key}\" is set twice"));
    }

    // The parser sometimes reports the position just after the broken line.
    let mut text = line_text(src, line);
    if text.trim().is_empty() || offset >= src.len() {
        let mut prev = line;
        while prev > 1 && line_text(src, prev - 1).trim().is_empty() {
            prev -= 1;
        }
        if prev > 1 {
            let candidate = line_text(src, prev - 1);
            if has_unclosed_quote(candidate) || unclosed_brackets(candidate) {
                line = prev - 1;
                column = 1;
                text = candidate;
            }
        }
    }
    let trimmed = text.trim();
    let reason = if text.chars().any(|c| c.is_control() && c != '\t') {
        "it contains an invisible control character".to_string()
    } else if has_unclosed_quote(text) {
        "a closing quote is missing".to_string()
    } else if unclosed_brackets(text) {
        "a closing bracket is missing".to_string()
    } else if !trimmed.starts_with('[') && !trimmed.contains('=') {
        "an \"=\" is missing".to_string()
    } else if let Some(word) = bare_word_value(trimmed) {
        format!("text needs quotes around it, like \"{word}\"")
    } else {
        "it isn't written like a setting; settings look like digits = \"ascii\"".to_string()
    };
    (line, column, reason)
}

fn has_unclosed_quote(text: &str) -> bool {
    let code = strip_comment(text);
    let mut in_basic = false;
    let mut in_literal = false;
    let mut escaped = false;
    for c in code.chars() {
        if in_basic {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_basic = false;
            }
        } else if in_literal {
            if c == '\'' {
                in_literal = false;
            }
        } else if c == '"' {
            in_basic = true;
        } else if c == '\'' {
            in_literal = true;
        }
    }
    in_basic || in_literal
}

/// The value of `key = value` when it is an unquoted word such as `qaf`.
fn bare_word_value(line: &str) -> Option<&str> {
    let value = strip_comment(line).split_once('=')?.1.trim();
    let is_word = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        && !value.chars().next()?.is_ascii_digit()
        && !matches!(value, "true" | "false" | "inf" | "nan");
    is_word.then_some(value)
}

/// More `[` than `]` outside quotes and comments.
fn unclosed_brackets(text: &str) -> bool {
    let code = strip_comment(text);
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in code.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            },
        }
    }
    depth > 0
}

fn strip_comment(text: &str) -> &str {
    // A '#' outside quotes starts a comment.
    let mut in_basic = false;
    let mut in_literal = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_basic {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_basic = false;
            }
        } else if in_literal {
            if c == '\'' {
                in_literal = false;
            }
        } else if c == '"' {
            in_basic = true;
        } else if c == '\'' {
            in_literal = true;
        } else if c == '#' {
            return &text[..i];
        }
    }
    text
}

/// 1-based (line, column-in-chars) of a byte offset.
pub(crate) fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(src.len());
    while !src.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &src[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = src[line_start..offset].chars().count() + 1;
    (line, column)
}

/// The text of a 1-based line (without the newline).
pub(crate) fn line_text(src: &str, line: usize) -> &str {
    src.split('\n')
        .nth(line.saturating_sub(1))
        .unwrap_or("")
        .trim_end_matches('\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_swaps_as_one() {
        assert_eq!(edit_distance("diigts", "digits"), 1);
        assert_eq!(edit_distance("optons", "options"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn suggestions_need_to_be_close() {
        let known = ["digits", "punctuation", "gh"];
        assert_eq!(suggest("diigts", known), Some("digits"));
        assert_eq!(suggest("zzzzzzzz", known), None);
        assert_eq!(suggest("GH", known), Some("gh"));
    }

    #[test]
    fn or_lists() {
        assert_eq!(or_list(&["a"]), "\"a\"");
        assert_eq!(or_list(&["a", "b"]), "\"a\" or \"b\"");
        assert_eq!(or_list(&["a", "b", "c"]), "\"a\", \"b\" or \"c\"");
    }

    #[test]
    fn line_and_column() {
        let src = "ab\ncdé\nf";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 3), (2, 1));
        assert_eq!(line_col(src, 7), (2, 4));
        assert_eq!(line_col(src, 100), (3, 2));
        assert_eq!(line_text(src, 2), "cdé");
        assert_eq!(line_text(src, 9), "");
    }

    #[test]
    fn quote_detection_ignores_comments_and_escapes() {
        assert!(has_unclosed_quote("gh = \"qaf"));
        assert!(!has_unclosed_quote("gh = \"qaf\" # it's fine"));
        assert!(!has_unclosed_quote(r#"x = "a\"b""#));
        assert!(has_unclosed_quote("x = 'abc"));
    }

    fn issue(
        kind: IssueKind,
        severity: Severity,
        line: usize,
        msg: &str,
        hint: &str,
    ) -> ConfigIssue {
        ConfigIssue {
            severity,
            kind,
            line,
            column: 1,
            snippet: "gh = \"kaf\"".into(),
            message: msg.into(),
            hint: hint.into(),
        }
    }

    #[test]
    fn formats_match_plan_examples() {
        let bad = issue(
            IssueKind::InvalidValue,
            Severity::Error,
            7,
            "\"kaf\" isn't one of the choices for gh.",
            "Use \"ghain\" or \"qaf\".",
        );
        assert_eq!(
            format_issues(&[bad]),
            "Your Persian Phonetic IME settings have a problem (config.toml, line 7):\n  gh = \"kaf\"\n\
             \"kaf\" isn't one of the choices for gh. Use \"ghain\" or \"qaf\".\n\
             That setting was skipped; everything else in your file is still in use.\n"
        );
        let syntax = issue(
            IssueKind::Syntax,
            Severity::Error,
            12,
            "this line couldn't be read (a closing quote is missing).",
            "Fix the line and save the file; the change is picked up automatically.",
        );
        assert_eq!(
            format_issues(&[syntax]),
            "config.toml, line 12: this line couldn't be read (a closing quote is missing).\n\
             Your whole settings file was skipped and the default settings are in use.\n\
             Fix the line and save the file; the change is picked up automatically.\n"
        );
        let unknown = issue(
            IssueKind::UnknownKey,
            Severity::Error,
            3,
            "unknown setting \"diigts\".",
            "Did you mean \"digits\"?",
        );
        assert_eq!(
            format_issues(&[unknown]),
            "config.toml, line 3: unknown setting \"diigts\". Did you mean \"digits\"?\n"
        );
    }

    #[test]
    fn custom_file_name_in_every_prefix() {
        let issues = [
            issue(
                IssueKind::UnknownKey,
                Severity::Error,
                3,
                "unknown setting \"x\".",
                "",
            ),
            issue(IssueKind::InvalidValue, Severity::Error, 7, "bad.", ""),
            issue(IssueKind::ArabicForm, Severity::Warning, 0, "note.", ""),
            issue(IssueKind::Syntax, Severity::Error, 2, "broken.", ""),
        ];
        let f = r"C:\Users\me\PersianPhoneticIme\config.toml.preview";
        let text = format_issues_for(f, &issues);
        assert!(text.contains(&format!("{f}, line 3: unknown")), "{text}");
        assert!(text.contains(&format!("problem ({f}, line 7):")), "{text}");
        assert!(text.contains(&format!("settings ({f}):")), "{text}");
        assert!(text.contains(&format!("{f}, line 2: broken.")), "{text}");
        assert!(!text.replace(f, "").contains("config.toml"), "{text}");
        assert_eq!(
            format_issues(&issues),
            format_issues_for("config.toml", &issues)
        );
    }

    #[test]
    fn fallback_summary() {
        let w = issue(IssueKind::ArabicForm, Severity::Warning, 1, "m", "");
        let e = issue(IssueKind::InvalidValue, Severity::Error, 1, "m", "");
        let s = issue(IssueKind::Syntax, Severity::Error, 1, "m", "");
        assert_eq!(fallback_of(&[]), Fallback::None);
        assert_eq!(fallback_of(std::slice::from_ref(&w)), Fallback::None);
        assert_eq!(fallback_of(&[w.clone(), e.clone()]), Fallback::Entry);
        assert_eq!(fallback_of(&[e, s]), Fallback::WholeFile);
    }
}
