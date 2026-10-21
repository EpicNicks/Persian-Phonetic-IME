//! The text service object: one COM object implementing every TSF interface we need.
//!
//! Re-entrancy discipline: state lives in `RefCell`s that are never borrowed across a call back
//! into TSF, the language bar or the presenter (a TF_ES_SYNC edit session, and some TSF
//! callbacks, re-enter this object on the same thread). Handlers take what they need, drop the
//! borrow, then call out.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use pime_core::config::ToggleHotkey;
use pime_core::{ContextPolicy, Dictionary, LearnEvent, ResolvedConfig, Session};
use pime_dict::Runtime;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::{VARIANT, VT_EMPTY, VT_I4};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, IEnumTfDisplayAttributeInfo, ITfCategoryMgr, ITfCompartment,
    ITfCompartmentEventSink, ITfCompartmentEventSink_Impl, ITfCompartmentMgr, ITfComposition,
    ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextView,
    ITfDisplayAttributeInfo, ITfDisplayAttributeProvider, ITfDisplayAttributeProvider_Impl,
    ITfDocumentMgr, ITfEditRecord, ITfKeyEventSink, ITfKeystrokeMgr, ITfSource, ITfTextEditSink,
    ITfTextEditSink_Impl, ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl,
    ITfTextInputProcessor_Impl, ITfTextLayoutSink, ITfTextLayoutSink_Impl, ITfThreadFocusSink,
    ITfThreadFocusSink_Impl, ITfThreadMgr, ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl,
    TfLayoutCode, GUID_COMPARTMENT_EMPTYCONTEXT, GUID_COMPARTMENT_KEYBOARD_DISABLED,
    GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, GUID_PROP_ATTRIBUTE, TF_E_SYNCHRONOUS, TF_INVALID_COOKIE,
    TF_LC_DESTROY, TF_MOD_ALT, TF_MOD_CONTROL, TF_MOD_SHIFT, TF_PRESERVEDKEY,
};
use windows_core::{implement, ComObject, IUnknown, IUnknownImpl, Interface, Ref, GUID};

use crate::anchor::Anchor;
use crate::autocomplete;
use crate::candidates::{PopupPresenter, PresenterOptions};
use crate::composition::{
    build_plan, eat_when_sync_refused, latin_output, presenter_call, session_output, side_for,
    test_verdict, wants_key, EditPlan, Planned, PopupContent, PresenterCall, Shown,
};
use crate::config::ConfigHandle;
use crate::display;
use crate::edit::{self, Job, Mode};
use crate::exports::{guard, guard_unit, poisoned, ObjToken};
use crate::keysink::{Decoded, KeySet};
use crate::langbar::LangBar;
use crate::presenter::{CandidatePresenter, PreferredSide, ScreenRect};
use crate::registry::{self, GUID_DISPLAY_ATTR_INPUT, GUID_DISPLAY_ATTR_PENDING};
use crate::settings::Settings;

/// Cookies of sinks advised on one context (the focused document's top context).
struct ContextSinks {
    ctx: ITfContext,
    edit: Option<u32>,
    layout: Option<u32>,
}

/// The popup as last shown (execution-time state, unlike `Planned`).
struct Popup {
    shown: Shown,
    anchor: ScreenRect,
    side: PreferredSide,
    /// GetTextExt had no layout: retry from OnLayoutChange.
    inexact: bool,
}

struct State {
    thread_mgr: Option<ITfThreadMgr>,
    tm_sink_cookie: Option<u32>,
    focus_sink_cookie: Option<u32>,
    openclose: Option<(ITfCompartment, u32)>,
    key_sink_advised: bool,
    ctx_sinks: Option<ContextSinks>,
    session: Option<Session>,
    settings: Settings,
    side: PreferredSide,
    planned: Planned,
    composition: Option<ITfComposition>,
    /// Compositions left open in another context when typing moved on (focus changed before
    /// their commit ran). Kept so their queued Finish still finds them; bounded.
    stale: Vec<ITfComposition>,
    /// Context of the last key we handled: where a composition whose session has not run yet
    /// lives.
    last_ctx: Option<ITfContext>,
    popup: Option<Popup>,
    eaten_down: KeySet,
    reload_started: bool,
    /// `candidate_labels` the presenter was created with.
    labels: bool,
    /// `candidate_size` the presenter was created with.
    size: u32,
    /// The Persian/Latin hotkey currently preserved with TSF.
    hotkey: Option<TF_PRESERVEDKEY>,
    /// Our reference on the process autocomplete runtime (`autocomplete::acquire`), held
    /// while the config has `[autocomplete] enabled`.
    ac: Option<Arc<Runtime>>,
    /// The focused document's top context, whose input scope sets the session's policy.
    policy_ctx: Option<ITfContext>,
}

impl Default for State {
    fn default() -> Self {
        State {
            thread_mgr: None,
            tm_sink_cookie: None,
            focus_sink_cookie: None,
            openclose: None,
            key_sink_advised: false,
            ctx_sinks: None,
            session: None,
            settings: Settings::default(),
            side: PreferredSide::Below,
            planned: Planned::default(),
            composition: None,
            stale: Vec::new(),
            last_ctx: None,
            popup: None,
            eaten_down: KeySet::default(),
            reload_started: false,
            labels: false,
            size: 0,
            hotkey: None,
            ac: None,
            policy_ctx: None,
        }
    }
}

