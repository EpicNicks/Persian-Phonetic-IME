//! The per-context composition state machine (PLAN.md 2.2, 5.1).
//!
//! The host feeds key events in and applies each [`Output`]: first insert
//! `commit` (ending any composition), then show `preedit` as the new
//! composition (empty = no composition), then, if `eaten` is false, let the key
//! reach the application.

use crate::config::{EnterMode, ResolvedConfig, ShiftLetters, ZWNJ};
use crate::keys::*;
use crate::render::{render_chars, ChoiceOverride, Rendered, TokenKind};
use std::ops::Range;
use std::sync::Arc;

/// A key press, as virtual-key code plus modifier state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyEvent {
    /// Windows virtual-key code (see [`crate::keys`]).
    pub vk: u32,
    /// Shift held.
    pub shift: bool,
    /// Ctrl held.
    pub ctrl: bool,
    /// Alt held.
    pub alt: bool,
}

impl KeyEvent {
    /// A key with no modifiers.
    pub fn plain(vk: u32) -> Self {
        KeyEvent {
            vk,
            ..Default::default()
        }
    }

    /// A key with Shift held.
    pub fn shifted(vk: u32) -> Self {
        KeyEvent {
            vk,
            shift: true,
            ..Default::default()
        }
    }
}

/// What the host must do after a key (or other event).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// Text to commit now, before anything else (may be empty). Committing
    /// always ends the current composition.
    pub commit: String,
    /// The composition to show after committing ("" = no composition).
    pub preedit: String,
    /// UTF-16 code-unit range inside `preedit` to draw with the "pending"
    /// attribute: the trailing unmapped Latin letters (a lone `c`).
    pub preedit_pending: Option<Range<usize>>,
    /// The candidate list to show, or `None` to hide it.
    pub candidates: Option<CandidateList>,
    /// True if the key was handled; false means pass it to the application
    /// (after committing `commit`, which then needs a synchronous edit).
    pub eaten: bool,
    /// Set only for Shift+Space with no composition (`commit` is U+200C). If
    /// the host can read the character before the insertion point and it is
    /// already U+200C, it should delete that character and insert nothing
    /// (tapping again removes the ZWNJ). If the read fails, insert `commit`.
    pub zwnj_toggle: bool,
}

/// A candidate list for the candidate window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateList {
    /// Candidates in display order; item `i` is selected with key `i + 1`.
    pub items: Vec<String>,
    /// Index of the highlighted item (what the preedit currently shows).
    pub highlighted: usize,
    /// What the items are.
    pub kind: CandidateKind,
}

/// What a candidate list contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// Alternates of the last ambiguous letter (v1).
    Alternates,
    /// Word completions (phase 2; unused in v1).
    Completions,
}

/// Placeholder for the phase-2 dictionary (PLAN.md 2.3). v1 ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dictionary;

impl Dictionary {
    /// The empty dictionary (v1 behaviour).
    pub fn empty() -> Self {
        Dictionary
    }
}

/// Composition state for one input context.
#[derive(Debug, Clone)]
pub struct Session {
    cfg: Arc<ResolvedConfig>,
    /// Lowercase Latin key characters and U+200C.
    buf: Vec<char>,
    overrides: Vec<ChoiceOverride>,
    /// Escape closed the list; it reopens on the next buffer change or Tab/Down.
    list_closed: bool,
}

impl Session {
    /// A new idle session.
    pub fn new(cfg: Arc<ResolvedConfig>, dict: Arc<Dictionary>) -> Self {
        let _ = dict;
        Session {
            cfg,
            buf: Vec::new(),
            overrides: Vec::new(),
            list_closed: false,
        }
    }

    /// Whether [`Session::on_key`] would eat this key (for `OnTestKeyDown`).
    /// Has no side effects.
    pub fn would_eat(&self, k: &KeyEvent) -> bool {
        self.clone().on_key(k).eaten
    }

    /// True while a composition is open.
    pub fn is_composing(&self) -> bool {
        !self.buf.is_empty()
    }

    /// The current composition, with nothing to commit (for redrawing, e.g.
    /// after [`Session::set_config`]).
    pub fn snapshot(&self) -> Output {
        self.state(false)
    }

    /// Handles one key press.
    pub fn on_key(&mut self, k: &KeyEvent) -> Output {
        if is_modifier(k.vk) {
            return self.state(false);
        }
        let composing = self.is_composing();
        if k.ctrl || k.alt || matches!(k.vk, VK_LWIN | VK_RWIN) {
            return self.commit_then(String::new(), false);
        }
        match k.vk {
            VK_SPACE if k.shift => self.zwnj(),
            VK_SPACE => self.commit_then(" ".into(), true),
            VK_BACK if composing => {
                self.buf.pop();
                self.after_edit();
                self.state(true)
            }
            VK_RETURN if composing => {
                let pass = self.cfg.options().enter == EnterMode::CommitAndPass;
                self.commit_then(String::new(), !pass)
            }
            VK_ESCAPE if composing => {
                if self.list_visible() {
                    self.list_closed = true;
                    self.state(true)
                } else {
                    self.reset();
                    Output {
                        eaten: true,
                        ..Default::default()
                    }
                }
            }
            VK_TAB | VK_DOWN | VK_UP if composing => {
                let up = k.vk == VK_UP || (k.vk == VK_TAB && k.shift);
                if self.rendered().last_alternates(&self.cfg).is_none() {
                    self.commit_then(String::new(), false)
                } else if self.list_closed {
                    self.list_closed = false;
                    self.state(true)
                } else {
                    self.move_highlight(up);
                    self.state(true)
                }
            }
            _ => match us_char(k.vk, k.shift) {
                Some(c) => self.printable(c, k.shift),
                None => self.commit_then(String::new(), false),
            },
        }
    }

