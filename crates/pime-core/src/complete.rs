//! Word completion interface (autocomplete, `docs/autocomplete.md` section 2).
//!
//! `pime-core` defines the [`Completer`] trait and the pure matching helpers;
//! the real dictionary lives in `pime-dict`, which implements the trait. The
//! [`ListCompleter`] here is a small fixture for goldens, fuzzing and demos.

/// Implemented by pime-dict. Called on the key path: no I/O, no blocking beyond short
/// in-memory locks, target < 0.5 ms. Must not panic on any input.
pub trait Completer: Send + Sync {
    /// At most `limit` completions of `prefix` (the preedit), best first. Every `text`
    /// starts with `prefix` verbatim, differs from it, and is unique in the result.
    fn complete(&self, prefix: &str, limit: usize, ctx: &CompleteCtx) -> Vec<Completion>;
}

/// Per-query context passed to [`Completer::complete`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteCtx {
    /// Whether learned history may influence ranking (`cfg.autocomplete.learning`).
    pub use_history: bool,
}

/// One completion returned by a [`Completer`].
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// The full word to commit; starts with the queried prefix.
    pub text: String,
    /// Ranking score (higher is better); informational for the session.
    pub score: f32,
}

/// The completion section of the popup.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionList {
    /// Completions in display order; completion `i` is popup row
    /// `alternates.len() + i` (number key `alternates.len() + i + 1`).
    pub items: Vec<String>,
    /// The highlighted completion; `None` means the typed text is "selected".
    pub highlighted: Option<usize>,
}

/// A learning signal for one dictionary word, emitted when an episode ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnEvent {
    /// The completion text as it was shown.
    pub word: String,
    /// What happened to it.
    pub kind: LearnKind,
}

/// What happened to a shown completion; ascending priority (dedupe keeps the max).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LearnKind {
    /// A row above the accepted one.
    Skipped,
    /// Visible while the typed text was committed as is.
    Ignored,
    /// Visible when Esc closed the popup.
    Dismissed,
    /// Accepted by the user.
    Accepted,
}

/// What the focused input context allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextPolicy {
    /// Show completions.
    pub completions: bool,
    /// Produce learn events.
    pub learning: bool,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        ContextPolicy {
            completions: true,
            learning: true,
        }
    }
}

/// Zero-width non-joiner.
pub const ZWNJ: char = '\u{200C}';

/// The per-char part of [`match_key`]: `None` drops the char.
fn key_char(c: char) -> Option<char> {
    match c {
        ZWNJ | '\u{0640}' | '\u{064B}'..='\u{065F}' | '\u{0670}' => None,
        '\u{064A}' | '\u{0649}' => Some('\u{06CC}'),
        '\u{0643}' => Some('\u{06A9}'),
        '\u{0629}' => Some('\u{0647}'),
        c => Some(c),
    }
}

/// Matching key: drop ZWNJ, U+0640 tatweel and U+064B..=U+065F, U+0670; map ي U+064A and
/// ى U+0649 -> ی U+06CC, ك U+0643 -> ک U+06A9, ة U+0629 -> ه U+0647. Nothing else.
pub fn match_key(s: &str) -> String {
    s.chars().filter_map(key_char).collect()
}

