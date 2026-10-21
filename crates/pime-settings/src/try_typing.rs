//! The "Try typing" preview: Latin text typed into a plain box is replayed through
//! a real `pime_core::Session` with the user's current configuration.
//!
//! The typed text stays in the window; it is never saved or logged.

use pime_core::{
    keys, Completer, ContextPolicy, Dictionary, KeyEvent, Output, ResolvedConfig, Session,
};
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
    /// The word completions offered below the alternatives, if any.
    pub completions: Vec<String>,
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

impl Preview {
    /// "Suggestions: 1 word  2 word", or empty. Numbering continues after the alternatives.
    pub fn completions_line(&self) -> String {
        if self.completions.is_empty() {
            return String::new();
        }
        let first = self.candidates.len();
        let items: Vec<String> = self
            .completions
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{} {c}\u{200E}", first + i + 1))
            .collect();
        format!("Suggestions: {}", items.join("   "))
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
    preview_with(cfg, latin, None)
}

/// [`preview`] with a word completer (the real dictionary in the window). Learning
/// is off, so nothing typed is ever recorded.
pub fn preview_with(
    cfg: Arc<ResolvedConfig>,
    latin: &str,
    completer: Option<Arc<dyn Completer>>,
) -> Preview {
    let mut session = Session::new(cfg, Arc::new(Dictionary::empty()));
    session.set_context_policy(ContextPolicy {
        completions: true,
        learning: false,
    });
    session.set_completer(completer);
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
        completions: last.completions.map(|l| l.items).unwrap_or_default(),
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
    fn completions_from_a_completer_without_learning() {
        let c: Arc<dyn Completer> = Arc::new(pime_core::ListCompleter::new([
            ("سالام".to_string(), 10.0),
            ("سالامت".to_string(), 5.0),
        ]));
        let on = cfg("[autocomplete]\nenabled = true\npredict_min_chars = 2\n");
        let p = preview_with(on.clone(), "salam", Some(c.clone()));
        assert!(p.completions.contains(&"سالامت".to_string()), "{p:?}");
        assert!(p.completions_line().starts_with("Suggestions: "));
        assert!(preview_with(on, "salam", None).completions.is_empty());
        assert!(preview_with(cfg(""), "salam", Some(c))
            .completions
            .is_empty());
    }

    #[test]
    fn fuzzy_and_filter_follow_config() {
        let c: Arc<dyn Completer> =
            Arc::new(pime_core::ListCompleter::new([("سالام".to_string(), 10.0)]));
        let base = "[autocomplete]
enabled = true
predict_min_chars = 2
";
        let on = preview_with(
            cfg(&format!(
                "{base}fuzzy = true
"
            )),
            "salaam",
            Some(c.clone()),
        );
        let off = preview_with(
            cfg(&format!(
                "{base}fuzzy = false
"
            )),
            "salaam",
            Some(c),
        );
        assert!(off.completions.is_empty(), "{off:?}");
        assert!(on.completions.len() >= off.completions.len());
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
