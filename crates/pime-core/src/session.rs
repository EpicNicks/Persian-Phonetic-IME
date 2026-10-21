//! The per-context composition state machine (PLAN.md 2.2, 5.1), with word
//! completion when a [`Completer`] is set (`docs/autocomplete.md` 2.3-2.6).
//!
//! The host feeds key events in and applies each [`Output`]: first insert
//! `commit` (ending any composition), then show `preedit` as the new
//! composition (empty = no composition), then, if `eaten` is false, let the key
//! reach the application.

use crate::complete::{
    completable, match_key, CompleteCtx, Completer, CompletionList, ContextPolicy, LearnEvent,
    LearnKind,
};
use crate::config::{EnterMode, ResolvedConfig, ShiftLetters, SpaceMode, ZWNJ};
use crate::keys::*;
use crate::render::{render_chars, ChoiceOverride, Rendered, TokenKind};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// Hard cap on the composition length (key characters). A longer run is
/// committed and a fresh composition starts, so the buffer and the per-key
/// re-render stay bounded.
const MAX_PREEDIT: usize = 64;

/// The popup never shows more rows than the number keys can reach.
const MAX_ROWS: usize = 9;

/// Rows of the visible list that count as ignored when the preedit is
/// committed as typed (`docs/autocomplete.md` 2.6).
const IGNORED_ROWS: usize = 3;

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
    /// The completion section of the popup (`None` = none). Its rows continue
    /// the numbering of `candidates` (the alternates).
    pub completions: Option<CompletionList>,
    /// Learning events; non-empty only on the Output that ends an episode.
    pub learn: Vec<LearnEvent>,
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
    /// Unused: completions travel in [`Output::completions`].
    Completions,
}

/// Placeholder for the phase-2 dictionary (PLAN.md 2.3). Ignored; completions
/// come from [`Session::set_completer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dictionary;

impl Dictionary {
    /// The empty dictionary (v1 behaviour).
    pub fn empty() -> Self {
        Dictionary
    }
}

/// Keeps `Session: Debug` with a trait object inside.
#[derive(Clone)]
struct CompleterHandle(Arc<dyn Completer>);

impl fmt::Debug for CompleterHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Completer")
    }
}

