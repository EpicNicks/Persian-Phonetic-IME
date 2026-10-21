//! Word completion interface (autocomplete, `docs/autocomplete.md` section 2).
//!
//! `pime-core` defines the [`Completer`] trait and the pure matching helpers;
//! the real dictionary lives in `pime-dict`, which implements the trait. The
//! [`ListCompleter`] here is a small fixture for goldens, fuzzing and demos.

/// Implemented by pime-dict. Called on the key path: no I/O, no blocking beyond short
/// in-memory locks, target < 0.5 ms. Must not panic on any input.
pub trait Completer: Send + Sync {
    /// At most `limit` completions of `prefix` (the preedit), best first, unique by `text`.
    /// Exact rows (`edits == 0`) come first; their `text` starts with `prefix` verbatim
    /// and differs from it. Fuzzy rows (`edits > 0`, only if `ctx.fuzzy`) follow; their
    /// `text` is the dictionary display form and replaces the whole preedit.
    fn complete(&self, prefix: &str, limit: usize, ctx: &CompleteCtx) -> Vec<Completion>;
}

/// Per-query context passed to [`Completer::complete`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteCtx {
    /// Whether learned history may influence ranking (`cfg.autocomplete.learning`).
    pub use_history: bool,
    /// Whether typo-tolerant (edit-distance) matches may fill remaining slots
    /// (`cfg.autocomplete.fuzzy`).
    pub fuzzy: bool,
    /// Letters of the preedit's match key needed before fuzzy matching is tried
    /// (`cfg.autocomplete.fuzzy_min_chars`).
    pub fuzzy_min_chars: usize,
    /// Skip dictionary words flagged as vulgar (`cfg.autocomplete.filter_profanity`).
    pub hide_flagged: bool,
}

impl Default for CompleteCtx {
    /// The configuration defaults: history used, fuzzy on from 3 letters, flagged hidden.
    fn default() -> Self {
        CompleteCtx {
            use_history: true,
            fuzzy: true,
            fuzzy_min_chars: 3,
            hide_flagged: true,
        }
    }
}

/// One completion returned by a [`Completer`].
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// The full word to commit; starts with the queried prefix when `edits == 0`,
    /// otherwise it replaces the whole preedit.
    pub text: String,
    /// Ranking score (higher is better); informational for the session.
    pub score: f32,
    /// Edit distance from the preedit to the best prefix of the word; 0 = exact
    /// prefix completion.
    pub edits: u8,
}

/// The completion section of the popup.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionList {
    /// Completions in display order; completion `i` is popup row
    /// `alternates.len() + i` (number key `alternates.len() + i + 1`).
    pub items: Vec<String>,
    /// The highlighted completion; `None` means the typed text is "selected".
    pub highlighted: Option<usize>,
    /// Parallel to `items`: 0 = exact prefix completion, else the fuzzy edit distance.
    pub edits: Vec<u8>,
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

/// Largest edit distance a fuzzy match may have for a match key of `n` letters
/// (`docs/autocomplete.md` 13).
pub fn fuzzy_max_edits(n: usize) -> usize {
    if n <= 5 {
        1
    } else {
        2
    }
}

/// The smallest Damerau-Levenshtein distance (optimal string alignment: insert,
/// delete, substitute, adjacent transposition, each cost 1) between `key` and any
/// prefix of `word` (both match keys), or `None` if that exceeds `max`.
pub fn prefix_edit_distance(key: &str, word: &str, max: usize) -> Option<usize> {
    let a: Vec<char> = key.chars().collect();
    let b: Vec<char> = word.chars().collect();
    let n = a.len();
    // One row per prefix of `word`; column `i` is the prefix of `key` of length `i`.
    let mut prev2: Vec<usize> = Vec::new();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut best = prev[n];
    for j in 1..=b.len() {
        let mut cur = vec![j; n + 1];
        for i in 1..=n {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (prev[i] + 1).min(cur[i - 1] + 1).min(prev[i - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(prev2[i - 2] + 1);
            }
            cur[i] = d;
        }
        best = best.min(cur[n]);
        // Once every cell of a row is over budget, no longer prefix of `word` can
        // come back within it (a transposition from two rows up would also have
        // left a cell within budget in this row).
        if cur.iter().all(|d| *d > max) {
            break;
        }
        prev2 = std::mem::replace(&mut prev, cur);
    }
    (best <= max).then_some(best)
}

/// Fixture completer for goldens, fuzz, demo, settings "try typing": linear scan, match by
/// `match_key` prefix, output via `complete_with_zwnj_rule`, order by score desc then text.
/// If fewer than `limit` exact matches exist, `ctx.fuzzy` is set and the key has at least
/// `ctx.fuzzy_min_chars` letters, fuzzy matches (`docs/autocomplete.md` 13) fill the rest,
/// ordered by edits, score desc, shorter display, display. Ignores `ctx.use_history` and
/// `ctx.hide_flagged` (the fixture has no history or flags).
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
    fn complete(&self, prefix: &str, limit: usize, ctx: &CompleteCtx) -> Vec<Completion> {
        let key = match_key(prefix);
        if limit == 0 || key.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<Completion> = self
            .words
            .iter()
            .filter_map(|(w, score)| {
                complete_with_zwnj_rule(prefix, w).map(|text| Completion {
                    text,
                    score: *score,
                    edits: 0,
                })
            })
            .collect();
        found.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.text.cmp(&b.text))
        });
        let mut out: Vec<Completion> = Vec::with_capacity(limit.min(found.len()));
        push_unique(&mut out, found, limit);
        let n = key.chars().count();
        if out.len() < limit && ctx.fuzzy && n >= ctx.fuzzy_min_chars {
            let max = fuzzy_max_edits(n);
            let mut fuzzy: Vec<Completion> = self
                .words
                .iter()
                .filter_map(|(w, score)| {
                    let wk = match_key(w);
                    if wk == key {
                        return None;
                    }
                    let d = prefix_edit_distance(&key, &wk, max).filter(|d| *d >= 1)?;
                    Some(Completion {
                        text: w.clone(),
                        score: *score,
                        edits: d as u8,
                    })
                })
                .collect();
            fuzzy.sort_by(|a, b| {
                a.edits
                    .cmp(&b.edits)
                    .then_with(|| b.score.total_cmp(&a.score))
                    .then_with(|| a.text.chars().count().cmp(&b.text.chars().count()))
                    .then_with(|| a.text.cmp(&b.text))
            });
            push_unique(&mut out, fuzzy, limit);
        }
        out
    }
}

