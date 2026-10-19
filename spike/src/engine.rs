//! Tiny hard-coded phonetic engine for the spike. Pure logic, no Windows types, so it is
//! unit-tested with plain `cargo test`. The real engine is `pime-core` (WS2); this only has to be
//! rich enough to exercise every TSF path (compose, re-render, commit, cancel, ZWNJ, pass-through).

pub const ZWNJ: char = '\u{200C}';

/// Virtual-key codes the shell feeds in (same numeric values as Win32 VK_*; duplicated so this
/// module stays Windows-free).
pub mod vk {
    pub const BACK: u32 = 0x08;
    pub const TAB: u32 = 0x09;
    pub const RETURN: u32 = 0x0D;
    pub const SHIFT: u32 = 0x10;
    pub const CONTROL: u32 = 0x11;
    pub const MENU: u32 = 0x12;
    pub const CAPITAL: u32 = 0x14;
    pub const ESCAPE: u32 = 0x1B;
    pub const SPACE: u32 = 0x20;
    pub const KEY_0: u32 = 0x30;
    pub const KEY_9: u32 = 0x39;
    pub const KEY_A: u32 = 0x41;
    pub const KEY_Z: u32 = 0x5A;
    pub const LWIN: u32 = 0x5B;
    pub const RWIN: u32 = 0x5C;
    pub const LSHIFT: u32 = 0xA0;
    pub const RMENU: u32 = 0xA5;
    pub const OEM_1: u32 = 0xBA;
    pub const OEM_PLUS: u32 = 0xBB;
    pub const OEM_COMMA: u32 = 0xBC;
    pub const OEM_MINUS: u32 = 0xBD;
    pub const OEM_PERIOD: u32 = 0xBE;
    pub const OEM_2: u32 = 0xBF;
    pub const OEM_3: u32 = 0xC0;
    pub const OEM_4: u32 = 0xDB;
    pub const OEM_5: u32 = 0xDC;
    pub const OEM_6: u32 = 0xDD;
    pub const OEM_7: u32 = 0xDE;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// `a`-`z` or `'`, already folded to lowercase.
    Letter(char),
    Space,
    ShiftSpace,
    Backspace,
    Escape,
    Enter,
    /// A printable non-letter key resolved through the fixed US-QWERTY table.
    Printable(char),
    /// Shift/Ctrl/Alt/Win/CapsLock pressed on their own: never commits.
    Modifier,
    /// Anything else (arrows, Tab, F-keys, Ctrl/Alt chords...).
    Other,
}

/// Experimental bidi wrapping of the displayed composition (VM A/B test, docs/spike.md 8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BidiMark {
    #[default]
    None,
    /// U+200F RIGHT-TO-LEFT MARK before and after.
    Rlm,
    /// U+202B RIGHT-TO-LEFT EMBEDDING ... U+202C POP DIRECTIONAL FORMATTING.
    Rle,
    /// U+2067 RIGHT-TO-LEFT ISOLATE ... U+2069 POP DIRECTIONAL ISOLATE.
    Rli,
}

