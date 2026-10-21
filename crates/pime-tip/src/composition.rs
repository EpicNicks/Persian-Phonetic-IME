//! Output -> EditPlan: the pure half of the composition layer (PLAN 2.1/2.2, contracts.md
//! "Session API"). `edit.rs` executes a plan inside a TSF edit session; everything that decides
//! *what* to do lives here so it is unit-testable without TSF.

use std::ops::Range;

use pime_core::config::CandidatePosition;
use pime_core::{keys, CandidateList, CompletionList, KeyEvent, Output, ResolvedConfig, Session};

use crate::presenter::PreferredSide;
use crate::settings::Settings;

/// One document operation. Each carries full text (never a delta) because an async edit session
/// may run after later keys were already processed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocStep {
    /// End the open composition showing `text` (empty erases it). With no composition open,
    /// insert `text` as a start -> set -> end composition (nothing if empty).
    Finish(String),
    /// Idle Shift+Space: if the char before the caret is U+200C delete it, else insert one
    /// (also insert when the read-back fails).
    ToggleZwnj,
    /// Show `text` as the composition (start one if none is open), underlined, with `pending`
    /// (UTF-16 units) drawn with the pending attribute.
    Compose {
        text: String,
        pending: Option<Range<usize>>,
    },
}

/// What the popup shows: the alternates of the last ambiguous letter and/or word completions
/// (docs/autocomplete.md 2.3), in one row index space (alternates first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupContent {
    pub alternates: Option<CandidateList>,
    pub completions: Option<CompletionList>,
    /// The typed text the completions start with (drawn dimmed); empty without completions.
    pub preedit: String,
}

impl PopupContent {
    /// The popup for an `Output`; `None` when neither section has rows.
    pub fn of(out: &Output) -> Option<Self> {
        let alternates = out.candidates.clone().filter(|l| !l.items.is_empty());
        let completions = out.completions.clone().filter(|c| !c.items.is_empty());
        if alternates.is_none() && completions.is_none() {
            return None;
        }
        let preedit = if completions.is_some() {
            out.preedit.clone()
        } else {
            String::new()
        };
        Some(PopupContent {
            alternates,
            completions,
            preedit,
        })
    }

    pub fn alternate_count(&self) -> usize {
        self.alternates.as_ref().map_or(0, |a| a.items.len())
    }

    /// Combined rows: alternates, then completions.
    pub fn rows(&self) -> impl Iterator<Item = &str> {
        let alts = self.alternates.iter().flat_map(|a| a.items.iter());
        let comps = self.completions.iter().flat_map(|c| c.items.iter());
        alts.chain(comps).map(String::as_str)
    }

    /// The selected combined row: a highlighted completion, else the alternates' highlight,
    /// else none.
    pub fn selected_row(&self) -> Option<usize> {
        match self.completions.as_ref().and_then(|c| c.highlighted) {
            Some(j) => Some(self.alternate_count() + j),
            None => self.alternates.as_ref().map(|a| a.highlighted),
        }
    }

    fn same_rows(&self, other: &PopupContent) -> bool {
        self.preedit == other.preedit && self.rows().eq(other.rows())
    }
}

/// What to do with the candidate popup after the document steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandStep {
    Keep,
    Hide,
    /// Show/refresh, anchored on the composition; `edit.rs` decides show vs set_highlight via
    /// [`presenter_call`].
    Show(PopupContent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditPlan {
    pub doc: Vec<DocStep>,
    pub candidates: CandStep,
    pub side: PreferredSide,
    /// Return value for OnKeyDown: false passes the key to the application.
    pub eaten: bool,
    /// The document steps must land before the key reaches the app: request TF_ES_SYNC.
    pub sync: bool,
}

impl EditPlan {
    pub fn needs_session(&self) -> bool {
        !self.doc.is_empty() || self.candidates != CandStep::Keep
    }
}

/// What earlier plans put on screen (as planned: their sessions may still be queued).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Planned {
    pub preedit: String,
    pub pending: Option<Range<usize>>,
    pub candidates: Option<PopupContent>,
}

impl Planned {
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }
}

pub fn side_for(cfg: &ResolvedConfig) -> PreferredSide {
    match cfg.options().candidate_position {
        CandidatePosition::Below => PreferredSide::Below,
        CandidatePosition::Above => PreferredSide::Above,
    }
}

/// Maps one Session `Output` to an edit plan, given what is already planned on screen.
/// Returns the plan and the new planned state.
pub fn build_plan(out: &Output, prev: &Planned, side: PreferredSide) -> (EditPlan, Planned) {
    let mut doc = Vec::new();
    if out.zwnj_toggle {
        // Only produced while idle; `commit` is U+200C.
        if prev.composing() {
            doc.push(DocStep::Finish(String::new()));
        }
        doc.push(DocStep::ToggleZwnj);
    } else if !out.commit.is_empty() || (out.preedit.is_empty() && prev.composing()) {
        doc.push(DocStep::Finish(out.commit.clone()));
    }
    let finished = !doc.is_empty();
    if !out.preedit.is_empty()
        && (finished || out.preedit != prev.preedit || out.preedit_pending != prev.pending)
    {
        doc.push(DocStep::Compose {
            text: out.preedit.clone(),
            pending: out.preedit_pending.clone(),
        });
    }
    let preedit_changed = finished || out.preedit != prev.preedit;
    let popup = PopupContent::of(out);
    let candidates = match &popup {
        Some(p) if preedit_changed || prev.candidates.as_ref() != Some(p) => {
            CandStep::Show(p.clone())
        }
        Some(_) => CandStep::Keep,
        None if prev.candidates.is_some() => CandStep::Hide,
        None => CandStep::Keep,
    };
    let sync = !out.eaten && !doc.is_empty();
    let plan = EditPlan {
        doc,
        candidates,
        side,
        eaten: out.eaten,
        sync,
    };
    let planned = Planned {
        preedit: out.preedit.clone(),
        pending: if out.preedit.is_empty() {
            None
        } else {
            out.preedit_pending.clone()
        },
        candidates: popup,
    };
    (plan, planned)
}

