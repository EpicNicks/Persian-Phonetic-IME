//! Tokeniser and renderer: Latin buffer -> preedit text (PLAN.md 2.1, 3.3).
//!
//! The whole buffer is re-tokenised on every key, longest match first over the
//! resolved map's keys (3, then 2, then 1 characters), so digraph fix-ups
//! (`s` + `h` -> ش) and Backspace need no special cases. A character that
//! starts no key (a lone `c` under `c = "none"`) is shown as itself.

use crate::config::{ResolvedConfig, ZWNJ};
use std::ops::Range;

/// A user's choice of candidate for one token, remembered across re-renders.
/// It applies only while a token with the same `start` and `key` exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOverride {
    /// Index of the token's first character in the Latin buffer (in chars).
    pub start: usize,
    /// The map key the choice was made for.
    pub key: String,
    /// Chosen candidate index.
    pub index: usize,
}

/// What a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A map key; renders as one of its candidates.
    Mapped,
    /// A zero-width non-joiner typed with Shift+Space.
    Zwnj,
    /// A character with no key; renders as itself.
    Unmapped,
}

/// One token of the Latin buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Index of the first character in the buffer (in chars).
    pub start: usize,
    /// Number of buffer characters covered.
    pub len: usize,
    /// Kind of token.
    pub kind: TokenKind,
    /// The covered buffer text (the map key for `Mapped`).
    pub key: String,
    /// Chosen candidate index (always 0 unless overridden; `Mapped` only).
    pub choice: usize,
}

/// A rendered buffer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rendered {
    /// The preedit text.
    pub text: String,
    /// UTF-16 range of `text` to draw with the "pending" attribute: the trailing
    /// run of unmapped characters (a lone `c` waiting for `h`), if any.
    pub pending: Option<Range<usize>>,
    /// The tokens, in order.
    pub tokens: Vec<Token>,
}

impl Rendered {
    /// The last token and its candidates, if it is ambiguous (two or more
    /// candidates): the v1 candidate list (PLAN.md 2.3).
    pub fn last_alternates<'c>(&self, cfg: &'c ResolvedConfig) -> Option<(&Token, &'c [String])> {
        let last = self.tokens.last()?;
        if last.kind != TokenKind::Mapped {
            return None;
        }
        let list = cfg.candidates(&last.key)?;
        (list.len() > 1).then_some((last, list))
    }
}

/// Splits a buffer into tokens (all choices 0).
pub fn tokenize(latin: &[char], cfg: &ResolvedConfig) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut key = String::new();
    while i < latin.len() {
        if latin[i] == ZWNJ {
            tokens.push(Token {
                start: i,
                len: 1,
                kind: TokenKind::Zwnj,
                key: ZWNJ.into(),
                choice: 0,
            });
            i += 1;
            continue;
        }
        let max = cfg.max_key_len().min(latin.len() - i);
        let mut matched = None;
        for len in (1..=max).rev() {
            let slice = &latin[i..i + len];
            if slice.contains(&ZWNJ) {
                continue;
            }
            key.clear();
            key.extend(slice);
            if cfg.candidates(&key).is_some() {
                matched = Some(len);
                break;
            }
        }
        match matched {
            Some(len) => {
                let key: String = latin[i..i + len].iter().collect();
                tokens.push(Token {
                    start: i,
                    len,
                    kind: TokenKind::Mapped,
                    key,
                    choice: 0,
                });
                i += len;
            }
            None => {
                let key = latin[i].to_string();
                tokens.push(Token {
                    start: i,
                    len: 1,
                    kind: TokenKind::Unmapped,
                    key,
                    choice: 0,
                });
                i += 1;
            }
        }
    }
    tokens
}

/// Renders a Latin buffer (which may contain U+200C) with the given choices.
pub fn render(latin: &str, cfg: &ResolvedConfig, overrides: &[ChoiceOverride]) -> Rendered {
    let chars: Vec<char> = latin.chars().collect();
    render_chars(&chars, cfg, overrides)
}