    /// Picks candidate `index` of the open list and commits (mouse click).
    /// Ignored (returns the unchanged state) if there is no such candidate.
    pub fn select_candidate(&mut self, index: usize) -> Output {
        if !self.choose(index) {
            return self.state(false);
        }
        self.commit_then(String::new(), true)
    }

    /// Focus moved away: commit the composition.
    pub fn on_focus_lost(&mut self) -> Output {
        self.commit_then(String::new(), false)
    }

    /// The host ended the composition itself: forget it without editing.
    pub fn on_composition_terminated(&mut self) {
        self.reset();
    }

    /// Swaps the configuration; an open composition is kept and re-rendered
    /// with the new map (call [`Session::snapshot`] to redraw it).
    pub fn set_config(&mut self, cfg: Arc<ResolvedConfig>) {
        self.cfg = cfg;
        // Drop choices the new map cannot honour, so they do not resurface later.
        let cfg = &self.cfg;
        self.overrides
            .retain(|o| cfg.candidates(&o.key).is_some_and(|l| o.index < l.len()));
        self.after_edit();
    }

    // -- internals --------------------------------------------------------

    fn rendered(&self) -> Rendered {
        render_chars(&self.buf, &self.cfg, &self.overrides)
    }

    fn list_visible(&self) -> bool {
        !self.list_closed && self.rendered().last_alternates(&self.cfg).is_some()
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.overrides.clear();
        self.list_closed = false;
    }

    fn after_edit(&mut self) {
        let len = self.buf.len();
        self.overrides.retain(|o| o.start < len);
        self.list_closed = false;
    }

    fn state(&self, eaten: bool) -> Output {
        let r = self.rendered();
        let candidates = match r.last_alternates(&self.cfg) {
            Some((tok, list)) if !self.list_closed => Some(CandidateList {
                items: list.to_vec(),
                highlighted: tok.choice,
                kind: CandidateKind::Alternates,
            }),
            _ => None,
        };
        Output {
            commit: String::new(),
            preedit: r.text,
            preedit_pending: r.pending,
            candidates,
            eaten,
            zwnj_toggle: false,
        }
    }

    /// Commits the composition (if any) followed by `then`.
    fn commit_then(&mut self, then: String, eaten: bool) -> Output {
        let mut commit = if self.is_composing() {
            self.rendered().text
        } else {
            String::new()
        };
        commit.push_str(&then);
        self.reset();
        Output {
            commit,
            eaten,
            ..Default::default()
        }
    }

    fn set_override(&mut self, start: usize, key: String, index: usize) {
        self.overrides.retain(|o| o.start != start);
        self.overrides.push(ChoiceOverride { start, key, index });
    }

    /// Sets the last ambiguous token's choice; false if `index` is out of range.
    fn choose(&mut self, index: usize) -> bool {
        let r = self.rendered();
        let Some((tok, list)) = r.last_alternates(&self.cfg) else {
            return false;
        };
        if index >= list.len() {
            return false;
        }
        let (start, key) = (tok.start, tok.key.clone());
        self.set_override(start, key, index);
        true
    }

    fn move_highlight(&mut self, up: bool) {
        let r = self.rendered();
        if let Some((tok, list)) = r.last_alternates(&self.cfg) {
            let n = list.len();
            let next = if up {
                (tok.choice + n - 1) % n
            } else {
                (tok.choice + 1) % n
            };
            let (start, key) = (tok.start, tok.key.clone());
            self.set_override(start, key, next);
        }
    }

    fn zwnj(&mut self) -> Output {
        if !self.is_composing() {
            return Output {
                commit: ZWNJ.to_string(),
                eaten: true,
                zwnj_toggle: true,
                ..Default::default()
            };
        }
        if self.buf.last() == Some(&ZWNJ) {
            self.buf.pop();
        } else {
            self.buf.push(ZWNJ);
        }
        self.after_edit();
        self.state(true)
    }

    /// Whether typing `c` continues or starts a map key.
    fn extends_key(&self, c: char) -> bool {
        let mut s = String::new();
        for tail in 0..=self.buf.len().min(2) {
            let start = self.buf.len() - tail;
            if self.buf[start..].contains(&ZWNJ) {
                break;
            }
            s.clear();
            s.extend(&self.buf[start..]);
            s.push(c);
            if self.cfg.is_key_prefix(&s) {
                return true;
            }
        }
        false
    }

    fn printable(&mut self, c: char, shift: bool) -> Output {
        let letter = c.is_ascii_alphabetic() || c == '\'';
        let lower = c.to_ascii_lowercase();
        if let Some(d) = c.to_digit(10).filter(|d| *d >= 1) {
            if self.cfg.options().number_select
                && self.list_visible()
                && self.choose(d as usize - 1)
            {
                return self.commit_then(String::new(), true);
            }
        }
        if letter || self.extends_key(c) {
            let ch = if letter { lower } else { c };
            self.buf.push(ch);
            self.after_edit();
            if shift
                && c.is_ascii_alphabetic()
                && self.cfg.options().shift_letters == ShiftLetters::Alternate
            {
                let r = self.rendered();
                if let Some(tok) = r.tokens.last().filter(|t| t.kind == TokenKind::Mapped) {
                    let has_second = self.cfg.candidates(&tok.key).is_some_and(|l| l.len() > 1);
                    if has_second {
                        let (start, key) = (tok.start, tok.key.clone());
                        self.set_override(start, key, 1);
                    }
                }
            }
            return self.state(true);
        }
        let inserted = match self.cfg.convert_char(c) {
            Some(s) => s.to_string(),
            None => c.to_string(),
        };
        self.commit_then(inserted, true)
    }
}

#[cfg(test)]
mod tests;