// ITfTextInputProcessorEx's vtable QI also answers ITfTextInputProcessor, so it is not listed.
#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfCompositionSink,
    ITfDisplayAttributeProvider,
    ITfCompartmentEventSink,
    ITfThreadFocusSink,
    ITfTextEditSink,
    ITfTextLayoutSink
)]
pub struct TextService {
    st: RefCell<State>,
    config: RefCell<Option<ConfigHandle>>,
    langbar: RefCell<Option<LangBar>>,
    /// Concrete type (not the pinned trait): the trait cannot show two sections.
    presenter: RefCell<Option<PopupPresenter>>,
    client_id: Cell<u32>,
    input_atom: Cell<u32>,
    pending_atom: Cell<u32>,
    /// Open/close compartment: true = Persian, false = Latin ("A").
    open: Cell<bool>,
    /// Bumped whenever the session is reset behind the document's back (host termination).
    generation: Cell<u32>,
    _tok: ObjToken,
}

impl Default for TextService {
    fn default() -> Self {
        Self::new()
    }
}

impl TextService {
    /// Cheap on purpose: TSF also creates instances just for ITfDisplayAttributeProvider.
    pub fn new() -> Self {
        TextService {
            st: RefCell::new(State::default()),
            config: RefCell::new(None),
            langbar: RefCell::new(None),
            presenter: RefCell::new(None),
            client_id: Cell::new(0),
            input_atom: Cell::new(0),
            pending_atom: Cell::new(0),
            open: Cell::new(true),
            generation: Cell::new(0),
            _tok: ObjToken::new(),
        }
    }
}

impl TextService_Impl {
    pub fn client_id(&self) -> u32 {
        self.client_id.get()
    }

    pub fn generation(&self) -> u32 {
        self.generation.get()
    }

    pub fn atoms(&self) -> (u32, u32) {
        (self.input_atom.get(), self.pending_atom.get())
    }

    pub fn composition(&self) -> Option<ITfComposition> {
        self.st.borrow().composition.clone()
    }

    /// Our composition living in `ctx` (current or stale), if any. An edit session may only
    /// touch compositions of its own context: its edit cookie is meaningless elsewhere.
    pub fn composition_in(&self, ctx: &ITfContext) -> Option<ITfComposition> {
        let all: Vec<ITfComposition> = {
            let st = self.st.borrow();
            st.composition
                .iter()
                .chain(st.stale.iter())
                .cloned()
                .collect()
        };
        all.into_iter().find(|c| composition_context_is(c, ctx))
    }

    /// Makes `c` the current composition. A previous current one in another context becomes
    /// stale and is returned so the caller can queue its end in its own context.
    pub fn adopt_composition(&self, c: &ITfComposition) -> Option<ITfComposition> {
        const MAX_STALE: usize = 4;
        let mut st = self.st.borrow_mut();
        st.stale.retain(|s| !same_object(s, c));
        let old = st.composition.replace(c.clone())?;
        if same_object(&old, c) {
            return None;
        }
        if st.stale.len() >= MAX_STALE {
            st.stale.remove(0); // only forgotten: the host keeps its text
        }
        st.stale.push(old.clone());
        Some(old)
    }

    /// Stops tracking `c` (it is being ended).
    pub fn release_composition(&self, c: &ITfComposition) {
        let mut st = self.st.borrow_mut();
        st.stale.retain(|s| !same_object(s, c));
        if st.composition.as_ref().is_some_and(|o| same_object(o, c)) {
            st.composition = None;
        }
    }

    /// Removes `c` from the stale list; true if it was there.
    pub fn take_stale(&self, c: &ITfComposition) -> bool {
        let mut st = self.st.borrow_mut();
        let before = st.stale.len();
        st.stale.retain(|s| !same_object(s, c));
        st.stale.len() != before
    }

    /// The host refused a composition: forget the word (its text was inserted as plain text)
    /// and drop plans queued against the old state.
    pub fn abandon_session(&self) {
        {
            let mut st = self.st.borrow_mut();
            if let Some(s) = st.session.as_mut() {
                s.on_composition_terminated();
            }
            st.planned = Planned::default();
        }
        self.generation.set(self.generation.get().wrapping_add(1));
        self.hide_candidates();
    }

    fn active(&self) -> bool {
        !poisoned() && self.st.borrow().key_sink_advised
    }

    fn with_presenter(&self, f: impl FnOnce(&mut PopupPresenter)) {
        // Busy = we are inside a presenter call already (re-entrant): skip rather than panic.
        if let Ok(mut g) = self.presenter.try_borrow_mut() {
            if let Some(p) = g.as_mut() {
                f(p);
            }
        }
    }

    // -- candidate popup (called from edit sessions) --------------------------------------

    pub fn hide_candidates(&self) {
        let had = self.st.borrow_mut().popup.take().is_some();
        if had {
            self.with_presenter(|p| p.hide());
        }
    }

    /// Hide and make sure the next key re-shows the list (focus left the thread).
    fn forget_candidates(&self) {
        self.st.borrow_mut().planned.candidates = None;
        self.st.borrow_mut().popup = None;
        self.with_presenter(|p| p.hide());
    }