impl BidiMark {
    pub fn parse(s: &str) -> BidiMark {
        match s.trim().to_ascii_lowercase().as_str() {
            "rlm" => BidiMark::Rlm,
            "rle" => BidiMark::Rle,
            "rli" => BidiMark::Rli,
            _ => BidiMark::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Enter commits and is then passed to the app (needs a TF_ES_SYNC commit).
    pub enter_passes: bool,
    /// While idle, eat printable non-letter keys and insert the US-QWERTY character ourselves
    /// (instead of letting the fa-IR layout produce Persian-layout characters).
    pub eat_printable: bool,
    /// Space keeps "word + space" in the composition until the next letter (variant B).
    pub space_in_composition: bool,
    /// Bidi marks around the displayed composition (variant A).
    pub bidi_mark: BidiMark,
    /// Keep the bidi marks in committed text (default: strip them at commit).
    pub bidi_keep: bool,
    /// Shell-only switches (variant C), ignored by the engine.
    pub plain_insert: bool,
    pub no_attribute: bool,
    pub no_composition: bool,
}

impl Options {
    /// Text as displayed while composing.
    pub fn display(&self, text: &str) -> String {
        decorate(text, self.bidi_mark)
    }

    /// Text as committed into the document.
    pub fn committed(&self, text: &str) -> String {
        if self.bidi_keep {
            decorate(text, self.bidi_mark)
        } else {
            text.to_string()
        }
    }
}

/// Wrap non-empty text in the chosen bidi controls. Empty stays empty so Cancel still clears.
pub fn decorate(text: &str, mark: BidiMark) -> String {
    if text.is_empty() {
        return String::new();
    }
    let (open, close) = match mark {
        BidiMark::None => return text.to_string(),
        BidiMark::Rlm => ('\u{200F}', '\u{200F}'),
        BidiMark::Rle => ('\u{202B}', '\u{202C}'),
        BidiMark::Rli => ('\u{2067}', '\u{2069}'),
    };
    let mut s = String::with_capacity(text.len() + 6);
    s.push(open);
    s.push_str(text);
    s.push(close);
    s
}

/// What the TSF shell must do in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Key not eaten, no document change.
    PassThrough,
    /// Composition shows this text (start one if none is open).
    Update(String),
    /// Finish the composition with `text`, then insert `append` (outside the composition).
    /// Key eaten.
    Commit { text: String, append: String },
    /// Finish the composition with `text` synchronously, then let the key reach the app.
    CommitAndPass(String),
    /// Remove the composition text and end it. Key eaten.
    Cancel,
    /// Idle: insert this text at the selection. Key eaten.
    Insert(String),
    /// Idle Shift+Space: delete a ZWNJ just before the caret if there is one, else insert one.
    ToggleZwnj,
    /// Finish the composition with `text`, then open a new one showing `preedit` (variant B:
    /// a letter after a held "word + space").
    CommitAndCompose { text: String, preedit: String },
}

#[derive(Default, Debug)]
pub struct Engine {
    latin: Vec<char>,
    /// Variant B: a finished word plus trailing spaces still shown as composition. Invariant:
    /// `held` and `latin` are never both non-empty (a letter commits `held` first).
    held: String,
    /// Latin of the held word, restored when Backspace removes its last trailing space.
    held_latin: Vec<char>,
    held_spaces: usize,
}

const DIGRAPHS: &[(&str, char)] = &[
    ("sh", 'ش'),
    ("kh", 'خ'),
    ("ch", 'چ'),
    ("gh", 'غ'),
    ("zh", 'ژ'),
];

fn single(c: char) -> Option<char> {
    Some(match c {
        'a' | 'e' => 'ا',
        'b' => 'ب',
        'p' => 'پ',
        't' => 'ت',
        'j' => 'ج',
        'h' => 'ه',
        'x' => 'خ',
        'd' => 'د',
        'r' => 'ر',
        'z' => 'ز',
        's' => 'س',
        'f' => 'ف',
        'q' => 'ق',
        'k' => 'ک', // U+06A9, never Arabic kaf U+0643
        'g' => 'گ',
        'l' => 'ل',
        'm' => 'م',
        'n' => 'ن',
        'v' | 'w' | 'o' | 'u' => 'و',
        'y' | 'i' => 'ی', // U+06CC, never Arabic yeh U+064A
        '\'' => 'ع',
        ZWNJ => ZWNJ,
        _ => return None, // lone `c`: rendered as a visible Latin placeholder
    })
}

/// Longest-match render of the whole Latin buffer. Re-run on every key, so digraph fix-ups and
/// Backspace need no special cases.
pub fn render(latin: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < latin.len() {
        if i + 1 < latin.len() {
            let pair: String = latin[i..i + 2].iter().collect();
            if let Some(&(_, ch)) = DIGRAPHS.iter().find(|(k, _)| *k == pair) {
                out.push(ch);
                i += 2;
                continue;
            }
        }
        let c = latin[i];
        out.push(single(c).unwrap_or(c));
        i += 1;
    }
    out
}

/// Fixed US-QWERTY table for printable non-letter keys, independent of the active (Persian)
/// layout. Returns None for keys that are letters or not printable.
pub fn us_printable(vkey: u32, shift: bool) -> Option<char> {
    const SHIFTED_DIGITS: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];
    if (vk::KEY_0..=vk::KEY_9).contains(&vkey) {
        let d = (vkey - vk::KEY_0) as usize;
        return Some(if shift {
            SHIFTED_DIGITS[d]
        } else {
            (b'0' + d as u8) as char
        });
    }
    let (plain, shifted) = match vkey {
        vk::OEM_1 => (';', ':'),
        vk::OEM_PLUS => ('=', '+'),
        vk::OEM_COMMA => (',', '<'),
        vk::OEM_MINUS => ('-', '_'),
        vk::OEM_PERIOD => ('.', '>'),
        vk::OEM_2 => ('/', '?'),
        vk::OEM_3 => ('`', '~'),
        vk::OEM_4 => ('[', '{'),
        vk::OEM_5 => ('\\', '|'),
        vk::OEM_6 => (']', '}'),
        vk::OEM_7 => ('\'', '"'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain })
}