pub(crate) fn render_chars(
    latin: &[char],
    cfg: &ResolvedConfig,
    overrides: &[ChoiceOverride],
) -> Rendered {
    let mut tokens = tokenize(latin, cfg);
    let mut text = String::new();
    let mut utf16 = 0usize;
    let mut pending: Option<Range<usize>> = None;
    for t in &mut tokens {
        let piece: &str = match t.kind {
            TokenKind::Mapped => {
                let list = cfg.candidates(&t.key).unwrap_or_default();
                if let Some(o) = overrides
                    .iter()
                    .rev()
                    .find(|o| o.start == t.start && o.key == t.key)
                {
                    if o.index < list.len() {
                        t.choice = o.index;
                    }
                }
                list.get(t.choice).map(String::as_str).unwrap_or("")
            }
            TokenKind::Zwnj => "\u{200C}",
            TokenKind::Unmapped => &t.key,
        };
        let n = piece.encode_utf16().count();
        if t.kind == TokenKind::Unmapped {
            pending = Some(match pending {
                Some(r) if r.end == utf16 => r.start..utf16 + n,
                _ => utf16..utf16 + n,
            });
        } else if n > 0 {
            pending = None;
        }
        text.push_str(piece);
        utf16 += n;
    }
    Rendered {
        text,
        pending,
        tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_config;

    fn cfg(toml: &str) -> ResolvedConfig {
        let loaded = load_config(toml);
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        loaded.config
    }

    fn text(latin: &str, c: &ResolvedConfig) -> String {
        render(latin, c, &[]).text
    }

    #[test]
    fn longest_match_and_retroactive_digraphs() {
        let c = cfg("");
        assert_eq!(text("s", &c), "س");
        assert_eq!(text("sh", &c), "ش");
        assert_eq!(text("shh", &c), "شه");
        assert_eq!(text("salam", &c), "سالام");
        assert_eq!(text("khub", &c), "خوب");
    }

    #[test]
    fn lone_c_is_visible_and_pending() {
        let c = cfg("");
        let r = render("c", &c, &[]);
        assert_eq!(r.text, "c");
        assert_eq!(r.pending, Some(0..1));
        let r = render("sac", &c, &[]);
        assert_eq!(r.text, "ساc");
        assert_eq!(r.pending, Some(2..3));
        let r = render("ca", &c, &[]);
        assert_eq!(r.text, "cا");
        assert_eq!(r.pending, None, "only a trailing unmapped run is pending");
        assert_eq!(render("ch", &c, &[]).pending, None);
    }

    #[test]
    fn trigraph_prefix_shows_pending_c() {
        let c = load_config("[map.trigraphs]\ntch = [\"چ\"]\n").config;
        let r = render("tc", &c, &[]);
        assert_eq!(r.text, "تc");
        assert_eq!(r.pending, Some(1..2));
        assert_eq!(text("tch", &c), "چ");
    }

    #[test]
    fn zwnj_is_its_own_token() {
        let c = cfg("");
        let r = render("s\u{200C}h", &c, &[]);
        assert_eq!(r.text, "س\u{200C}ه");
        assert_eq!(r.tokens.len(), 3);
        assert_eq!(r.tokens[1].kind, TokenKind::Zwnj);
    }

    #[test]
    fn overrides_apply_to_matching_token_only() {
        let c = cfg("");
        let o = [ChoiceOverride {
            start: 0,
            key: "s".into(),
            index: 1,
        }];
        assert_eq!(render("s", &c, &o).text, "ص");
        assert_eq!(render("sa", &c, &o).text, "صا");
        assert_eq!(
            render("sh", &c, &o).text,
            "ش",
            "stale after the token changed"
        );
        let bad = [ChoiceOverride {
            start: 0,
            key: "s".into(),
            index: 9,
        }];
        assert_eq!(
            render("s", &c, &bad).text,
            "س",
            "out-of-range choice ignored"
        );
    }

    #[test]
    fn pending_range_counts_utf16_units() {
        let c =
            load_config("[options]\nallow_sequences = true\n[map.singles]\nb = [\"\u{FDF2}\"]\n")
                .config;
        let r = render("bc", &c, &[]);
        assert_eq!(r.pending, Some(1..2));
    }

    #[test]
    fn alternates_of_last_token() {
        let c = cfg("");
        let r = render("bas", &c, &[]);
        let (tok, list) = r.last_alternates(&c).unwrap();
        assert_eq!(tok.key, "s");
        assert_eq!(list.len(), 3);
        assert!(render("sh", &c, &[]).last_alternates(&c).is_none());
        assert!(render("s\u{200C}", &c, &[]).last_alternates(&c).is_none());
        assert!(render("", &c, &[]).last_alternates(&c).is_none());
    }
}