    pub fn show_candidates(
        &self,
        list: &PopupContent,
        a: Anchor,
        side: PreferredSide,
        units: usize,
    ) {
        let call = {
            let mut st = self.st.borrow_mut();
            let call = presenter_call(st.popup.as_ref().map(|p| &p.shown), list, units);
            st.popup = Some(Popup {
                shown: Shown {
                    list: list.clone(),
                    preedit_units: units,
                },
                anchor: a.rect,
                side,
                inexact: !a.exact,
            });
            call
        };
        self.with_presenter(|p| match call {
            PresenterCall::Show => show_popup(p, list, a.rect, side),
            PresenterCall::Highlight(row) => p.set_selected_row(row),
        });
    }

    pub fn reanchor_candidates(&self, a: Anchor) {
        let redo = {
            let mut st = self.st.borrow_mut();
            match st.popup.as_mut() {
                Some(p) if p.anchor != a.rect => {
                    p.anchor = a.rect;
                    p.inexact = !a.exact;
                    Some((p.shown.list.clone(), p.side))
                }
                Some(p) => {
                    p.inexact = !a.exact;
                    None
                }
                None => None,
            }
        };
        if let Some((list, side)) = redo {
            self.with_presenter(|p| show_popup(p, &list, a.rect, side));
        }
    }

    // -- keys --------------------------------------------------------------------------------

    pub(crate) fn test_key(&self, ctx: &ITfContext, d: &Decoded) -> bool {
        if !self.active() || context_disabled(ctx) {
            return false;
        }
        // True also for keys that commit and then pass through: OnKeyDown must run for them.
        let st = self.st.borrow();
        if !self.open.get() {
            let out = latin_output(&d.ev, d.caps, &st.settings);
            return wants_key(&build_plan(&out, &st.planned, st.side).0);
        }
        let Some(session) = st.session.as_ref() else {
            return false;
        };
        test_verdict(session, &d.ev, &st.settings, &st.planned, st.side)
    }

    pub(crate) fn key_down(&self, ctx: &ITfContext, d: &Decoded) -> windows_core::Result<bool> {
        if !self.active() || context_disabled(ctx) {
            return Ok(false);
        }
        let vk = d.ev.vk as usize;
        let (plan, learn) = {
            let mut st = self.st.borrow_mut();
            let settings = st.settings;
            let mut out = if !self.open.get() {
                latin_output(&d.ev, d.caps, &settings)
            } else {
                let Some(s) = st.session.as_mut() else {
                    return Ok(false);
                };
                session_output(s, &d.ev, &settings)
            };
            let learn = std::mem::take(&mut out.learn);
            let (plan, planned) = build_plan(&out, &st.planned, st.side);
            st.planned = planned;
            st.last_ctx = Some(ctx.clone());
            st.eaten_down.set(vk, plan.eaten);
            (plan, learn)
        };
        let result = self.run_key_plan(ctx, d, plan);
        // Only now, with the Output handed to the document: OnTestKeyDown never gets here.
        if result.is_ok() {
            self.record_learn(&learn);
        }
        result
    }

    fn run_key_plan(
        &self,
        ctx: &ITfContext,
        d: &Decoded,
        plan: EditPlan,
    ) -> windows_core::Result<bool> {
        let vk = d.ev.vk as usize;
        if !plan.needs_session() {
            return Ok(plan.eaten);
        }
        let eaten = plan.eaten;
        if !plan.sync {
            let _queued =
                edit::request(self.to_object(), ctx, Job::Apply(plan), Mode::AsyncDontCare)?;
            return Ok(eaten);
        }
        // Must be in the document before the app sees the key (Enter in a chat box, arrows).
        let fallback = plan.clone();
        match edit::request(self.to_object(), ctx, Job::Apply(plan), Mode::Sync) {
            Ok(hr) if hr != TF_E_SYNCHRONOUS => Ok(eaten),
            _ => {
                // Sync refused (document locked): commit async and eat the key rather than let
                // it land before our text; chords (Ctrl+S, Alt+F) still go to the app.
                let eat = eat_when_sync_refused(&d.ev);
                let _queued = edit::request(
                    self.to_object(),
                    ctx,
                    Job::Apply(fallback),
                    Mode::AsyncDontCare,
                )?;
                self.st.borrow_mut().eaten_down.set(vk, eat);
                Ok(eat)
            }
        }
    }

    pub(crate) fn key_up(&self, vk: usize, consume: bool) -> bool {
        let mut st = self.st.borrow_mut();
        let was = st.eaten_down.get(vk);
        if consume {
            st.eaten_down.set(vk, false);
        }
        was
    }

    // -- non-key events ----------------------------------------------------------------------

    /// The context a plan for the current composition must run in.
    fn composition_context(&self) -> Option<ITfContext> {
        let (comp, last) = {
            let st = self.st.borrow();
            (st.composition.clone(), st.last_ctx.clone())
        };
        comp.and_then(|c| unsafe { c.GetRange().ok()?.GetContext().ok() })
            .or(last)
    }

    /// Builds a plan from a Session call and runs it in the composition's context.
    fn run_session_plan(
        &self,
        mode: Mode,
        f: impl FnOnce(&mut Session) -> pime_core::Output,
    ) -> windows_core::Result<()> {
        let (plan, learn): (Option<EditPlan>, Vec<LearnEvent>) = {
            let mut st = self.st.borrow_mut();
            let side = st.side;
            match st.session.as_mut() {
                Some(s) => {
                    let mut out = f(s);
                    let learn = std::mem::take(&mut out.learn);
                    let (plan, planned) = build_plan(&out, &st.planned, side);
                    st.planned = planned;
                    (Some(plan), learn)
                }
                None => (None, Vec::new()),
            }
        };
        let result = match plan.filter(EditPlan::needs_session) {
            None => Ok(()),
            Some(plan) => match self.composition_context() {
                Some(ctx) => {
                    edit::request(self.to_object(), &ctx, Job::Apply(plan), mode).map(|_| ())
                }
                None => {
                    self.hide_candidates();
                    Ok(())
                }
            },
        };
        if result.is_ok() {
            self.record_learn(&learn);
        }
        result
    }