/// Classify a key-down from its virtual key and modifier state.
pub fn classify(vkey: u32, shift: bool, ctrl: bool, alt: bool) -> Key {
    match vkey {
        vk::SHIFT | vk::CONTROL | vk::MENU | vk::CAPITAL | vk::LWIN | vk::RWIN => {
            return Key::Modifier
        }
        v if (vk::LSHIFT..=vk::RMENU).contains(&v) => return Key::Modifier,
        _ => {}
    }
    if ctrl || alt {
        return Key::Other;
    }
    match vkey {
        vk::KEY_A..=vk::KEY_Z => Key::Letter((b'a' + (vkey - vk::KEY_A) as u8) as char),
        vk::OEM_7 if !shift => Key::Letter('\''),
        vk::SPACE if shift => Key::ShiftSpace,
        vk::SPACE => Key::Space,
        vk::BACK => Key::Backspace,
        vk::ESCAPE => Key::Escape,
        vk::RETURN => Key::Enter,
        vk::TAB => Key::Other,
        _ => us_printable(vkey, shift).map_or(Key::Other, Key::Printable),
    }
}

impl Engine {
    pub fn is_composing(&self) -> bool {
        !self.latin.is_empty() || !self.held.is_empty()
    }

    pub fn preedit(&self) -> String {
        let mut s = self.held.clone();
        s.push_str(&render(&self.latin));
        s
    }

    /// OnTestKeyDown answer. Must agree with `on_key(..)` being anything but PassThrough, except
    /// for keys that commit-and-pass: those are reported as eaten so OnKeyDown runs, and
    /// OnKeyDown then returns "not eaten".
    pub fn would_eat(&self, key: Key, opts: &Options) -> bool {
        match key {
            Key::Modifier => false,
            Key::Letter(_) | Key::ShiftSpace => true,
            Key::Printable(_) => self.is_composing() || opts.eat_printable,
            Key::Space | Key::Backspace | Key::Escape | Key::Enter | Key::Other => {
                self.is_composing()
            }
        }
    }

    pub fn on_key(&mut self, key: Key, opts: &Options) -> Action {
        if !self.is_composing() {
            return match key {
                Key::Letter(c) => {
                    self.latin.push(c);
                    Action::Update(self.preedit())
                }
                Key::ShiftSpace => Action::ToggleZwnj,
                Key::Printable(c) if opts.eat_printable => Action::Insert(c.to_string()),
                _ => Action::PassThrough,
            };
        }
        if !self.held.is_empty() {
            if let Some(a) = self.on_key_held(key) {
                return a;
            }
        }
        match key {
            Key::Letter(c) => {
                self.latin.push(c);
                Action::Update(self.preedit())
            }
            Key::ShiftSpace => {
                // Tapping Shift+Space again right after removes the ZWNJ.
                if self.latin.last() == Some(&ZWNJ) {
                    self.latin.pop();
                } else {
                    self.latin.push(ZWNJ);
                }
                if self.latin.is_empty() {
                    Action::Cancel
                } else {
                    Action::Update(self.preedit())
                }
            }
            Key::Backspace => {
                self.latin.pop();
                if self.latin.is_empty() {
                    Action::Cancel
                } else {
                    Action::Update(self.preedit())
                }
            }
            Key::Space if opts.space_in_composition => {
                self.held = format!("{} ", render(&self.latin));
                self.held_latin = std::mem::take(&mut self.latin);
                self.held_spaces = 1;
                Action::Update(self.preedit())
            }
            Key::Space => Action::Commit {
                text: self.take(),
                append: " ".into(),
            },
            Key::Printable(c) => Action::Commit {
                text: self.take(),
                append: c.to_string(),
            },
            Key::Enter if opts.enter_passes => Action::CommitAndPass(self.take()),
            Key::Enter => Action::Commit {
                text: self.take(),
                append: String::new(),
            },
            Key::Escape => {
                self.latin.clear();
                Action::Cancel
            }
            Key::Other => Action::CommitAndPass(self.take()),
            Key::Modifier => Action::PassThrough,
        }
    }

