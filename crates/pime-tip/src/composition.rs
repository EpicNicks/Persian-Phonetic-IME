//! Output -> EditPlan: the pure half of the composition layer (PLAN 2.1/2.2, contracts.md
//! "Session API"). `edit.rs` executes a plan inside a TSF edit session; everything that decides
//! *what* to do lives here so it is unit-testable without TSF.

use std::ops::Range;

use pime_core::config::CandidatePosition;
use pime_core::{keys, CandidateList, KeyEvent, Output, ResolvedConfig};

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

/// What to do with the candidate popup after the document steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandStep {
    Keep,
    Hide,
    /// Show/refresh, anchored on the composition; `edit.rs` decides show vs set_highlight via
    /// [`presenter_call`].
    Show(CandidateList),
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
    pub candidates: Option<CandidateList>,
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
    let candidates = match &out.candidates {
        Some(list) if preedit_changed || prev.candidates.as_ref() != Some(list) => {
            CandStep::Show(list.clone())
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
        candidates: out.candidates.clone(),
    };
    (plan, planned)
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
    pub list: CandidateList,
    pub preedit_units: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenterCall {
    Show,
    Highlight(usize),
}

/// Same items over a preedit of the same length means only the highlight moved (Tab/Down/Up):
/// no relayout. Anything else is a fresh `show` at a fresh anchor.
pub fn presenter_call(
    shown: Option<&Shown>,
    list: &CandidateList,
    preedit_units: usize,
) -> PresenterCall {
    match shown {
        Some(s) if s.list.items == list.items && s.preedit_units == preedit_units => {
            PresenterCall::Highlight(list.highlighted)
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
    use pime_core::{load_config_with_base, CandidateKind, Dictionary, Session};
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
        popup: Option<CandidateList>,
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
    }

    impl Rig {
        fn new(s: Session) -> Self {
            Rig {
                s,
                planned: Planned::default(),
                doc: SimDoc::default(),
                plans: Vec::new(),
                settings: Settings::default(),
            }
        }
        fn key(&mut self, k: KeyEvent) -> EditPlan {
            let was = self.s.is_composing();
            let would = self.s.would_eat(&k);
            let out = apply_settings(self.s.on_key(&k), &k, was, &self.settings);
            if self.settings.eat_printable {
                assert_eq!(would, out.eaten, "would_eat agrees with on_key");
            }
            self.run(out)
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
        let CandStep::Show(list) = &p.candidates else {
            panic!("{p:?}")
        };
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
        let list = |items: &[&str], h| CandidateList {
            items: items.iter().map(|s| s.to_string()).collect(),
            highlighted: h,
            kind: CandidateKind::Alternates,
        };
        let shown = Shown {
            list: list(&["س", "ص", "ث"], 0),
            preedit_units: 1,
        };
        assert_eq!(
            presenter_call(Some(&shown), &list(&["س", "ص", "ث"], 1), 1),
            PresenterCall::Highlight(1)
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
            _ => {
                let mut cs = name.chars();
                let c = cs.next().unwrap();
                assert!(cs.next().is_none(), "unknown key {name:?}");
                ch(c)
            }
        }
    }

    #[test]
    fn golden_session_cases_replay_through_plans() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/golden/session.json"
        );
        let text = std::fs::read_to_string(path).expect("golden session.json");
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
            let mut r = Rig::new(session(preset, toml));
            for k in case["keys"].as_array().unwrap() {
                r.key(golden_key(k.as_str().unwrap()));
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
        }
        assert!(n > 10, "only {n} session cases found");
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }
}