    /// Hands an episode's learn events to the autocomplete runtime (in memory; key path).
    fn record_learn(&self, learn: &[LearnEvent]) {
        if learn.is_empty() {
            return;
        }
        let rt = self.st.borrow().ac.clone();
        if let Some(rt) = rt {
            autocomplete::record(&rt, learn);
        }
    }

    // -- autocomplete --------------------------------------------------------------------

    /// Takes or drops our runtime reference to match `cfg`, then gives the session the
    /// matching completer. Activation and config reload only (never the key path).
    fn sync_autocomplete(&self, cfg: &ResolvedConfig) {
        let enabled = cfg.options().autocomplete.enabled;
        let held = self.st.borrow().ac.is_some();
        let mut acquired = false;
        if enabled && !held {
            if let Some(rt) = autocomplete::acquire() {
                self.st.borrow_mut().ac = Some(rt);
                acquired = true;
            }
        }
        let dropped = if !enabled && held {
            self.st.borrow_mut().ac.take()
        } else {
            None
        };
        {
            let mut st = self.st.borrow_mut();
            let completer = st.ac.as_ref().map(|rt| rt.completer());
            if let Some(s) = st.session.as_mut() {
                s.set_completer(completer);
            }
        }
        if let Some(rt) = dropped {
            drop(rt);
            autocomplete::release();
        }
        if acquired {
            self.refresh_policy();
        }
    }

    /// Closes the policy of the focused context until its input scope has been read (a read
    /// edit session; only while autocomplete is on).
    fn refresh_policy(&self) {
        let (ctx, on) = {
            let st = self.st.borrow();
            (st.policy_ctx.clone(), st.ac.is_some())
        };
        self.apply_policy(autocomplete::CLOSED);
        if let (Some(ctx), true) = (ctx, on) {
            // A refused or failed read leaves the context closed: no completions, no learning.
            let _ = edit::request(self.to_object(), &ctx, Job::ReadPolicy, Mode::AsyncDontCare);
        }
    }

    fn apply_policy(&self, p: ContextPolicy) {
        if let Some(s) = self.st.borrow_mut().session.as_mut() {
            s.set_context_policy(p);
        }
    }

    /// The input scope of `ctx` was read (inside a read edit session).
    pub fn set_context_policy_for(&self, ctx: &ITfContext, p: ContextPolicy) {
        let current = self.st.borrow().policy_ctx.clone();
        if !current.is_some_and(|c| same_object(&c, ctx)) {
            return; // focus moved on meanwhile
        }
        self.apply_policy(p);
        if self.st.borrow().planned.composing() {
            // Queued: we are inside an edit session of this context.
            let _ = self.run_session_plan(Mode::Async, |s| s.snapshot());
        }
    }

