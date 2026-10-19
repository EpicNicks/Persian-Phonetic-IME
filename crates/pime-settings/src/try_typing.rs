//! The "Try typing" preview: Latin text typed into a plain box is replayed through
//! a real `pime_core::Session` with the user's current configuration.
//!
//! The typed text stays in the window; it is never saved or logged.

use pime_core::{keys, Dictionary, KeyEvent, Output, ResolvedConfig, Session};
use std::sync::Arc;

/// What the IME would have produced for the typed text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preview {
    /// Text the IME committed.
    pub committed: String,
    /// The word still being composed (shown underlined by the real IME).
    pub preedit: String,
    /// The alternatives offered for the last letter, if any.
    pub candidates: Vec<String>,
}

impl Preview {
    /// The text line: committed text, then the open word in brackets.
    pub fn text_line(&self) -> String {
        if self.preedit.is_empty() {
            self.committed.clone()
        } else {
            format!("{}[{}]", self.committed, self.preedit)
        }
    }

    /// "Alternatives: 1 س  2 ص  3 ث", or empty when no list is open. Each letter is
    /// followed by U+200E so the numbered items keep their order in a left-to-right line.
    pub fn alternatives_line(&self) -> String {
        if self.candidates.is_empty() {
            return String::new();
        }
        let items: Vec<String> = self
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{} {c}\u{200E}", i + 1))
            .collect();
        format!("Alternatives: {}", items.join("   "))
    }
}

fn apply(out: Output, committed: &mut String, last: &mut Output, passthrough: Option<char>) {
    committed.push_str(&out.commit);
    if !out.eaten {
        if let Some(c) = passthrough {
            committed.push(c);
        }
    }
    *last = out;
}

/// Replays `latin` key by key. Characters that have no US-QWERTY key (already
/// Persian text, emoji, ...) are inserted as they are, after committing the word;
/// a line break is Enter; a backspace character (U+0008) is Backspace.
pub fn preview(cfg: Arc<ResolvedConfig>, latin: &str) -> Preview {
    let mut session = Session::new(cfg, Arc::new(Dictionary::empty()));
    let mut committed = String::new();
    let mut last = Output::default();
    for c in latin.chars() {
        match c {
            '\r' => continue,
            '\n' => {
                let out = session.on_key(&KeyEvent::plain(keys::VK_RETURN));
                apply(out, &mut committed, &mut last, Some('\n'));
            }
            '\u{8}' => {
                let was_composing = session.is_composing();
                let out = session.on_key(&KeyEvent::plain(keys::VK_BACK));
                committed.push_str(&out.commit);
                if !was_composing {
                    committed.pop();
                }
                last = out;
            }
            _ => match keys::vk_for_char(c) {
                Some((vk, shift)) => {
                    let k = if shift {
                        KeyEvent::shifted(vk)
                    } else {
                        KeyEvent::plain(vk)
                    };
                    let out = session.on_key(&k);
                    apply(out, &mut committed, &mut last, Some(c));
                }
                None => {
                    let out = session.on_focus_lost();
                    committed.push_str(&out.commit);
                    committed.push(c);
                    last = Output::default();
                }
            },
        }
    }
    Preview {
        committed,
        preedit: last.preedit,
        candidates: last.candidates.map(|l| l.items).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::load_config;

    fn cfg(toml: &str) -> Arc<ResolvedConfig> {
        Arc::new(load_config(toml).config)
    }

    #[test]
    fn word_and_space() {
        let p = preview(cfg(""), "salam ");
        // v1 has no dictionary, so every vowel letter is typed.
        assert_eq!(p.committed, "سالام ");
        assert_eq!(p.preedit, "");
    }

    #[test]
    fn open_word_and_alternatives() {
        let p = preview(cfg(""), "drs s");
        assert_eq!(p.committed, "درس ");
        assert_eq!(p.preedit, "س");
        assert_eq!(p.candidates, ["س", "ص", "ث"]);
        assert_eq!(p.text_line(), "درس [س]");
        assert_eq!(
            p.alternatives_line(),
            "Alternatives: 1 س\u{200E}   2 ص\u{200E}   3 ث\u{200E}"
        );
    }

    #[test]
    fn follows_config() {
        assert_eq!(preview(cfg(""), "gh").preedit, "غ");
        assert_eq!(preview(cfg("[options]\ngh = \"qaf\"\n"), "gh").preedit, "ق");
        assert_eq!(preview(cfg(""), "12 ").committed, "۱۲ ");
        assert_eq!(
            preview(cfg("[options]\ndigits = \"ascii\"\n"), "12 ").committed,
            "12 "
        );
    }

    #[test]
    fn non_latin_and_line_breaks() {
        let p = preview(cfg(""), "ab\u{2764}\r\nb");
        assert_eq!(p.committed, "اب\u{2764}\n");
        assert_eq!(p.preedit, "ب");
    }

    #[test]
    fn empty() {
        let p = preview(cfg(""), "");
        assert_eq!(
            (p.text_line(), p.alternatives_line()),
            (String::new(), String::new())
        );
    }
}