/// Appends `found` to `out` up to `limit` entries, skipping texts already present.
fn push_unique(out: &mut Vec<Completion>, found: Vec<Completion>, limit: usize) {
    for c in found {
        if out.len() >= limit {
            break;
        }
        if !out.iter().any(|o| o.text == c.text) {
            out.push(c);
        }
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
        let ctx = CompleteCtx::default();
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

    #[test]
    fn prefix_edit_distance_cases() {
        let d = |k: &str, w: &str| prefix_edit_distance(k, w, 2);
        assert_eq!(d("سلم", "سلام"), Some(1), "deletion");
        assert_eq!(d("کتب", "کتاب"), Some(1));
        assert_eq!(d("سلا", "سلام"), Some(0), "exact prefix");
        assert_eq!(d("سال", "سلام"), Some(1), "transposition");
        assert_eq!(d("سکام", "سلامت"), Some(1), "substitution");
        assert_eq!(d("سسلا", "سلام"), Some(1), "insertion");
        assert_eq!(d("ابپت", "سلام"), None);
        assert_eq!(prefix_edit_distance("سکم", "سلام", 1), None);
        assert_eq!(prefix_edit_distance("", "سلام", 0), Some(0));
        assert_eq!(prefix_edit_distance("سلم", "", 3), Some(3));
        assert_eq!(fuzzy_max_edits(3), 1);
        assert_eq!(fuzzy_max_edits(5), 1);
        assert_eq!(fuzzy_max_edits(6), 2);
    }

    #[test]
    fn list_completer_fuzzy() {
        let c = ListCompleter::new([
            ("سلام".to_string(), 5.0),
            ("سالم".to_string(), 3.0),
            ("سلامت".to_string(), 4.2),
            ("سلم".to_string(), 1.0),
            ("سلیم".to_string(), 2.0),
        ]);
        let ctx = CompleteCtx::default();
        let got = |p: &str, limit: usize, ctx: &CompleteCtx| -> Vec<(String, u8)> {
            c.complete(p, limit, ctx)
                .into_iter()
                .map(|c| (c.text, c.edits))
                .collect()
        };
        // Exact first; the preedit's own word is excluded; fuzzy by edits then score.
        assert_eq!(
            got("سلم", 9, &ctx),
            [
                ("سلام".to_string(), 1),
                ("سلامت".to_string(), 1),
                ("سالم".to_string(), 1),
                ("سلیم".to_string(), 1),
            ]
        );
        assert_eq!(got("سل", 9, &ctx).iter().filter(|r| r.1 > 0).count(), 0);
        let off = CompleteCtx {
            fuzzy: false,
            ..ctx
        };
        assert!(got("سلم", 9, &off).is_empty());
        let high = CompleteCtx {
            fuzzy_min_chars: 4,
            ..ctx
        };
        assert!(got("سلم", 9, &high).is_empty());
        // Fuzzy only fills slots the exact path left.
        assert_eq!(
            got("سلا", 2, &ctx).iter().map(|r| r.1).collect::<Vec<_>>(),
            [0, 0]
        );
        assert_eq!(
            got("سلا", 3, &ctx),
            [
                ("سلام".to_string(), 0),
                ("سلامت".to_string(), 0),
                ("سالم".to_string(), 1),
            ]
        );
    }
}