    /// Pushed/popped context: the focused document's top context may have changed.
    fn refocus_policy(&self) {
        let tm = self.st.borrow().thread_mgr.clone();
        let top = tm.and_then(|tm| unsafe { tm.GetFocus().ok()?.GetTop().ok() });
        let current = self.st.borrow().policy_ctx.clone();
        let same = match (&current, &top) {
            (Some(a), Some(b)) => same_object(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.st.borrow_mut().policy_ctx = top;
            self.refresh_policy();
        }
    }

    /// Focus moved / mode switched / caret left the composition: commit what is shown.
    fn commit_pending(&self, mode: Mode) -> windows_core::Result<()> {
        let composing = {
            let st = self.st.borrow();
            st.planned.composing() || st.planned.candidates.is_some()
        };
        if !composing {
            return Ok(());
        }
        self.run_session_plan(mode, |s| s.on_focus_lost())
    }

    fn on_candidate_click(&self, index: usize) {
        if !self.active() {
            return;
        }
        // Always queued: we are inside the presenter's window procedure.
        let _ = self.run_session_plan(Mode::Async, |s| s.select_candidate(index));
    }

    /// Focus changes: pick up an edited config file (never on the key path).
    fn refresh_config(&self) -> windows_core::Result<()> {
        autocomplete::note_status();
        let fresh = {
            let cfg = self.config.borrow();
            match cfg.as_ref() {
                Some(c) if c.reload_if_changed() => Some((c.current(), c.badge_text())),
                _ => None,
            }
        };
        let Some((cfg, badge)) = fresh else {
            return Ok(());
        };
        let labels = cfg.options().candidate_labels;
        let size = cfg.options().candidate_size;
        let hotkey = cfg.options().toggle_hotkey;
        let (tm, relabel) = {
            let mut st = self.st.borrow_mut();
            st.side = side_for(&cfg);
            if let Some(s) = st.session.as_mut() {
                s.set_config(cfg.clone());
            }
            let relabel = st.labels != labels || st.size != size;
            st.labels = labels;
            st.size = size;
            (st.thread_mgr.clone(), relabel)
        };
        self.sync_autocomplete(&cfg);
        if let Some(tm) = tm.as_ref() {
            self.sync_hotkey(tm, hotkey);
        }
        if let (true, Some(tm)) = (relabel, tm) {
            // Labels are a presenter option: recreating it is cheap. Never called from inside
            // the presenter's own click callback (that path only queues an edit session).
            self.forget_candidates();
            self.install_presenter(&tm, labels, size);
        }
        if let Some(lb) = self.langbar.borrow().as_ref() {
            lb.set_error_badge(badge);
        }
        let composing = self.st.borrow().planned.composing();
        if composing {
            self.run_session_plan(Mode::AsyncDontCare, |s| s.snapshot())?;
        }
        Ok(())
    }

    fn read_open(&self) {
        let comp = self.st.borrow().openclose.as_ref().map(|c| c.0.clone());
        if let Some(c) = comp {
            if let Ok(v) = unsafe { c.GetValue() } {
                self.open.set(variant_i32(&v) != 0);
            }
        }
    }

    // -- sink management ---------------------------------------------------------------------

    /// Text-edit + layout sinks follow the focused document's top context (SampleIME pattern).
    fn advise_context(&self, doc: Option<&ITfDocumentMgr>) {
        let old = self.st.borrow_mut().ctx_sinks.take();
        if let Some(old) = old {
            unadvise_context(&old);
        }
        let top = doc.and_then(|d| unsafe { d.GetTop().ok() });
        self.st.borrow_mut().policy_ctx = top.clone();
        self.refresh_policy();
        let Some(ctx) = top else {
            return;
        };
        let Ok(src) = ctx.cast::<ITfSource>() else {
            return;
        };
        let edit_sink: ITfTextEditSink = self.to_interface();
        let layout_sink: ITfTextLayoutSink = self.to_interface();
        let cookie = |r: windows_core::Result<u32>| r.ok().filter(|c| *c != TF_INVALID_COOKIE);
        let edit = cookie(unsafe { src.AdviseSink(&ITfTextEditSink::IID, &edit_sink) });
        let layout = cookie(unsafe { src.AdviseSink(&ITfTextLayoutSink::IID, &layout_sink) });
        self.st.borrow_mut().ctx_sinks = Some(ContextSinks { ctx, edit, layout });
    }

    fn activate(&self, tm: ITfThreadMgr, tid: u32) -> windows_core::Result<()> {
        self.client_id.set(tid);
        let config = ConfigHandle::load();
        let cfg = config.current();
        let badge = config.badge_text();
        *self.config.borrow_mut() = Some(config);
        {
            let mut st = self.st.borrow_mut();
            st.thread_mgr = Some(tm.clone());
            st.settings = Settings::read();
            st.side = side_for(&cfg);
            let mut session = Session::new(cfg.clone(), Arc::new(Dictionary::empty()));
            // Opened by the input-scope read once a context has focus (advise_context).
            session.set_context_policy(autocomplete::CLOSED);
            st.session = Some(session);
        }
        self.sync_autocomplete(&cfg);
        unsafe {
            let src = tm.cast::<ITfSource>()?;
            let sink: ITfThreadMgrEventSink = self.to_interface();
            let cookie = src.AdviseSink(&ITfThreadMgrEventSink::IID, &sink)?;
            if cookie != TF_INVALID_COOKIE {
                self.st.borrow_mut().tm_sink_cookie = Some(cookie);
            }
            let focus: ITfThreadFocusSink = self.to_interface();
            if let Ok(c) = src.AdviseSink(&ITfThreadFocusSink::IID, &focus) {
                if c != TF_INVALID_COOKIE {
                    self.st.borrow_mut().focus_sink_cookie = Some(c);
                }
            }

            let cats: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
            self.input_atom
                .set(cats.RegisterGUID(&GUID_DISPLAY_ATTR_INPUT).unwrap_or(0));
            self.pending_atom
                .set(cats.RegisterGUID(&GUID_DISPLAY_ATTR_PENDING).unwrap_or(0));

            // Open/close: a fresh thread has no value yet; that means Persian (SampleIME sets
            // it at activation). A value the user already chose is kept.
            if let Ok(comp) = tm
                .cast::<ITfCompartmentMgr>()
                .and_then(|m| m.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE))
            {
                if let Ok(v) = comp.GetValue() {
                    if variant_is_empty(&v) {
                        let _ = comp.SetValue(tid, &VARIANT::from(1i32));
                    }
                }
                let sink: ITfCompartmentEventSink = self.to_interface();
                if let Ok(c) = comp
                    .cast::<ITfSource>()
                    .and_then(|s| s.AdviseSink(&ITfCompartmentEventSink::IID, &sink))
                {
                    self.st.borrow_mut().openclose = Some((comp, c));
                }
            }
        }
        self.read_open();

        // Language bar: a failure here must not stop typing. Its callbacks hold a strong
        // reference to us; `teardown` drops the bar, breaking that cycle.
        if let Ok(lb) = LangBar::new(&tm, tid) {
            lb.set_open(self.open.get());
            lb.set_error_badge(badge);
            let me = self.to_object();
            lb.set_on_recheck(move || guard_unit(|| drop(me.refresh_config())));
            *self.langbar.borrow_mut() = Some(lb);
        }
        crate::reload::start();
        self.st.borrow_mut().reload_started = true;

        let (labels, size) = self
            .config
            .borrow()
            .as_ref()
            .map(|c| {
                let o = *c.current().options();
                (o.candidate_labels, o.candidate_size)
            })
            .unwrap_or((false, 0));
        {
            let mut st = self.st.borrow_mut();
            st.labels = labels;
            st.size = size;
        }
        self.install_presenter(&tm, labels, size);

        unsafe {
            let keys: ITfKeyEventSink = self.to_interface();
            tm.cast::<ITfKeystrokeMgr>()?
                .AdviseKeyEventSink(tid, &keys, true)?;
        }
        self.st.borrow_mut().key_sink_advised = true;
        let hotkey = self
            .config
            .borrow()
            .as_ref()
            .map_or(ToggleHotkey::None, |c| c.current().options().toggle_hotkey);
        self.sync_hotkey(&tm, hotkey);
        let focus = unsafe { tm.GetFocus().ok() };
        self.advise_context(focus.as_ref());
        Ok(())
    }