/// OnTestKeyDown's answer for a plan. TSF only calls OnKeyDown when the test said true, so a
/// commit-then-pass key (eaten == false, but its commit must land first) must answer true too;
/// OnKeyDown then returns false and the key still reaches the app.
pub fn wants_key(plan: &EditPlan) -> bool {
    plan.eaten || plan.needs_session()
}

/// The Session's answer to one key, with the user settings applied.
pub fn session_output(s: &mut Session, key: &KeyEvent, settings: &Settings) -> Output {
    let was = s.is_composing();
    apply_settings(s.on_key(key), key, was, settings)
}

/// Dry run of OnKeyDown for OnTestKeyDown (works on a clone; `s` is untouched).
pub fn test_verdict(
    s: &Session,
    key: &KeyEvent,
    settings: &Settings,
    prev: &Planned,
    side: PreferredSide,
) -> bool {
    let out = session_output(&mut s.clone(), key, settings);
    wants_key(&build_plan(&out, prev, side).0)
}

/// The synchronous commit was refused and is queued instead: eat the key so it cannot land
/// before our text (Enter in a chat box, arrows), except chords, which belong to the app.
pub fn eat_when_sync_refused(key: &KeyEvent) -> bool {
    !(key.ctrl || key.alt)
}

/// `Settings::eat_printable == false`: an idle printable key that the Session would only
/// insert (no composition starts) passes through to the keyboard layout instead.
pub fn apply_settings(out: Output, key: &KeyEvent, was_composing: bool, s: &Settings) -> Output {
    if s.eat_printable || was_composing || !passable(&out, key) {
        return out;
    }
    Output::default()
}

fn passable(out: &Output, key: &KeyEvent) -> bool {
    out.eaten
        && out.preedit.is_empty()
        && !out.zwnj_toggle
        && !key.ctrl
        && !key.alt
        && keys::us_char(key.vk, key.shift).is_some()
}

/// Latin ("A") mode, open/close compartment closed: never composes. With `eat_printable`,
/// printable keys insert their US-QWERTY character (the underlying layout is Persian, so a
/// passed key would type Persian-layout letters); everything else passes through.
pub fn latin_output(key: &KeyEvent, caps: bool, s: &Settings) -> Output {
    if !s.eat_printable || key.ctrl || key.alt {
        return Output::default();
    }
    match keys::us_char(key.vk, key.shift) {
        Some(c) => {
            let c = if caps && c.is_ascii_alphabetic() {
                if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }
            } else {
                c
            };
            Output {
                commit: c.to_string(),
                eaten: true,
                ..Default::default()
            }
        }
        None => Output::default(),
    }
}

/// What the popup currently shows (tracked at execution time).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub list: PopupContent,
    pub preedit_units: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenterCall {
    Show,
    /// Move the selection to this combined row (`None`: no row selected).
    Highlight(Option<usize>),
}

/// Same rows over a preedit of the same length means only the highlight moved (Tab/Down/Up):
/// no relayout. Anything else is a fresh `show` at a fresh anchor.
pub fn presenter_call(
    shown: Option<&Shown>,
    list: &PopupContent,
    preedit_units: usize,
) -> PresenterCall {
    let alt_hl = |p: &PopupContent| p.alternates.as_ref().map(|a| a.highlighted);
    let comp_hl = |p: &PopupContent| p.completions.as_ref().and_then(|c| c.highlighted);
    match shown {
        // One row selection can express one moved highlight, not both sections' at once.
        Some(s)
            if s.list.same_rows(list)
                && s.preedit_units == preedit_units
                && (alt_hl(&s.list) == alt_hl(list) || comp_hl(&s.list) == comp_hl(list)) =>
        {
            PresenterCall::Highlight(list.selected_row())
        }
        _ => PresenterCall::Show,
    }
}

pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::keys::*;
    use pime_core::{
        load_config_with_base, CandidateKind, Completer, ContextPolicy, Dictionary, LearnEvent,
        LearnKind, ListCompleter, Session,
    };
    use std::sync::Arc;

    const ZWNJ: &str = "\u{200C}";

    fn session(preset: &str, toml: &str) -> Session {
        let cfg = load_config_with_base(preset, toml).config;
        Session::new(Arc::new(cfg), Arc::new(Dictionary::empty()))
    }

    fn ch(c: char) -> KeyEvent {
        let (vk, shift) = vk_for_char(c).unwrap();
        KeyEvent {
            vk,
            shift,
            ..Default::default()
        }
    }

    /// Simulated document: committed text, the open composition, and TSF semantics of each
    /// step as `edit.rs` implements them. Pass-through keys type nothing (as in the goldens).
    #[derive(Default)]
    struct SimDoc {
        text: String,
        comp: Option<String>,
        readback: bool,
        sync_requests: usize,
        popup: Option<PopupContent>,
    }

    impl SimDoc {
        fn apply(&mut self, p: &EditPlan) {
            if p.sync {
                self.sync_requests += 1;
            }
            for s in &p.doc {
                match s {
                    DocStep::Finish(t) => {
                        self.comp = None;
                        self.text.push_str(t);
                    }
                    DocStep::ToggleZwnj => {
                        assert!(self.comp.is_none());
                        if self.readback && self.text.ends_with(ZWNJ) {
                            self.text.pop();
                        } else {
                            self.text.push_str(ZWNJ);
                        }
                    }
                    DocStep::Compose { text, pending } => {
                        if let Some(r) = pending {
                            assert!(r.end <= utf16_len(text), "pending inside preedit");
                        }
                        self.comp = Some(text.clone());
                    }
                }
            }
            match &p.candidates {
                CandStep::Keep => {}
                CandStep::Hide => self.popup = None,
                CandStep::Show(l) => self.popup = Some(l.clone()),
            }
        }
    }

    struct Rig {
        s: Session,
        planned: Planned,
        doc: SimDoc,
        plans: Vec<EditPlan>,
        settings: Settings,
        /// What the service hands to `Runtime::record`: `Output.learn` of every real event.
        learned: Vec<LearnEvent>,
    }

    impl Rig {
        fn new(s: Session) -> Self {
            Rig {
                s,
                planned: Planned::default(),
                doc: SimDoc::default(),
                plans: Vec::new(),
                settings: Settings::default(),
                learned: Vec::new(),
            }
        }
        fn key(&mut self, k: KeyEvent) -> EditPlan {
            let tested = test_verdict(
                &self.s,
                &k,
                &self.settings,
                &self.planned,
                PreferredSide::Below,
            );
            let out = session_output(&mut self.s, &k, &self.settings);
            let plan = self.event(out);
            // OnKeyDown only runs when OnTestKeyDown said true: every plan that eats the key or
            // touches the document must have been announced.
            assert_eq!(tested, wants_key(&plan), "test verdict for {k:?}: {plan:?}");
            plan
        }
        fn run(&mut self, out: Output) -> EditPlan {
            let (plan, planned) = build_plan(&out, &self.planned, PreferredSide::Below);
            self.planned = planned;
            self.doc.apply(&plan);
            // The planned state always matches the simulated screen.
            assert_eq!(
                self.doc.comp.clone().unwrap_or_default(),
                self.planned.preedit
            );
            assert_eq!(self.doc.popup, self.planned.candidates);
            self.plans.push(plan.clone());
            plan
        }
        /// A real event's Output: applied, then its learn events recorded (service order).
        fn event(&mut self, out: Output) -> EditPlan {
            let learn = out.learn.clone();
            let plan = self.run(out);
            self.learned.extend(learn);
            plan
        }
        fn click(&mut self, row: usize) -> EditPlan {
            let out = self.s.select_candidate(row);
            self.event(out)
        }
        /// The host ended the composition (OnCompositionTerminated): state dropped, no edit.
        fn terminated(&mut self) {
            self.s.on_composition_terminated();
            self.planned = Planned::default();
            self.doc.comp = None;
            self.doc.popup = None;
        }
        fn typ(&mut self, s: &str) {
            for c in s.chars() {
                self.key(ch(c));
            }
        }
    }

    fn rig() -> Rig {
        Rig::new(session("qamooscheh", ""))
    }

    #[test]
    fn sh_composes_then_space_commits_in_one_finish() {
        let mut r = rig();
        let p = r.key(ch('s'));
        assert_eq!(
            p.doc,
            vec![DocStep::Compose {
                text: "س".into(),
                pending: None
            }]
        );
        assert!(matches!(p.candidates, CandStep::Show(_)), "s is ambiguous");
        let p = r.key(ch('h'));
        assert_eq!(
            p.doc,
            vec![DocStep::Compose {
                text: "ش".into(),
                pending: None
            }]
        );
        assert_eq!(p.candidates, CandStep::Hide);
        let p = r.key(KeyEvent::plain(VK_SPACE));
        assert_eq!(p.doc, vec![DocStep::Finish("ش ".into())]);
        assert!(p.eaten && !p.sync);
        assert_eq!(r.doc.text, "ش ");
    }

    #[test]
    fn escape_closes_list_then_cancels() {
        let mut r = rig();
        r.typ("s");
        let p = r.key(KeyEvent::plain(VK_ESCAPE));
        assert!(p.doc.is_empty());
        assert_eq!(p.candidates, CandStep::Hide);
        let p = r.key(KeyEvent::plain(VK_ESCAPE));
        assert_eq!(p.doc, vec![DocStep::Finish(String::new())]);
        assert_eq!(r.doc.text, "");
        assert!(r.doc.comp.is_none());
    }

    #[test]
    fn backspace_to_empty_ends_composition_idle_backspace_passes() {
        let mut r = rig();
        r.typ("b");
        let p = r.key(KeyEvent::plain(VK_BACK));
        assert_eq!(p.doc, vec![DocStep::Finish(String::new())]);
        let p = r.key(KeyEvent::plain(VK_BACK));
        assert!(!p.eaten && !p.needs_session());
    }

    #[test]
    fn arrow_while_composing_commits_synchronously_then_passes() {
        let mut r = rig();
        r.typ("ba");
        let p = r.key(KeyEvent::plain(VK_LEFT));
        assert_eq!(p.doc, vec![DocStep::Finish("با".into())]);
        assert!(!p.eaten && p.sync);
        let p = r.key(KeyEvent::plain(VK_LEFT));
        assert!(!p.eaten && !p.needs_session(), "idle arrows untouched");
    }

    #[test]
    fn enter_modes() {
        let mut r = rig();
        r.typ("b");
        let p = r.key(KeyEvent::plain(VK_RETURN));
        assert!(p.eaten && !p.sync);
        let mut r = Rig::new(session(
            "qamooscheh",
            "[options]\nenter = \"commit-and-pass\"\n",
        ));
        r.typ("b");
        let p = r.key(KeyEvent::plain(VK_RETURN));
        assert_eq!(p.doc, vec![DocStep::Finish("ب".into())]);
        assert!(!p.eaten && p.sync);
    }

    #[test]
    fn tab_moves_highlight_and_digit_picks() {
        let mut r = rig();
        r.typ("s");
        let p = r.key(KeyEvent::plain(VK_TAB));
        let CandStep::Show(popup) = &p.candidates else {
            panic!("{p:?}")
        };
        assert!(
            popup.completions.is_none(),
            "autocomplete is off by default"
        );
        let list = popup.alternates.as_ref().unwrap();
        assert_eq!(list.highlighted, 1);
        assert_eq!(list.kind, CandidateKind::Alternates);
        assert_eq!(
            p.doc,
            vec![DocStep::Compose {
                text: list.items[1].clone(),
                pending: None
            }]
        );
        let p = r.key(ch('3'));
        assert_eq!(p.candidates, CandStep::Hide);
        assert_eq!(r.doc.text, "ث");
    }

    #[test]
    fn modifiers_alone_touch_nothing() {
        let mut r = rig();
        r.typ("s");
        for vk in [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LSHIFT] {
            let p = r.key(KeyEvent::plain(vk));
            assert!(!p.eaten && !p.needs_session());
        }
        assert_eq!(r.doc.comp.as_deref(), Some("س"));
    }

    #[test]
    fn ctrl_chord_commits_and_passes() {
        let mut r = rig();
        r.typ("b");
        let p = r.key(KeyEvent {
            vk: VK_A + 2,
            ctrl: true,
            ..Default::default()
        });
        assert!(p.sync && !p.eaten);
        assert_eq!(r.doc.text, "ب");
    }

    /// Regression: these keys pass through (eaten == false) but must commit first, so the test
    /// verdict has to be true or TSF never calls OnKeyDown and the commit never happens.
    #[test]
    fn commit_then_pass_keys_are_announced_to_tsf() {
        let ctrl_c = KeyEvent {
            ctrl: true,
            ..ch('c')
        };
        let alt_f = KeyEvent {
            alt: true,
            ..ch('f')
        };
        let keys = [
            KeyEvent::plain(VK_LEFT),
            KeyEvent::plain(VK_RIGHT),
            KeyEvent::plain(VK_HOME),
            KeyEvent::plain(VK_END),
            KeyEvent::plain(VK_DELETE),
            ctrl_c,
            alt_f,
        ];
        for k in keys {
            let mut r = rig();
            r.typ("ba");
            assert!(test_verdict(
                &r.s,
                &k,
                &r.settings,
                &r.planned,
                PreferredSide::Below
            ));
            let p = r.key(k);
            assert!(!p.eaten && p.sync && p.needs_session(), "{k:?}: {p:?}");
            assert_eq!(r.doc.text, "با", "{k:?}");
        }
        // Enter in commit-and-pass mode.
        let mut r = Rig::new(session(
            "qamooscheh",
            "[options]\nenter = \"commit-and-pass\"\n",
        ));
        r.typ("b");
        let enter = KeyEvent::plain(VK_RETURN);
        assert!(test_verdict(
            &r.s,
            &enter,
            &r.settings,
            &r.planned,
            PreferredSide::Below
        ));
        assert!(!r.key(enter).eaten);
        // Tab / Down with no alternates for the word: whatever the Session decides, the verdict
        // matches the plan (asserted inside `key`).
        for vk in [VK_TAB, VK_DOWN] {
            let mut r = rig();
            r.typ("b");
            r.key(KeyEvent::plain(vk));
        }
        // Idle pass-through keys still answer false.
        let mut r = rig();
        for k in [KeyEvent::plain(VK_LEFT), ctrl_c, KeyEvent::plain(VK_BACK)] {
            assert!(!test_verdict(
                &r.s,
                &k,
                &r.settings,
                &r.planned,
                PreferredSide::Below
            ));
            r.key(k);
        }
    }

    #[test]
    fn sync_refusal_eats_plain_keys_but_not_chords() {
        assert!(eat_when_sync_refused(&KeyEvent::plain(VK_RETURN)));
        assert!(eat_when_sync_refused(&KeyEvent::plain(VK_LEFT)));
        assert!(eat_when_sync_refused(&KeyEvent::shifted(VK_HOME)));
        let chord = |ctrl, alt| KeyEvent {
            ctrl,
            alt,
            ..ch('s')
        };
        assert!(!eat_when_sync_refused(&chord(true, false)));
        assert!(!eat_when_sync_refused(&chord(false, true)));
        assert!(!eat_when_sync_refused(&chord(true, true)));
    }

    #[test]
    fn lone_c_pending_range_in_utf16_units() {
        let mut r = rig();
        r.typ("bc");
        let last = r.plans.last().unwrap();
        let DocStep::Compose { text, pending } = &last.doc[0] else {
            panic!()
        };
        assert_eq!(text, "بc");
        assert_eq!(pending.clone(), Some(1..2));
    }

    #[test]
    fn idle_zwnj_toggle_with_and_without_readback() {
        let mut r = rig();
        r.doc.readback = true;
        let p = r.key(KeyEvent::shifted(VK_SPACE));
        assert_eq!(p.doc, vec![DocStep::ToggleZwnj]);
        r.key(KeyEvent::shifted(VK_SPACE));
        assert_eq!(r.doc.text, "", "second tap removes it");
        r.doc.readback = false;
        r.key(KeyEvent::shifted(VK_SPACE));
        r.key(KeyEvent::shifted(VK_SPACE));
        assert_eq!(r.doc.text, format!("{ZWNJ}{ZWNJ}"), "failed read inserts");
    }

    #[test]
    fn zwnj_inside_composition_is_preedit() {
        let mut r = rig();
        r.typ("mi");
        r.key(KeyEvent::shifted(VK_SPACE));
        r.typ("x");
        assert!(r.doc.comp.as_deref().unwrap().contains(ZWNJ));
        r.key(KeyEvent::plain(VK_SPACE));
        assert!(r.doc.text.contains(ZWNJ) && r.doc.text.ends_with(' '));
    }

    #[test]
    fn idle_digits_and_punctuation_eaten_by_default() {
        let mut r = rig();
        let p = r.key(ch('1'));
        assert!(p.eaten);
        assert_eq!(r.doc.text, "۱");
        r.key(ch(','));
        assert_eq!(r.doc.text, "۱،");
    }

    #[test]
    fn pass_printable_setting() {
        let mut r = rig();
        r.settings.eat_printable = false;
        for c in ['1', ',', ' '] {
            let p = r.key(ch(c));
            assert!(!p.eaten && !p.needs_session(), "{c:?}");
        }
        // Letters still compose, Shift+Space still toggles ZWNJ.
        assert!(r.key(ch('b')).eaten);
        assert!(
            r.key(ch('1')).eaten,
            "while composing digits commit + insert"
        );
        assert!(r.key(KeyEvent::shifted(VK_SPACE)).eaten);
    }

    #[test]
    fn latin_mode() {
        let s = Settings::default();
        let out = latin_output(&ch('a'), false, &s);
        assert_eq!((out.commit.as_str(), out.eaten), ("a", true));
        assert_eq!(latin_output(&ch('a'), true, &s).commit, "A");
        assert_eq!(latin_output(&ch('A'), true, &s).commit, "a");
        assert_eq!(latin_output(&ch('1'), true, &s).commit, "1");
        assert_eq!(latin_output(&ch('?'), false, &s).commit, "?");
        assert!(!latin_output(&KeyEvent::plain(VK_LEFT), false, &s).eaten);
        let ctrl_c = KeyEvent {
            ctrl: true,
            ..ch('c')
        };
        assert!(!latin_output(&ctrl_c, false, &s).eaten);
        let off = Settings {
            eat_printable: false,
        };
        assert!(!latin_output(&ch('a'), false, &off).eaten);
    }

    #[test]
    fn focus_lost_and_select_candidate() {
        let mut r = rig();
        r.typ("sa");
        let out = r.s.on_focus_lost();
        let p = r.run(out);
        assert_eq!(p.doc, vec![DocStep::Finish("سا".into())]);
        r.typ("z");
        let out = r.s.select_candidate(2);
        r.run(out);
        assert!(r.doc.comp.is_none() && r.doc.popup.is_none());
        let picked: String = r.doc.text.chars().last().unwrap().to_string();
        assert_ne!(picked, "ز");
    }

    #[test]
    fn presenter_call_rules() {
        let list = |items: &[&str], h| PopupContent {
            alternates: Some(CandidateList {
                items: items.iter().map(|s| s.to_string()).collect(),
                highlighted: h,
                kind: CandidateKind::Alternates,
            }),
            ..Default::default()
        };
        let shown = Shown {
            list: list(&["س", "ص", "ث"], 0),
            preedit_units: 1,
        };
        assert_eq!(
            presenter_call(Some(&shown), &list(&["س", "ص", "ث"], 1), 1),
            PresenterCall::Highlight(Some(1))
        );
        assert_eq!(
            presenter_call(Some(&shown), &list(&["س", "ص", "ث"], 0), 2),
            PresenterCall::Show
        );
        assert_eq!(
            presenter_call(Some(&shown), &list(&["ز", "ذ"], 0), 1),
            PresenterCall::Show
        );
        assert_eq!(
            presenter_call(None, &list(&["س"], 0), 1),
            PresenterCall::Show
        );

        // Both sections: rows are combined, completion highlight wins the selection.
        let both = |alt_hl, comp_hl| PopupContent {
            completions: Some(CompletionList {
                items: vec!["سلام".into(), "سلامت".into()],
                highlighted: comp_hl,
                edits: vec![0, 0],
            }),
            preedit: "س".into(),
            ..list(&["س", "ص", "ث"], alt_hl)
        };
        assert_eq!(both(0, Some(1)).selected_row(), Some(4));
        assert_eq!(both(2, None).selected_row(), Some(2));
        assert_eq!(both(0, None).rows().count(), 5);
        let shown = Shown {
            list: both(0, None),
            preedit_units: 1,
        };
        assert_eq!(
            presenter_call(Some(&shown), &both(0, Some(0)), 1),
            PresenterCall::Highlight(Some(3))
        );
        assert_eq!(
            presenter_call(Some(&shown), &both(1, Some(0)), 1),
            PresenterCall::Show,
            "two highlights moved at once"
        );
        assert_eq!(
            presenter_call(Some(&shown), &list(&["س", "ص", "ث"], 0), 1),
            PresenterCall::Show,
            "completion section went away"
        );
        // Completions only, nothing highlighted.
        let only = PopupContent {
            alternates: None,
            ..both(0, None)
        };
        assert_eq!(only.selected_row(), None);
        let shown = Shown {
            list: PopupContent {
                alternates: None,
                ..both(0, Some(1))
            },
            preedit_units: 1,
        };
        assert_eq!(
            presenter_call(Some(&shown), &only, 1),
            PresenterCall::Highlight(None)
        );
        // Empty sections never make a popup.
        let empty = Output {
            candidates: Some(CandidateList {
                items: vec![],
                highlighted: 0,
                kind: CandidateKind::Alternates,
            }),
            completions: Some(CompletionList::default()),
            ..Default::default()
        };
        assert_eq!(PopupContent::of(&empty), None);
    }

    #[test]
    fn side_from_config() {
        let below = load_config_with_base("qamooscheh", "").config;
        assert_eq!(side_for(&below), PreferredSide::Below);
        let above =
            load_config_with_base("qamooscheh", "[options]\ncandidate_position = \"above\"\n")
                .config;
        assert_eq!(side_for(&above), PreferredSide::Above);
    }

    // -- golden replay: every session case in data/golden/session.json, through the plan
    //    builder and the simulated document, must leave exactly the committed text and
    //    preedit the golden file expects.

    fn golden_key(name: &str) -> KeyEvent {
        match name {
            "Space" => KeyEvent::plain(VK_SPACE),
            "ShiftSpace" => KeyEvent::shifted(VK_SPACE),
            "Enter" => KeyEvent::plain(VK_RETURN),
            "Escape" => KeyEvent::plain(VK_ESCAPE),
            "Backspace" => KeyEvent::plain(VK_BACK),
            "Tab" => KeyEvent::plain(VK_TAB),
            "Up" => KeyEvent::plain(VK_UP),
            "Down" => KeyEvent::plain(VK_DOWN),
            "Left" => KeyEvent::plain(VK_LEFT),
            "Right" => KeyEvent::plain(VK_RIGHT),
            "ShiftTab" => KeyEvent::shifted(VK_TAB),
            "Delete" => KeyEvent::plain(VK_DELETE),
            "Home" => KeyEvent::plain(VK_HOME),
            "End" => KeyEvent::plain(VK_END),
            "CapsLock" => KeyEvent::plain(VK_CAPITAL),
            // Same grammar as pime-core's tests/golden.rs.
            _ if name.rsplit_once('+').is_some_and(|(_, b)| !b.is_empty()) => {
                let (mods, base) = name.rsplit_once('+').unwrap();
                let mut k = golden_key(base);
                for m in mods.split('+') {
                    match m {
                        "Ctrl" => k.ctrl = true,
                        "Alt" => k.alt = true,
                        _ => panic!("unknown modifier in {name:?}"),
                    }
                }
                k
            }
            _ if name.starts_with("Numpad") && name.len() > 6 => {
                let d: u32 = name[6..].parse().expect("NumpadN");
                assert!(d <= 9, "{name:?}");
                KeyEvent::plain(VK_NUMPAD0 + d)
            }
            _ => {
                let mut cs = name.chars();
                let c = cs.next().unwrap();
                assert!(cs.next().is_none(), "unknown key {name:?}");
                ch(c)
            }
        }
    }

    /// Replays every session case of a golden file through the plan builder and the simulated
    /// document (with the runner-only steps `Click<row>`, `FocusLost`, `Terminated`); checks
    /// committed text, preedit and (when given) the learn events handed to the recorder.
    fn replay_golden(file: &str) -> usize {
        let path = format!("{}/../../data/golden/{file}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).expect("golden file");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut n = 0;
        let mut fails = Vec::new();
        for case in v["cases"].as_array().unwrap() {
            if case["type"] != "session" {
                continue;
            }
            let (preset, toml) = match &case["config"] {
                serde_json::Value::String(p) => (p.as_str(), ""),
                serde_json::Value::Object(o) => (
                    o.get("preset")
                        .and_then(|p| p.as_str())
                        .unwrap_or("qamooscheh"),
                    o.get("toml").and_then(|t| t.as_str()).unwrap_or(""),
                ),
                _ => ("qamooscheh", ""),
            };
            let mut s = session(preset, toml);
            if let Some(words) = case["completer"]["words"].as_array() {
                let words = words.iter().map(|w| {
                    (
                        w[0].as_str().unwrap().to_string(),
                        w[1].as_f64().unwrap() as f32,
                    )
                });
                s.set_completer(Some(Arc::new(ListCompleter::new(words))));
            }
            if let Some(p) = case.get("policy") {
                s.set_context_policy(ContextPolicy {
                    completions: p["completions"].as_bool().unwrap(),
                    learning: p["learning"].as_bool().unwrap(),
                });
            }
            let mut r = Rig::new(s);
            for k in case["keys"].as_array().unwrap() {
                let k = k.as_str().unwrap();
                if let Some(row) = k.strip_prefix("Click").and_then(|r| r.parse().ok()) {
                    r.click(row);
                } else if k == "FocusLost" {
                    let out = r.s.on_focus_lost();
                    r.event(out);
                } else if k == "Terminated" {
                    r.terminated();
                } else {
                    r.key(golden_key(k));
                }
            }
            n += 1;
            let want = case["committed"].as_str().unwrap();
            let want_pre = case.get("preedit").and_then(|p| p.as_str()).unwrap_or("");
            let got_pre = r.doc.comp.clone().unwrap_or_default();
            if r.doc.text != want || (case.get("preedit").is_some() && got_pre != want_pre) {
                fails.push(format!(
                    "{}: committed {:?} (want {want:?}), preedit {got_pre:?} (want {want_pre:?})",
                    case["name"], r.doc.text
                ));
            }
            if let Some(learn) = case.get("learn").and_then(|l| l.as_array()) {
                let want: Vec<(String, String)> = learn
                    .iter()
                    .map(|e| (e[0].as_str().unwrap().into(), e[1].as_str().unwrap().into()))
                    .collect();
                let got: Vec<(String, String)> = r
                    .learned
                    .iter()
                    .map(|e| (format!("{:?}", e.kind).to_lowercase(), e.word.clone()))
                    .collect();
                if got != want {
                    fails.push(format!("{}: learn {got:?} (want {want:?})", case["name"]));
                }
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
        n
    }

    #[test]
    fn golden_session_cases_replay_through_plans() {
        let n = replay_golden("session.json");
        assert!(n > 10, "only {n} session cases found");
    }

    #[test]
    fn golden_autocomplete_cases_replay_through_plans() {
        let n = replay_golden("autocomplete.json");
        assert!(n > 10, "only {n} autocomplete cases found");
    }

    // -- autocomplete through the plan builder (docs/autocomplete.md 2.5, 5) ----------------

    const AC_ON: &str = "[autocomplete]\nenabled = true\n";

    fn words() -> ListCompleter {
        ListCompleter::new([
            ("سلام".to_string(), 5.0),
            ("سلامت".to_string(), 4.2),
            ("سلطان".to_string(), 4.0),
        ])
    }

    fn ac_rig_with(toml: &str, c: Option<Arc<dyn Completer>>) -> Rig {
        let mut s = session("qamooscheh", toml);
        s.set_completer(c);
        Rig::new(s)
    }

    fn ac_rig() -> Rig {
        ac_rig_with(AC_ON, Some(Arc::new(words())))
    }

    fn comps_shown(r: &Rig) -> Vec<String> {
        r.doc
            .popup
            .as_ref()
            .and_then(|p| p.completions.as_ref())
            .map(|c| c.items.clone())
            .unwrap_or_default()
    }

    fn ev(kind: LearnKind, word: &str) -> LearnEvent {
        LearnEvent {
            word: word.into(),
            kind,
        }
    }

    #[test]
    fn completions_show_in_popup_and_down_enter_accepts() {
        let mut r = ac_rig();
        r.typ("sl");
        assert_eq!(comps_shown(&r), ["سلام", "سلامت", "سلطان"]);
        let popup = r.doc.popup.clone().unwrap();
        assert_eq!(popup.preedit, "سل", "typed part is dimmed");
        assert_eq!(popup.selected_row(), None);
        // Down is eaten (and announced by the test verdict, checked inside `key`).
        let p = r.key(KeyEvent::plain(VK_DOWN));
        assert!(p.eaten && p.doc.is_empty());
        let CandStep::Show(shown) = &p.candidates else {
            panic!("{p:?}")
        };
        let row = shown.selected_row().unwrap();
        assert_eq!(row, shown.alternate_count(), "first completion row");
        let p = r.key(KeyEvent::plain(VK_RETURN));
        assert!(p.eaten && !p.sync);
        assert_eq!(p.candidates, CandStep::Hide);
        assert_eq!(r.doc.text, "سلام");
        assert!(r.doc.comp.is_none());
        assert_eq!(r.learned, [ev(LearnKind::Accepted, "سلام")]);
    }

    #[test]
    fn enter_accepts_even_in_commit_and_pass_mode() {
        let toml = format!("{AC_ON}[options]\nenter = \"commit-and-pass\"\n");
        let mut r = ac_rig_with(&toml, Some(Arc::new(words())));
        r.typ("sl");
        r.key(KeyEvent::plain(VK_DOWN));
        r.key(KeyEvent::plain(VK_DOWN));
        let p = r.key(KeyEvent::plain(VK_RETURN));
        assert!(p.eaten, "accepting never passes Enter to the app");
        assert_eq!(r.doc.text, "سلامت");
        assert_eq!(
            r.learned,
            [
                ev(LearnKind::Skipped, "سلام"),
                ev(LearnKind::Accepted, "سلامت")
            ]
        );
    }

    #[test]
    fn click_on_completion_row_commits_it() {
        let mut r = ac_rig();
        r.typ("sl");
        let n = r.doc.popup.as_ref().unwrap().alternate_count();
        r.click(n + 2);
        assert_eq!(r.doc.text, "سلطان");
        assert!(r.doc.comp.is_none() && r.doc.popup.is_none());
        assert_eq!(r.learned.last(), Some(&ev(LearnKind::Accepted, "سلطان")));
    }

    #[test]
    fn digit_accepts_completion_and_up_wraps() {
        let mut r = ac_rig();
        r.typ("sl");
        let n = r.doc.popup.as_ref().unwrap().alternate_count();
        let p = r.key(KeyEvent::plain(VK_UP));
        let CandStep::Show(shown) = &p.candidates else {
            panic!("{p:?}")
        };
        assert_eq!(shown.selected_row(), Some(n + 2), "Up from none = last");
        r.key(ch(char::from_digit(n as u32 + 2, 10).unwrap()));
        assert_eq!(r.doc.text, "سلامت");
    }

    #[test]
    fn escape_dismisses_and_records_on_episode_end() {
        let mut r = ac_rig();
        r.typ("sl");
        let p = r.key(KeyEvent::plain(VK_ESCAPE));
        assert_eq!(p.candidates, CandStep::Hide);
        assert!(p.eaten && p.doc.is_empty());
        assert!(
            r.learned.is_empty(),
            "events wait for the end of the episode"
        );
        r.key(KeyEvent::plain(VK_SPACE));
        assert_eq!(r.doc.text, "سل ");
        assert_eq!(
            r.learned,
            [
                ev(LearnKind::Dismissed, "سلام"),
                ev(LearnKind::Dismissed, "سلامت"),
                ev(LearnKind::Dismissed, "سلطان"),
            ]
        );
    }

    #[test]
    fn test_verdict_never_produces_recorded_events() {
        let mut r = ac_rig();
        r.typ("sl");
        // The dry run (OnTestKeyDown) of an accepting key works on a clone: nothing is
        // recorded and the session is unchanged.
        for vk in [VK_DOWN, VK_RETURN, VK_ESCAPE, VK_SPACE] {
            assert!(test_verdict(
                &r.s,
                &KeyEvent::plain(vk),
                &r.settings,
                &r.planned,
                PreferredSide::Below
            ));
        }
        assert!(r.learned.is_empty());
        assert_eq!(r.s.snapshot().preedit, "سل");
        assert_eq!(r.s.snapshot().completions.unwrap().highlighted, None);
        r.key(KeyEvent::plain(VK_ESCAPE));
        r.key(KeyEvent::plain(VK_ESCAPE));
        assert_eq!(r.learned.len(), 3, "the real Escapes record once");
    }

    #[test]
    fn password_policy_shows_and_learns_nothing() {
        let mut r = ac_rig();
        r.s.set_context_policy(crate::autocomplete::policy_for_scopes(&[31]));
        r.typ("sl");
        assert!(comps_shown(&r).is_empty());
        r.key(KeyEvent::plain(VK_DOWN));
        r.key(KeyEvent::plain(VK_RETURN));
        assert_eq!(r.doc.text, "سل", "Down/Enter are v1 again");
        assert!(r.learned.is_empty());
    }

    #[test]
    fn autocomplete_off_or_no_completer_is_v1() {
        for (toml, c) in [
            ("", Some(Arc::new(words()) as Arc<dyn Completer>)),
            (AC_ON, None),
        ] {
            let mut r = ac_rig_with(toml, c);
            r.typ("sl");
            assert!(comps_shown(&r).is_empty());
            r.key(KeyEvent::plain(VK_ESCAPE));
            r.key(KeyEvent::plain(VK_SPACE));
            assert!(r.learned.is_empty());
        }
    }

    #[test]
    fn live_dictionary_learns_from_recorded_events() {
        let dir = crate::config::tests::TempDir::new("livedict");
        let tsv = "سلام\t5.0\nسلامت\t4.2\nسلطان\t4.0\n";
        let mut input: &[u8] = tsv.as_bytes();
        let mut inputs: [&mut dyn std::io::BufRead; 1] = [&mut input];
        let (bytes, _) =
            pime_dict::build_from_tsv(&mut inputs, &pime_dict::BuildOptions::default()).unwrap();
        std::fs::write(dir.path().join("dict.bin"), bytes).unwrap();
        let rt = pime_dict::Runtime::start(
            pime_dict::RuntimePaths {
                dict: dir.path().join("dict.bin"),
                history: dir.path().join("history.bin"),
            },
            pime_dict::RuntimeOptions {
                persist: false,
                ..Default::default()
            },
        );
        assert_eq!(
            rt.wait_loaded(std::time::Duration::from_secs(10)),
            pime_dict::Status::Ready { words: 3 }
        );
        let mut r = ac_rig_with(AC_ON, Some(rt.completer()));
        r.typ("sl");
        assert_eq!(comps_shown(&r)[0], "سلام");
        r.key(KeyEvent::plain(VK_DOWN));
        r.key(KeyEvent::plain(VK_DOWN));
        r.key(KeyEvent::plain(VK_RETURN));
        assert_eq!(r.doc.text, "سلامت");
        crate::autocomplete::record(&rt, &r.learned);
        let mut r = ac_rig_with(AC_ON, Some(rt.completer()));
        r.typ("sl");
        assert_eq!(comps_shown(&r)[0], "سلامت", "accepted word now ranks first");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "nothing persisted"
        );
        rt.shutdown();
    }

    #[test]
    fn typo_prefix_offers_fuzzy_row_and_learns_only_accepted() {
        let mut r = ac_rig();
        r.typ("slm");
        let shown = comps_shown(&r);
        assert!(shown.contains(&"سلام".to_string()), "{shown:?}");
        let list = r.doc.popup.as_ref().unwrap().completions.clone().unwrap();
        assert!(list.edits.iter().all(|e| *e > 0), "{:?}", list.edits);
        r.key(KeyEvent::plain(VK_DOWN));
        let idx = shown.iter().position(|w| w == "سلام").unwrap();
        for _ in 0..idx {
            r.key(KeyEvent::plain(VK_DOWN));
        }
        r.key(KeyEvent::plain(VK_RETURN));
        assert_eq!(r.doc.text, "سلام");
        assert!(r.doc.comp.is_none());
        assert_eq!(r.learned, [ev(LearnKind::Accepted, "سلام")]);
    }

    #[test]
    fn fuzzy_rows_are_never_skipped_or_dismissed() {
        let mut r = ac_rig();
        r.typ("slm");
        r.key(KeyEvent::plain(VK_ESCAPE));
        r.key(KeyEvent::plain(VK_SPACE));
        assert!(r.learned.is_empty(), "{:?}", r.learned);
    }

    #[test]
    fn config_reload_toggles_fuzzy_and_min_chars() {
        let off = format!(
            "{AC_ON}fuzzy = false
"
        );
        let min4 = format!(
            "{AC_ON}fuzzy_min_chars = 4
"
        );
        let cfg = |t: &str| Arc::new(load_config_with_base("qamooscheh", t).config);
        let mut r = ac_rig();
        r.typ("slm");
        assert!(!comps_shown(&r).is_empty());
        for (toml, expect) in [(off.as_str(), false), (min4.as_str(), false), (AC_ON, true)] {
            let mut s = session("qamooscheh", toml);
            s.set_completer(Some(Arc::new(words())));
            let mut r = Rig::new(s);
            r.typ("slm");
            assert_eq!(!comps_shown(&r).is_empty(), expect, "{toml}");
        }
        // A live reload through set_config takes effect on the next preedit.
        let mut s = session("qamooscheh", AC_ON);
        s.set_completer(Some(Arc::new(words())));
        s.set_config(cfg(&off));
        let mut r = Rig::new(s);
        r.typ("slm");
        assert!(comps_shown(&r).is_empty());
    }

    #[test]
    fn missing_dictionary_gives_no_completions() {
        let dir = crate::config::tests::TempDir::new("nodict");
        let rt = pime_dict::Runtime::start(
            pime_dict::RuntimePaths {
                dict: dir.path().join("dict.bin"),
                history: dir.path().join("history.bin"),
            },
            pime_dict::RuntimeOptions {
                persist: false,
                ..Default::default()
            },
        );
        assert!(matches!(
            rt.wait_loaded(std::time::Duration::from_secs(10)),
            pime_dict::Status::Unavailable(_)
        ));
        let mut r = ac_rig_with(AC_ON, Some(rt.completer()));
        r.typ("sl");
        assert!(comps_shown(&r).is_empty());
        r.key(KeyEvent::plain(VK_SPACE));
        assert_eq!(r.doc.text, "سل ");
        assert!(r.learned.is_empty());
        rt.shutdown();
    }
}