    /// Keys while a "word + space" is held (variant B). None = handle like normal composing
    /// (Enter, punctuation, arrows: they commit everything via `take`).
    fn on_key_held(&mut self, key: Key) -> Option<Action> {
        Some(match key {
            Key::Letter(_) | Key::ShiftSpace => {
                let c = match key {
                    Key::Letter(c) => c,
                    _ => ZWNJ,
                };
                let text = self.take();
                self.latin.push(c);
                Action::CommitAndCompose {
                    text,
                    preedit: self.preedit(),
                }
            }
            Key::Space => {
                self.held.push(' ');
                self.held_spaces += 1;
                Action::Update(self.preedit())
            }
            Key::Backspace => {
                self.held.pop();
                self.held_spaces -= 1;
                if self.held_spaces == 0 {
                    // Back into editing the word itself.
                    self.held.clear();
                    self.latin = std::mem::take(&mut self.held_latin);
                }
                if self.is_composing() {
                    Action::Update(self.preedit())
                } else {
                    Action::Cancel
                }
            }
            // Escape must not throw away a word the user already confirmed with Space.
            Key::Escape => Action::Commit {
                text: self.take(),
                append: String::new(),
            },
            _ => return None,
        })
    }

    /// Focus moved / host asked us to finish: returns the text to commit.
    pub fn take(&mut self) -> String {
        let s = self.preedit();
        self.reset();
        s
    }

    /// The host terminated our composition: drop state, touch nothing.
    pub fn reset(&mut self) {
        self.latin.clear();
        self.held.clear();
        self.held_latin.clear();
        self.held_spaces = 0;
    }