    /// Registers (or replaces / removes) the Persian/Latin hotkey with TSF. Failure only means
    /// no hotkey (another TIP may own the chord); typing is unaffected.
    fn sync_hotkey(&self, tm: &ITfThreadMgr, hk: ToggleHotkey) {
        let want = hk.chord().map(|(vk, ctrl, shift, alt)| TF_PRESERVEDKEY {
            uVKey: vk,
            uModifiers: (if ctrl { TF_MOD_CONTROL } else { 0 })
                | (if shift { TF_MOD_SHIFT } else { 0 })
                | (if alt { TF_MOD_ALT } else { 0 }),
        });
        let old = self.st.borrow().hotkey;
        let same = |a: &Option<TF_PRESERVEDKEY>, b: &Option<TF_PRESERVEDKEY>| {
            a.as_ref().map(|k| (k.uVKey, k.uModifiers))
                == b.as_ref().map(|k| (k.uVKey, k.uModifiers))
        };
        if same(&old, &want) {
            return;
        }
        let Ok(km) = tm.cast::<ITfKeystrokeMgr>() else {
            return;
        };
        let guid = GUID::from_u128(pime_ids::GUID_PRESERVED_KEY_TOGGLE);
        unsafe {
            if let Some(o) = old {
                let _ = km.UnpreserveKey(&guid, &o);
            }
            let ok = want.is_some_and(|w| {
                let desc: Vec<u16> = "Persian / Latin".encode_utf16().collect();
                km.PreserveKey(self.client_id.get(), &guid, &w, &desc)
                    .is_ok()
            });
            self.st.borrow_mut().hotkey = want.filter(|_| ok);
        }
    }

    /// Hotkey pressed: flip the open/close compartment. Its change sink updates the language
    /// bar and commits a pending word when going to Latin.
    pub(crate) fn toggle_open(&self) -> bool {
        let comp = self.st.borrow().openclose.as_ref().map(|c| c.0.clone());
        let Some(c) = comp else {
            return false;
        };
        self.read_open();
        let open = !self.open.get();
        unsafe { c.SetValue(self.client_id.get(), &VARIANT::from(i32::from(open))) }.is_ok()
    }

    /// Creates the presenter (also published as ITfCandidateListUIElement for UI-less hosts:
    /// we register TIPCAP_UIELEMENTENABLED) and replaces any previous one.
    fn install_presenter(&self, tm: &ITfThreadMgr, labels: bool, size: u32) {
        let mut presenter = PopupPresenter::new(PresenterOptions {
            candidate_labels: labels,
            size_percent: size,
            thread_mgr: Some(tm.clone()),
            ..Default::default()
        });
        let me = self.to_object();
        presenter.set_on_click(Box::new(move |i| guard_unit(|| me.on_candidate_click(i))));
        let old = self
            .presenter
            .try_borrow_mut()
            .ok()
            .map(|mut p| p.replace(presenter));
        if let Some(Some(mut old)) = old {
            old.hide();
        }
    }

    fn teardown(&self) {
        // The presenter's click closure holds a strong reference to us: dropping it here
        // breaks that cycle.
        let presenter = self
            .presenter
            .try_borrow_mut()
            .ok()
            .and_then(|mut p| p.take());
        if let Some(mut p) = presenter {
            p.hide();
            drop(p);
        }
        let lb = self
            .langbar
            .try_borrow_mut()
            .ok()
            .and_then(|mut l| l.take());
        if let Some(lb) = lb {
            lb.shutdown();
        }
        if std::mem::take(&mut self.st.borrow_mut().reload_started) {
            crate::reload::stop();
        }
        let (tm, tm_cookie, focus_cookie, openclose, keys, ctx_sinks, hotkey, ac) = {
            let mut st = self.st.borrow_mut();
            st.session = None;
            st.policy_ctx = None;
            st.composition = None;
            st.stale.clear();
            st.planned = Planned::default();
            st.popup = None;
            st.last_ctx = None;
            (
                st.thread_mgr.take(),
                st.tm_sink_cookie.take(),
                st.focus_sink_cookie.take(),
                st.openclose.take(),
                std::mem::take(&mut st.key_sink_advised),
                st.ctx_sinks.take(),
                st.hotkey.take(),
                st.ac.take(),
            )
        };
        if let Some(rt) = ac {
            // The session (and its completer) is gone; the last release saves and shuts down.
            drop(rt);
            autocomplete::release();
        }
        if let Some(cs) = ctx_sinks {
            unadvise_context(&cs);
        }
        unsafe {
            if let Some((comp, cookie)) = openclose {
                if let Ok(src) = comp.cast::<ITfSource>() {
                    let _ = src.UnadviseSink(cookie);
                }
            }
            if let Some(tm) = tm {
                if keys {
                    if let Ok(k) = tm.cast::<ITfKeystrokeMgr>() {
                        if let Some(h) = hotkey {
                            let guid = GUID::from_u128(pime_ids::GUID_PRESERVED_KEY_TOGGLE);
                            let _ = k.UnpreserveKey(&guid, &h);
                        }
                        let _ = k.UnadviseKeyEventSink(self.client_id.get());
                    }
                }
                if let Ok(src) = tm.cast::<ITfSource>() {
                    for c in [tm_cookie, focus_cookie].into_iter().flatten() {
                        let _ = src.UnadviseSink(c);
                    }
                }
            }
        }
        *self.config.borrow_mut() = None;
        self.client_id.set(0);
        self.input_atom.set(0);
        self.pending_atom.set(0);
    }
}