/// The ZWNJ rule (`docs/autocomplete.md` 2.4): the completion of `prefix` by the
/// dictionary display form `display`, never rewriting what was typed. `None` if
/// `display` does not extend `prefix`.
pub fn complete_with_zwnj_rule(prefix: &str, display: &str) -> Option<String> {
    let p = match_key(prefix);
    let mut want = p.chars();
    let mut need = p.chars().count();
    let mut tail_start = 0;
    if need > 0 {
        let mut found = false;
        for (i, c) in display.char_indices() {
            let Some(k) = key_char(c) else { continue };
            if want.next() != Some(k) {
                return None;
            }
            need -= 1;
            if need == 0 {
                tail_start = i + c.len_utf8();
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    let tail = &display[tail_start..];
    if match_key(tail).is_empty() {
        return None;
    }
    let mut out = String::with_capacity(prefix.len() + tail.len());
    out.push_str(prefix);
    if prefix.ends_with(ZWNJ) {
        out.push_str(tail.trim_start_matches(ZWNJ));
    } else {
        out.push_str(tail);
    }
    Some(out)
}

/// True if `preedit` may be completed: non-empty, only U+0600..=U+06FF and ZWNJ,
/// at least one non-ZWNJ char.
pub fn completable(preedit: &str) -> bool {
    preedit
        .chars()
        .all(|c| c == ZWNJ || ('\u{0600}'..='\u{06FF}').contains(&c))
        && preedit.chars().any(|c| c != ZWNJ)
}

/// Fixture completer for goldens, fuzz, demo, settings "try typing": linear scan, match by
/// `match_key` prefix, output via `complete_with_zwnj_rule`, order by score desc then text.
#[derive(Debug, Clone, Default)]
pub struct ListCompleter {
    words: Vec<(String, f32)>,
}

impl ListCompleter {
    /// A completer over `words` (display form, score).
    pub fn new(words: impl IntoIterator<Item = (String, f32)>) -> Self {
        ListCompleter {
            words: words.into_iter().collect(),
        }
    }
}

impl Completer for ListCompleter {
    fn complete(&self, prefix: &str, limit: usize, _ctx: &CompleteCtx) -> Vec<Completion> {
        if limit == 0 || match_key(prefix).is_empty() {
            return Vec::new();
        }
        let mut found: Vec<Completion> = self
            .words
            .iter()
            .filter_map(|(w, score)| {
                complete_with_zwnj_rule(prefix, w).map(|text| Completion {
                    text,
                    score: *score,
                })
            })
            .collect();
        found.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.text.cmp(&b.text))
        });
        let mut out: Vec<Completion> = Vec::with_capacity(limit.min(found.len()));
        for c in found {
            if out.len() >= limit {
                break;
            }
            if !out.iter().any(|o| o.text == c.text) {
                out.push(c);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIKHAHAM: &str = "می\u{200C}خواهم";

    #[test]
    fn match_key_normalises() {
        assert_eq!(match_key("مي\u{200C}خواهم"), "میخواهم");
        assert_eq!(match_key("كتاب"), "کتاب");
        assert_eq!(match_key("خانة"), "خانه");
        assert_eq!(match_key("مـَن\u{0670}ى"), "منی");
        assert_eq!(match_key(""), "");
    }

    #[test]
    fn zwnj_rule_examples() {
        let r = |p: &str, d: &str| complete_with_zwnj_rule(p, d);
        assert_eq!(r("می", MIKHAHAM).as_deref(), Some(MIKHAHAM));
        assert_eq!(r("میخ", MIKHAHAM).as_deref(), Some("میخواهم"));
        assert_eq!(r("می\u{200C}خ", MIKHAHAM).as_deref(), Some(MIKHAHAM));
        assert_eq!(r("می\u{200C}", MIKHAHAM).as_deref(), Some(MIKHAHAM));
        assert_eq!(r("می\u{200C}", "میخک").as_deref(), Some("می\u{200C}خک"));
        assert_eq!(r("می", "می"), None);
        assert_eq!(r("می", "می\u{200C}"), None);
        assert_eq!(r("مو", MIKHAHAM), None);
        assert_eq!(r("میخواهمم", MIKHAHAM), None);
        // Arabic forms in the dictionary are matched but never emitted beyond the prefix.
        assert_eq!(r("ک", "كتاب").as_deref(), Some("کتاب"));
    }

    #[test]
    fn completable_rules() {
        assert!(completable("سلام"));
        assert!(completable("می\u{200C}"));
        assert!(!completable(""));
        assert!(!completable("\u{200C}"));
        assert!(!completable("سلامc"));
    }

    #[test]
    fn list_completer_orders_and_limits() {
        let c = ListCompleter::new([
            ("سلطان".to_string(), 4.0),
            ("سلام".to_string(), 5.0),
            ("سلامت".to_string(), 4.2),
            ("سلامت".to_string(), 1.0),
            ("سل".to_string(), 9.0),
        ]);
        let ctx = CompleteCtx { use_history: true };
        let got: Vec<String> = c
            .complete("سل", 9, &ctx)
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(got, ["سلام", "سلامت", "سلطان"]);
        assert_eq!(c.complete("سل", 1, &ctx).len(), 1);
        assert!(c.complete("سل", 0, &ctx).is_empty());
        assert!(c.complete("", 5, &ctx).is_empty());
    }
}