    /// Start over with a single letter (used when the document moved under a direct-insert
    /// preedit, variant C3).
    pub fn restart_with(&mut self, c: char) -> String {
        self.reset();
        self.latin.push(c);
        self.preedit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Vec<String> {
        let mut e = Engine::default();
        let o = Options::default();
        s.chars()
            .map(|c| match e.on_key(Key::Letter(c), &o) {
                Action::Update(p) => p,
                a => panic!("unexpected {a:?}"),
            })
            .collect()
    }

    #[test]
    fn digraph_fixup_is_retroactive() {
        assert_eq!(typed("sh"), ["س", "ش"]);
        assert_eq!(typed("salam").last().unwrap(), "سالام");
        assert_eq!(typed("shab").last().unwrap(), "شاب");
    }

    #[test]
    fn lone_c_is_visible_placeholder() {
        assert_eq!(typed("c"), ["c"]);
        assert_eq!(typed("ch"), ["c", "چ"]);
    }

    #[test]
    fn persian_not_arabic_forms() {
        let s = typed("kyi").pop().unwrap();
        assert!(!s.contains('\u{0643}') && !s.contains('\u{064A}'));
        assert_eq!(s, "کیی");
    }

    #[test]
    fn space_commits_and_appends_space() {
        let mut e = Engine::default();
        let o = Options::default();
        e.on_key(Key::Letter('s'), &o);
        e.on_key(Key::Letter('h'), &o);
        assert_eq!(
            e.on_key(Key::Space, &o),
            Action::Commit {
                text: "ش".into(),
                append: " ".into()
            }
        );
        assert!(!e.is_composing());
        assert_eq!(e.on_key(Key::Space, &o), Action::PassThrough);
    }

    #[test]
    fn backspace_edits_latin_then_cancels() {
        let mut e = Engine::default();
        let o = Options::default();
        e.on_key(Key::Letter('s'), &o);
        e.on_key(Key::Letter('h'), &o);
        assert_eq!(e.on_key(Key::Backspace, &o), Action::Update("س".into()));
        assert_eq!(e.on_key(Key::Backspace, &o), Action::Cancel);
        assert!(!e.would_eat(Key::Backspace, &o));
        assert_eq!(e.on_key(Key::Backspace, &o), Action::PassThrough);
    }

    #[test]
    fn escape_cancels() {
        let mut e = Engine::default();
        let o = Options::default();
        e.on_key(Key::Letter('a'), &o);
        assert_eq!(e.on_key(Key::Escape, &o), Action::Cancel);
        assert!(!e.is_composing());
    }

    #[test]
    fn enter_modes() {
        let mut e = Engine::default();
        e.on_key(Key::Letter('a'), &Options::default());
        assert_eq!(
            e.on_key(Key::Enter, &Options::default()),
            Action::Commit {
                text: "ا".into(),
                append: String::new()
            }
        );
        let pass = Options {
            enter_passes: true,
            ..Default::default()
        };
        e.on_key(Key::Letter('a'), &pass);
        assert_eq!(
            e.on_key(Key::Enter, &pass),
            Action::CommitAndPass("ا".into())
        );
    }

    #[test]
    fn zwnj_toggle() {
        let mut e = Engine::default();
        let o = Options::default();
        assert_eq!(e.on_key(Key::ShiftSpace, &o), Action::ToggleZwnj);
        for c in "mi".chars() {
            e.on_key(Key::Letter(c), &o);
        }
        assert_eq!(
            e.on_key(Key::ShiftSpace, &o),
            Action::Update("می\u{200C}".into())
        );
        assert_eq!(e.on_key(Key::ShiftSpace, &o), Action::Update("می".into()));
        e.on_key(Key::ShiftSpace, &o);
        e.on_key(Key::Letter('x'), &o);
        assert_eq!(e.preedit(), "می\u{200C}خ");
    }

    #[test]
    fn modifiers_never_commit() {
        let mut e = Engine::default();
        let o = Options::default();
        e.on_key(Key::Letter('a'), &o);
        assert!(!e.would_eat(Key::Modifier, &o));
        assert_eq!(e.on_key(Key::Modifier, &o), Action::PassThrough);
        assert!(e.is_composing());
        assert_eq!(e.on_key(Key::Other, &o), Action::CommitAndPass("ا".into()));
    }

    #[test]
    fn printable_idle_and_composing() {
        let mut e = Engine::default();
        let o = Options::default();
        assert!(!e.would_eat(Key::Printable('1'), &o));
        let eat = Options {
            eat_printable: true,
            ..Default::default()
        };
        assert_eq!(
            e.on_key(Key::Printable('1'), &eat),
            Action::Insert("1".into())
        );
        e.on_key(Key::Letter('a'), &o);
        assert_eq!(
            e.on_key(Key::Printable(','), &o),
            Action::Commit {
                text: "ا".into(),
                append: ",".into()
            }
        );
    }

    fn held() -> Options {
        Options {
            space_in_composition: true,
            ..Default::default()
        }
    }

    #[test]
    fn space_in_composition_holds_word_and_space() {
        let mut e = Engine::default();
        let o = held();
        e.on_key(Key::Letter('s'), &o);
        e.on_key(Key::Letter('h'), &o);
        assert_eq!(e.on_key(Key::Space, &o), Action::Update("ش ".into()));
        assert_eq!(e.on_key(Key::Space, &o), Action::Update("ش  ".into()));
        assert_eq!(
            e.on_key(Key::Letter('a'), &o),
            Action::CommitAndCompose {
                text: "ش  ".into(),
                preedit: "ا".into()
            }
        );
        assert_eq!(e.preedit(), "ا");
    }

    #[test]
    fn space_in_composition_backspace_reopens_word() {
        let mut e = Engine::default();
        let o = held();
        e.on_key(Key::Letter('s'), &o);
        e.on_key(Key::Letter('h'), &o);
        e.on_key(Key::Space, &o);
        assert_eq!(e.on_key(Key::Backspace, &o), Action::Update("ش".into()));
        assert_eq!(e.on_key(Key::Backspace, &o), Action::Update("س".into()));
        assert_eq!(e.on_key(Key::Backspace, &o), Action::Cancel);
    }

    #[test]
    fn space_in_composition_escape_and_enter_keep_word() {
        let mut e = Engine::default();
        let o = held();
        e.on_key(Key::Letter('a'), &o);
        e.on_key(Key::Space, &o);
        assert_eq!(
            e.on_key(Key::Escape, &o),
            Action::Commit {
                text: "ا ".into(),
                append: String::new()
            }
        );
        e.on_key(Key::Letter('a'), &o);
        e.on_key(Key::Space, &o);
        assert_eq!(
            e.on_key(Key::Printable(','), &o),
            Action::Commit {
                text: "ا ".into(),
                append: ",".into()
            }
        );
        e.on_key(Key::Letter('a'), &o);
        e.on_key(Key::Space, &o);
        assert_eq!(e.take(), "ا ");
        assert!(!e.is_composing());
    }

    #[test]
    fn bidi_decoration() {
        assert_eq!(decorate("ش", BidiMark::Rlm), "\u{200F}ش\u{200F}");
        assert_eq!(decorate("ش", BidiMark::Rle), "\u{202B}ش\u{202C}");
        assert_eq!(decorate("ش", BidiMark::Rli), "\u{2067}ش\u{2069}");
        assert_eq!(decorate("", BidiMark::Rle), "");
        assert_eq!(decorate("ش", BidiMark::None), "ش");
        assert_eq!(BidiMark::parse(" RLI "), BidiMark::Rli);
        assert_eq!(BidiMark::parse("bogus"), BidiMark::None);
        let strip = Options {
            bidi_mark: BidiMark::Rlm,
            ..Default::default()
        };
        assert_eq!(strip.committed("ش"), "ش");
        let keep = Options {
            bidi_keep: true,
            ..strip
        };
        assert_eq!(keep.committed("ش"), "\u{200F}ش\u{200F}");
    }

    #[test]
    fn classify_keys() {
        assert_eq!(
            classify(vk::KEY_A + 18, false, false, false),
            Key::Letter('s')
        );
        assert_eq!(
            classify(vk::KEY_A + 18, true, false, false),
            Key::Letter('s')
        );
        assert_eq!(classify(vk::KEY_A, false, true, false), Key::Other);
        assert_eq!(classify(vk::SPACE, true, false, false), Key::ShiftSpace);
        assert_eq!(classify(vk::OEM_7, false, false, false), Key::Letter('\''));
        assert_eq!(classify(vk::OEM_7, true, false, false), Key::Printable('"'));
        assert_eq!(
            classify(vk::KEY_0 + 1, true, false, false),
            Key::Printable('!')
        );
        assert_eq!(classify(vk::LSHIFT, false, false, false), Key::Modifier);
        assert_eq!(classify(vk::SHIFT, true, false, false), Key::Modifier);
        assert_eq!(classify(0x25, false, false, false), Key::Other); // VK_LEFT
    }

    #[test]
    fn would_eat_agrees_with_on_key() {
        let keys = [
            Key::Letter('s'),
            Key::Space,
            Key::ShiftSpace,
            Key::Backspace,
            Key::Escape,
            Key::Enter,
            Key::Printable('1'),
            Key::Modifier,
            Key::Other,
        ];
        let all = Options {
            eat_printable: true,
            enter_passes: true,
            space_in_composition: true,
            ..Default::default()
        };
        for opts in [Options::default(), all] {
            // 0 = idle, 1 = composing, 2 = composing then Space (held word in variant B)
            for state in 0..3 {
                let composing = state > 0;
                for k in keys {
                    let mut e = Engine::default();
                    if composing {
                        e.on_key(Key::Letter('a'), &opts);
                    }
                    if state == 2 {
                        e.on_key(Key::Space, &opts);
                    }
                    let eat = e.would_eat(k, &opts);
                    let act = e.on_key(k, &opts);
                    let acts = !matches!(act, Action::PassThrough);
                    assert_eq!(eat, acts, "{k:?} composing={composing} {act:?}");
                }
            }
        }
    }
}