/// Both sections in one popup (alternates, then completions below a divider).
fn show_popup(p: &mut PopupPresenter, c: &PopupContent, anchor: ScreenRect, side: PreferredSide) {
    p.show_full(
        c.alternates.as_ref(),
        c.completions.as_ref(),
        &c.preedit,
        anchor,
        side,
    );
}

fn unadvise_context(cs: &ContextSinks) {
    if let Ok(src) = cs.ctx.cast::<ITfSource>() {
        for c in [cs.edit, cs.layout].into_iter().flatten() {
            let _ = unsafe { src.UnadviseSink(c) };
        }
    }
}

/// Password fields / read-only contexts (PLAN 2.4). Checked on the CONTEXT, where the docs put
/// these compartments (SampleIME reads the thread manager's, see docs/spike.md 5).
fn context_disabled(ctx: &ITfContext) -> bool {
    let Ok(mgr) = ctx.cast::<ITfCompartmentMgr>() else {
        return false;
    };
    [
        GUID_COMPARTMENT_KEYBOARD_DISABLED,
        GUID_COMPARTMENT_EMPTYCONTEXT,
    ]
    .iter()
    .any(|g| unsafe {
        mgr.GetCompartment(g)
            .and_then(|c| c.GetValue())
            .map(|v| variant_i32(&v) != 0)
            .unwrap_or(false)
    })
}

/// Read a VT_I4 without VariantToInt32 (which would add a propsys.dll import to every host).
fn variant_i32(v: &VARIANT) -> i32 {
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        if inner.vt == VT_I4 {
            inner.Anonymous.lVal
        } else {
            0
        }
    }
}

fn variant_is_empty(v: &VARIANT) -> bool {
    unsafe { v.Anonymous.Anonymous.vt == VT_EMPTY }
}

/// COM identity (IUnknown pointers), the only valid way to compare two interface pointers.
pub(crate) fn same_object<A: Interface, B: Interface>(a: &A, b: &B) -> bool {
    match (a.cast::<IUnknown>(), b.cast::<IUnknown>()) {
        (Ok(x), Ok(y)) => x.as_raw() == y.as_raw(),
        _ => false,
    }
}