/// The last completion query and its (filtered) answer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionCache {
    prefix: String,
    limit: usize,
    ctx: CompleteCtx,
    items: Vec<String>,
    /// Parallel to `items` (0 = exact).
    edits: Vec<u8>,
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
    completer: Option<CompleterHandle>,
    policy: ContextPolicy,
    cache: Option<CompletionCache>,
    /// Highlighted completion row; `None` = the typed text.
    highlight: Option<usize>,
    /// Learn events of the current episode, in order (deduplicated on emit).
    events: Vec<LearnEvent>,
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
            completer: None,
            policy: ContextPolicy::default(),
            cache: None,
            highlight: None,
            events: Vec::new(),
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
            VK_SPACE => {
                if let Some(j) = self.space_accept_row() {
                    return self.accept(j, " ");
                }
                self.commit_typed(" ".into(), true)
            }
            VK_BACK if composing => {
                self.buf.pop();
                self.after_edit();
                self.show(true)
            }
            VK_RETURN if composing => {
                if let Some(j) = self.visible_highlight() {
                    return self.accept(j, "");
                }
                let pass = self.cfg.options().enter == EnterMode::CommitAndPass;
                self.commit_typed(String::new(), !pass)
            }
            VK_ESCAPE if composing => {
                let comps = self.visible_completions();
                if self.list_visible() || !comps.items.is_empty() {
                    for w in exact_rows(&comps) {
                        self.record(w, LearnKind::Dismissed);
                    }
                    self.list_closed = true;
                    self.highlight = None;
                    self.state(true)
                } else {
                    self.reset();
                    Output {
                        eaten: true,
                        learn: self.take_learn(),
                        ..Default::default()
                    }
                }
            }
            VK_TAB | VK_DOWN | VK_UP if composing => {
                let up = k.vk == VK_UP || (k.vk == VK_TAB && k.shift);
                if k.vk != VK_TAB {
                    if let Some(out) = self.move_completion_highlight(up) {
                        return out;
                    }
                }
                if self.rendered().last_alternates(&self.cfg).is_none() {
                    self.commit_then(String::new(), false)
                } else if self.list_closed {
                    self.list_closed = false;
                    self.highlight = None;
                    self.show(true)
                } else {
                    self.move_highlight(up);
                    self.highlight = None;
                    self.show(true)
                }
            }
            _ => match us_char(k.vk, k.shift) {
                Some(c) => self.printable(c, k.shift),
                None => self.commit_then(String::new(), false),
            },
        }
    }

    /// Picks popup row `index` (alternates first, then completions) and
    /// commits (mouse click). Ignored (returns the unchanged state) if there is
    /// no such row.
    pub fn select_candidate(&mut self, index: usize) -> Output {
        let n = self.alternate_count();
        if index >= n {
            let comps = self.visible_completions();
            if index - n < comps.items.len() {
                return self.accept(index - n, "");
            }
            return self.state(false);
        }
        let comps = self.visible_completions();
        if !self.choose(index) {
            return self.state(false);
        }
        let text = self.rendered().text;
        self.record_ignored(&comps, &text);
        self.commit_then(String::new(), true)
    }

    /// Focus moved away: commit the composition.
    pub fn on_focus_lost(&mut self) -> Output {
        self.commit_then(String::new(), false)
    }

    /// The host ended the composition itself: forget it without editing.
    pub fn on_composition_terminated(&mut self) {
        self.reset();
        self.events.clear();
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
        self.refresh_completions();
    }

    /// Sets the word completer (`None` = v1 behaviour). Call
    /// [`Session::snapshot`] to redraw an open composition.
    pub fn set_completer(&mut self, c: Option<Arc<dyn Completer>>) {
        self.completer = c.map(CompleterHandle);
        self.cache = None;
        self.highlight = None;
        self.refresh_completions();
    }

    /// Sets what the focused context allows (completions, learning).
    pub fn set_context_policy(&mut self, p: ContextPolicy) {
        self.policy = p;
        if !self.learning() {
            self.events.clear();
        }
        self.refresh_completions();
    }

    // -- internals --------------------------------------------------------

    fn rendered(&self) -> Rendered {
        render_chars(&self.buf, &self.cfg, &self.overrides)
    }

    /// The alternates section is shown.
    fn list_visible(&self) -> bool {
        !self.list_closed && self.rendered().last_alternates(&self.cfg).is_some()
    }

    /// Rows of the alternates section; completion rows are numbered after them.
    fn alternate_count(&self) -> usize {
        self.rendered()
            .last_alternates(&self.cfg)
            .map_or(0, |(_, l)| l.len())
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.overrides.clear();
        self.list_closed = false;
        self.cache = None;
        self.highlight = None;
    }

    fn after_edit(&mut self) {
        let len = self.buf.len();
        self.overrides.retain(|o| o.start < len);
        self.list_closed = false;
        self.highlight = None;
    }

    fn learning(&self) -> bool {
        self.cfg.options().autocomplete.learning && self.policy.learning
    }

    /// The query limit the current composition calls for, or `None` when no
    /// completion section may be shown (ignoring an Esc-closed popup).
    fn completion_limit(&self, r: &Rendered) -> Option<usize> {
        let ac = &self.cfg.options().autocomplete;
        if !ac.enabled || self.completer.is_none() || !self.policy.completions {
            return None;
        }
        if r.pending.is_some() || !completable(&r.text) {
            return None;
        }
        let letters = r.text.chars().filter(|c| *c != ZWNJ).count();
        if (letters as i64) < ac.predict_min_chars {
            return None;
        }
        let alternates = r.last_alternates(&self.cfg).map_or(0, |(_, l)| l.len());
        let limit = (ac.max_completions as usize).min(MAX_ROWS.saturating_sub(alternates));
        (limit > 0).then_some(limit)
    }

    /// Queries the completer if the composition changed since the last query.
    fn refresh_completions(&mut self) {
        let r = self.rendered();
        let (Some(limit), Some(handle)) = (self.completion_limit(&r), self.completer.as_ref())
        else {
            self.cache = None;
            self.highlight = None;
            return;
        };
        let ac = &self.cfg.options().autocomplete;
        let ctx = CompleteCtx {
            use_history: ac.learning,
            fuzzy: ac.fuzzy,
            fuzzy_min_chars: ac.fuzzy_min_chars as usize,
            hide_flagged: ac.filter_profanity,
        };
        if self
            .cache
            .as_ref()
            .is_some_and(|c| c.prefix == r.text && c.limit == limit && c.ctx == ctx)
        {
            return;
        }
        let found = handle.0.complete(&r.text, limit, &ctx);
        let mut items: Vec<String> = Vec::with_capacity(limit);
        let mut edits: Vec<u8> = Vec::with_capacity(limit);
        for c in found {
            if items.len() >= limit {
                break;
            }
            if acceptable(&r.text, &c.text, c.edits) && !items.contains(&c.text) {
                items.push(c.text);
                edits.push(c.edits);
            }
        }
        self.cache = Some(CompletionCache {
            prefix: r.text,
            limit,
            ctx,
            items,
            edits,
        });
        self.highlight = None;
    }

    /// Completion rows for `r` and their edit counts, ignoring whether Esc
    /// closed the popup.
    fn completion_items(&self, r: &Rendered) -> (&[String], &[u8]) {
        let Some(limit) = self.completion_limit(r) else {
            return (&[], &[]);
        };
        match &self.cache {
            Some(c) if c.prefix == r.text => {
                let k = c.items.len().min(limit);
                (&c.items[..k], &c.edits[..k])
            }
            _ => (&[], &[]),
        }
    }

    fn completions_for(&self, r: &Rendered) -> Option<CompletionList> {
        if self.list_closed {
            return None;
        }
        let (items, edits) = self.completion_items(r);
        (!items.is_empty()).then(|| CompletionList {
            items: items.to_vec(),
            highlighted: self.highlight.filter(|h| *h < items.len()),
            edits: edits.to_vec(),
        })
    }

    /// The completion rows currently shown (empty if none).
    fn visible_completions(&self) -> CompletionList {
        self.completions_for(&self.rendered()).unwrap_or_default()
    }

    fn visible_highlight(&self) -> Option<usize> {
        self.completions_for(&self.rendered())
            .and_then(|c| c.highlighted)
    }

    /// Down/Up while completions are visible or the popup is Esc-closed;
    /// `None` = v1 handling.
    fn move_completion_highlight(&mut self, up: bool) -> Option<Output> {
        let r = self.rendered();
        let k = self.completion_items(&r).0.len();
        if k == 0 {
            return None;
        }
        if self.list_closed {
            self.list_closed = false;
            self.highlight = None;
            return Some(self.show(true));
        }
        self.highlight = match (self.highlight.filter(|h| *h < k), up) {
            (None, false) => Some(0),
            (None, true) => Some(k - 1),
            (Some(h), false) => (h + 1 < k).then_some(h + 1),
            (Some(h), true) => h.checked_sub(1),
        };
        Some(self.state(true))
    }

    /// The completion Space accepts, if any.
    fn space_accept_row(&self) -> Option<usize> {
        let list = self.completions_for(&self.rendered())?;
        list.highlighted
            .or_else(|| (self.cfg.options().space == SpaceMode::TopCandidate).then_some(0))
    }

    fn record(&mut self, word: &str, kind: LearnKind) {
        if self.learning() {
            self.events.push(LearnEvent {
                word: word.to_string(),
                kind,
            });
        }
    }

    /// `Ignored` for the first exact rows of `comps`, except one equal to
    /// `committed`. Fuzzy rows are never penalised.
    fn record_ignored(&mut self, comps: &CompletionList, committed: &str) {
        let committed = match_key(committed);
        for w in exact_rows(comps).take(IGNORED_ROWS) {
            if match_key(w) != committed {
                self.record(w, LearnKind::Ignored);
            }
        }
    }

    /// The episode's events, deduplicated by match key keeping the highest
    /// kind, in order of first appearance.
    fn take_learn(&mut self) -> Vec<LearnEvent> {
        let mut out: Vec<(String, LearnEvent)> = Vec::new();
        for ev in self.events.drain(..) {
            let key = match_key(&ev.word);
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, e)) => {
                    if ev.kind > e.kind {
                        *e = ev;
                    }
                }
                None => out.push((key, ev)),
            }
        }
        out.into_iter().map(|(_, e)| e).collect()
    }

    /// Commits completion row `j` of the visible list, followed by `then`.
    fn accept(&mut self, j: usize, then: &str) -> Output {
        let comps = self.visible_completions();
        let Some(word) = comps.items.get(j).cloned() else {
            return self.state(true);
        };
        // Fuzzy rows above the accepted one are not penalised.
        let above = CompletionList {
            items: comps.items[..j].to_vec(),
            highlighted: None,
            edits: comps.edits[..j].to_vec(),
        };
        for w in exact_rows(&above) {
            self.record(w, LearnKind::Skipped);
        }
        self.record(&word, LearnKind::Accepted);
        let mut commit = word;
        commit.push_str(then);
        self.reset();
        Output {
            commit,
            eaten: true,
            learn: self.take_learn(),
            ..Default::default()
        }
    }

    /// Re-queries completions, then reports the state. Ends the episode if
    /// the composition became empty (Backspace).
    fn show(&mut self, eaten: bool) -> Output {
        self.refresh_completions();
        let mut out = self.state(eaten);
        if self.buf.is_empty() {
            out.learn = self.take_learn();
        }
        out
    }

    fn state(&self, eaten: bool) -> Output {
        let r = self.rendered();
        let completions = self.completions_for(&r);
        let candidates = match r.last_alternates(&self.cfg) {
            Some((tok, list)) if !self.list_closed => Some(CandidateList {
                items: list.to_vec(),
                highlighted: tok.choice,
                kind: CandidateKind::Alternates,
            }),
            _ => None,
        };
        Output {
            preedit: r.text,
            preedit_pending: r.pending,
            candidates,
            eaten,
            completions,
            ..Default::default()
        }
    }

    /// The preedit as `commit_then(then, ..)` would commit it.
    fn committed_text(&self, then: &str) -> String {
        let mut text = self.rendered().text;
        // A ZWNJ ending the word (space, Enter, focus change, navigation) is
        // invisible junk; before a digit or punctuation it is kept.
        if then.is_empty() || then == " " {
            text.truncate(text.trim_end_matches(ZWNJ).len());
        }
        text
    }

    /// Commits the preedit as typed (Space, Enter, digit, punctuation): the
    /// visible completions count as ignored.
    fn commit_typed(&mut self, then: String, eaten: bool) -> Output {
        let comps = self.visible_completions();
        if !comps.items.is_empty() {
            let committed = self.committed_text(&then);
            self.record_ignored(&comps, &committed);
        }
        self.commit_then(then, eaten)
    }

    /// Commits the composition (if any) followed by `then`; ends the episode.
    fn commit_then(&mut self, then: String, eaten: bool) -> Output {
        let mut commit = if self.is_composing() {
            self.committed_text(&then)
        } else {
            String::new()
        };
        commit.push_str(&then);
        self.reset();
        Output {
            commit,
            eaten,
            learn: self.take_learn(),
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
        self.show(true)
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
            let comps = self.visible_completions();
            if self.cfg.options().number_select && (self.list_visible() || !comps.items.is_empty())
            {
                let row = d as usize - 1;
                let n = self.alternate_count();
                if row >= n {
                    // Out of range: eat the digit rather than guess what was meant.
                    return if row - n < comps.items.len() {
                        self.accept(row - n, "")
                    } else {
                        self.state(true)
                    };
                }
                return if self.choose(row) {
                    let text = self.rendered().text;
                    self.record_ignored(&comps, &text);
                    self.commit_then(String::new(), true)
                } else {
                    self.state(true)
                };
            }
        }
        if letter || self.extends_key(c) {
            let ch = if letter { lower } else { c };
            if self.buf.len() >= MAX_PREEDIT {
                let flushed = self.commit_then(String::new(), true);
                let mut out = self.printable(c, shift);
                out.commit.insert_str(0, &flushed.commit);
                let mut learn = flushed.learn;
                learn.append(&mut out.learn);
                out.learn = learn;
                return out;
            }
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
            return self.show(true);
        }
        let inserted = match self.cfg.convert_char(c) {
            Some(s) => s.to_string(),
            None => c.to_string(),
        };
        self.commit_typed(inserted, true)
    }
}

/// The exact (non-fuzzy) rows of `comps`, in order.
fn exact_rows(comps: &CompletionList) -> impl Iterator<Item = &String> {
    comps
        .items
        .iter()
        .zip(&comps.edits)
        .filter(|(_, e)| **e == 0)
        .map(|(w, _)| w)
}

/// Defence in depth over the completer's contract: an exact completion must
/// extend the preedit, and the part it adds must not contain Arabic-only
/// letters (D23). A fuzzy one replaces the preedit, so it must differ from it
/// and contain no Arabic-only letter at all.
fn acceptable(preedit: &str, text: &str, edits: u8) -> bool {
    let added = if edits == 0 {
        match text.strip_prefix(preedit) {
            Some(tail) if !tail.is_empty() => tail,
            _ => return false,
        }
    } else if text.is_empty() || text == preedit {
        return false;
    } else {
        text
    };
    !added
        .chars()
        .any(|c| matches!(c, '\u{064A}' | '\u{0649}' | '\u{0643}' | '\u{0629}'))
}

#[cfg(test)]
mod tests;