/// False also when the composition's range or context cannot be read (treated as foreign).
fn composition_context_is(c: &ITfComposition, ctx: &ITfContext) -> bool {
    unsafe { c.GetRange().and_then(|r| r.GetContext()) }.is_ok_and(|x| same_object(&x, ctx))
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> windows_core::Result<()> {
        guard(|| ITfTextInputProcessorEx_Impl::ActivateEx(self, ptim, tid, 0))
    }

    fn Deactivate(&self) -> windows_core::Result<()> {
        guard(|| {
            // Finish any open preedit synchronously while we still have a client id.
            if self.active() {
                let _ = self.commit_pending(Mode::Sync);
            }
            self.teardown();
            Ok(())
        })
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(
        &self,
        ptim: Ref<ITfThreadMgr>,
        tid: u32,
        _flags: u32,
    ) -> windows_core::Result<()> {
        guard(|| {
            // Kill switch: activate successfully but advise nothing, so we never see a key.
            if registry::kill_switch() || poisoned() {
                return Ok(());
            }
            let tm = ptim.ok()?.clone();
            let result = self.activate(tm, tid);
            if result.is_err() {
                self.teardown();
            }
            result
        })
    }
}

impl ITfThreadMgrEventSink_Impl for TextService_Impl {
    fn OnInitDocumentMgr(&self, _d: Ref<ITfDocumentMgr>) -> windows_core::Result<()> {
        guard(|| Ok(()))
    }
    fn OnUninitDocumentMgr(&self, _d: Ref<ITfDocumentMgr>) -> windows_core::Result<()> {
        guard(|| Ok(()))
    }
    fn OnSetFocus(
        &self,
        focus: Ref<ITfDocumentMgr>,
        _prev: Ref<ITfDocumentMgr>,
    ) -> windows_core::Result<()> {
        guard(|| {
            if !self.active() {
                return Ok(());
            }
            // Commit into the document the composition belongs to (PLAN 2.1).
            let _ = self.commit_pending(Mode::AsyncDontCare);
            self.forget_candidates();
            self.advise_context(focus.as_ref());
            self.refresh_config()
        })
    }
    fn OnPushContext(&self, _c: Ref<ITfContext>) -> windows_core::Result<()> {
        guard(|| {
            if self.active() {
                self.refocus_policy();
            }
            Ok(())
        })
    }
    fn OnPopContext(&self, _c: Ref<ITfContext>) -> windows_core::Result<()> {
        guard(|| {
            if self.active() {
                self.refocus_policy();
            }
            Ok(())
        })
    }
}

impl ITfThreadFocusSink_Impl for TextService_Impl {
    fn OnSetThreadFocus(&self) -> windows_core::Result<()> {
        guard(|| {
            if self.active() {
                self.refresh_config()?;
            }
            Ok(())
        })
    }
    fn OnKillThreadFocus(&self) -> windows_core::Result<()> {
        guard(|| {
            // The popup is topmost: never leave it over another app.
            self.forget_candidates();
            Ok(())
        })
    }
}

impl ITfCompartmentEventSink_Impl for TextService_Impl {
    fn OnChange(&self, rguid: *const GUID) -> windows_core::Result<()> {
        guard(|| {
            if rguid.is_null() || unsafe { *rguid } != GUID_COMPARTMENT_KEYBOARD_OPENCLOSE {
                return Ok(());
            }
            let was = self.open.get();
            self.read_open();
            let open = self.open.get();
            if let Some(lb) = self.langbar.borrow().as_ref() {
                lb.set_open(open);
            }
            if was && !open && self.active() {
                let _ = self.commit_pending(Mode::AsyncDontCare);
            }
            Ok(())
        })
    }
}

impl ITfTextEditSink_Impl for TextService_Impl {
    fn OnEndEdit(
        &self,
        pic: Ref<ITfContext>,
        ec: u32,
        record: Ref<ITfEditRecord>,
    ) -> windows_core::Result<()> {
        guard(|| unsafe {
            let (Some(ctx), Some(record)) = (pic.as_ref(), record.as_ref()) else {
                return Ok(());
            };
            let Some(comp) = self.composition() else {
                return Ok(());
            };
            if !record.GetSelectionStatus().is_ok_and(|b| b.as_bool()) {
                return Ok(());
            }
            let Ok(range) = comp.GetRange() else {
                return Ok(());
            };
            if range.GetContext().is_ok_and(|c| same_object(&c, ctx))
                && !edit::selection_inside(ctx, ec, &range)
            {
                // The caret was moved outside the composition (click, app shortcut): commit.
                let _ = self.commit_pending(Mode::AsyncDontCare);
            }
            Ok(())
        })
    }
}

impl ITfTextLayoutSink_Impl for TextService_Impl {
    fn OnLayoutChange(
        &self,
        pic: Ref<ITfContext>,
        lcode: TfLayoutCode,
        _view: Ref<ITfContextView>,
    ) -> windows_core::Result<()> {
        guard(|| {
            let Some(ctx) = pic.as_ref() else {
                return Ok(());
            };
            let showing = self.st.borrow().popup.is_some();
            if !showing || !self.active() {
                return Ok(());
            }
            if lcode == TF_LC_DESTROY {
                self.hide_candidates();
                return Ok(());
            }
            let _ = edit::request(self.to_object(), ctx, Job::Reanchor, Mode::AsyncDontCare);
            Ok(())
        })
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    fn OnCompositionTerminated(
        &self,
        ec: u32,
        comp: Ref<ITfComposition>,
    ) -> windows_core::Result<()> {
        guard(|| {
            let clear_attr = |c: &ITfComposition| unsafe {
                if let Ok(range) = c.GetRange() {
                    if let Ok(ctx) = range.GetContext() {
                        if let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
                            let _ = prop.Clear(ec, &range);
                        }
                    }
                }
            };
            if let Some(c) = comp.as_ref() {
                if self.take_stale(c) {
                    // Left behind in another context: the session has moved on, keep it.
                    clear_attr(c);
                    return Ok(());
                }
            }
            let ours = self.composition();
            if let (Some(o), Some(c)) = (ours.as_ref(), comp.as_ref()) {
                if !same_object(o, c) {
                    return Ok(()); // a stale composition we already ended
                }
            }
            // The host ended it (click elsewhere, app reset...): keep its text, drop our state.
            {
                let mut st = self.st.borrow_mut();
                if let Some(s) = st.session.as_mut() {
                    s.on_composition_terminated();
                }
                st.composition = None;
                st.planned = Planned::default();
            }
            self.generation.set(self.generation.get().wrapping_add(1));
            self.hide_candidates();
            if let Some(comp) = comp.as_ref() {
                clear_attr(comp);
            }
            Ok(())
        })
    }
}

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> windows_core::Result<IEnumTfDisplayAttributeInfo> {
        guard(|| Ok(display::EnumDisplayAttributeInfo::new(0).into()))
    }

    fn GetDisplayAttributeInfo(
        &self,
        guid: *const GUID,
    ) -> windows_core::Result<ITfDisplayAttributeInfo> {
        guard(|| {
            if guid.is_null() {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            display::info_for(unsafe { &*guid })
        })
    }
}

// Compile-time proof that one object answers every interface TSF will QI for.
#[allow(dead_code)]
fn _interfaces(s: &ComObject<TextService>) {
    let _: ITfTextInputProcessorEx = s.to_interface();
    let _: ITfKeyEventSink = s.to_interface();
    let _: ITfThreadMgrEventSink = s.to_interface();
    let _: ITfCompositionSink = s.to_interface();
    let _: ITfDisplayAttributeProvider = s.to_interface();
    let _: ITfCompartmentEventSink = s.to_interface();
    let _: ITfThreadFocusSink = s.to_interface();
    let _: ITfTextEditSink = s.to_interface();
    let _: ITfTextLayoutSink = s.to_interface();
}
