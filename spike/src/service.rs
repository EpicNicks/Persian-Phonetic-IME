//! The text service object: one COM object implementing every TSF interface we need.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, IEnumTfDisplayAttributeInfo, ITfCategoryMgr, ITfCompartmentMgr,
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
    ITfDisplayAttributeInfo, ITfDisplayAttributeProvider, ITfDisplayAttributeProvider_Impl,
    ITfDocumentMgr, ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfSource,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfTextInputProcessor_Impl,
    ITfThreadMgr, ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl, GUID_COMPARTMENT_EMPTYCONTEXT,
    GUID_COMPARTMENT_KEYBOARD_DISABLED, GUID_PROP_ATTRIBUTE, TF_INVALID_COOKIE,
};
use windows_core::{implement, ComObject, IUnknownImpl, Interface, Ref, BOOL, GUID};

use crate::display;
use crate::edit::{self, Op};
use crate::engine::{classify, Action, Engine, Key, Options};
use crate::exports::{guard, poisoned, ObjToken};
use crate::guids::GUID_DISPLAY_ATTR_INPUT;
use crate::registry;

#[derive(Default)]
struct State {
    thread_mgr: Option<ITfThreadMgr>,
    tm_sink_cookie: Option<u32>,
    key_sink_advised: bool,
    engine: Engine,
    opts: Options,
    composition: Option<ITfComposition>,
    /// Variant C3 (`NoComposition`): the plain document range showing the preedit.
    direct: Option<ITfRange>,
    /// Virtual keys whose key-down we ate; their key-up is eaten too (as SampleIME does), so the
    /// app never sees an unpaired WM_KEYUP.
    eaten_down: KeySet,
}

#[derive(Default)]
struct KeySet([u64; 4]);
impl KeySet {
    fn set(&mut self, vk: usize, on: bool) {
        let (i, b) = ((vk & 0xFF) / 64, 1u64 << (vk % 64));
        if on {
            self.0[i] |= b;
        } else {
            self.0[i] &= !b;
        }
    }
    fn get(&self, vk: usize) -> bool {
        self.0[(vk & 0xFF) / 64] & (1u64 << (vk % 64)) != 0
    }
}

// ITfTextInputProcessorEx's vtable QI also answers ITfTextInputProcessor, so it is not listed.
#[implement(
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfCompositionSink,
    ITfDisplayAttributeProvider
)]
pub struct TextService {
    // RefCell, never held across a call back into TSF: a TF_ES_SYNC edit session (or TSF
    // calling one of our sinks) re-enters this object on the same thread.
    st: RefCell<State>,
    client_id: Cell<u32>,
    input_atom: Cell<u32>,
    /// Bumped whenever the engine is reset behind the document's back (host termination).
    generation: Cell<u32>,
    _tok: ObjToken,
}

impl TextService {
    pub fn new() -> Self {
        TextService {
            st: RefCell::new(State::default()),
            client_id: Cell::new(0),
            input_atom: Cell::new(0),
            generation: Cell::new(0),
            _tok: ObjToken::new(),
        }
    }

    pub fn client_id(&self) -> u32 {
        self.client_id.get()
    }

    pub fn generation(&self) -> u32 {
        self.generation.get()
    }

    pub fn input_atom(&self) -> u32 {
        self.input_atom.get()
    }

    pub fn composition(&self) -> Option<ITfComposition> {
        self.st.borrow().composition.clone()
    }

    pub fn set_composition(&self, c: Option<ITfComposition>) {
        self.st.borrow_mut().composition = c;
    }

    pub fn direct(&self) -> Option<ITfRange> {
        self.st.borrow().direct.clone()
    }

    pub fn set_direct(&self, r: Option<ITfRange>) {
        self.st.borrow_mut().direct = r;
    }

    pub fn options(&self) -> Options {
        self.st.borrow().opts
    }

    /// Reset the engine to one letter and drop Updates queued against the old word.
    pub fn restart_with(&self, c: char) -> String {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.st.borrow_mut().engine.restart_with(c)
    }

    /// If a preedit is open (either kind): its context and the text to commit, engine cleared.
    fn take_pending(&self) -> Option<(ITfContext, String)> {
        let (range, text) = {
            let mut st = self.st.borrow_mut();
            let range = match (&st.composition, &st.direct) {
                (Some(c), _) => unsafe { c.GetRange().ok() },
                (None, Some(r)) => Some(r.clone()),
                (None, None) => None,
            }?;
            let text = st.engine.take();
            let opts = st.opts;
            (range, opts.committed(&text))
        };
        let ctx = unsafe { range.GetContext().ok()? };
        Some((ctx, text))
    }

    fn active(&self) -> bool {
        !poisoned() && self.st.borrow().key_sink_advised
    }

    fn teardown(&self) {
        let (tm, cookie, keys) = {
            let mut st = self.st.borrow_mut();
            st.engine.reset();
            st.composition = None;
            st.direct = None;
            (
                st.thread_mgr.take(),
                st.tm_sink_cookie.take(),
                std::mem::take(&mut st.key_sink_advised),
            )
        };
        if let Some(tm) = tm {
            unsafe {
                if keys {
                    if let Ok(k) = tm.cast::<ITfKeystrokeMgr>() {
                        let _ = k.UnadviseKeyEventSink(self.client_id.get());
                    }
                }
                if let Some(c) = cookie {
                    if let Ok(src) = tm.cast::<ITfSource>() {
                        let _ = src.UnadviseSink(c);
                    }
                }
            }
        }
        self.client_id.set(0);
        self.input_atom.set(0);
    }
}

/// Password fields / read-only contexts (PLAN 2.4): the host sets these compartments.
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

fn current_key(wparam: WPARAM) -> Key {
    let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe {
        GetKeyState(vk.0 as i32) < 0
    };
    classify(
        wparam.0 as u32,
        down(VK_SHIFT),
        down(VK_CONTROL),
        down(VK_MENU),
    )
}

fn action_to_op(a: Action, key: Key, o: &Options) -> Option<(Op, bool, bool)> {
    // (op, sync, eaten). Bidi decoration (variant A) is applied here, outside the engine, so
    // Backspace/Escape/ZWNJ keep operating on the plain Latin buffer.
    let last = match key {
        Key::Letter(c) => Some(c),
        _ => None,
    };
    Some(match a {
        Action::PassThrough => return None,
        Action::Update(s) => (
            Op::Update {
                text: o.display(&s),
                last,
            },
            false,
            true,
        ),
        Action::Commit { text, append } => (
            Op::Commit {
                text: o.committed(&text),
                append,
            },
            false,
            true,
        ),
        Action::CommitAndCompose { text, preedit } => (
            Op::CommitAndCompose {
                text: o.committed(&text),
                preedit: o.display(&preedit),
            },
            false,
            true,
        ),
        // Must be in the document before the app sees the key (e.g. Enter in a chat box).
        Action::CommitAndPass(text) => (
            Op::Commit {
                text: o.committed(&text),
                append: String::new(),
            },
            true,
            false,
        ),
        Action::Cancel => (Op::Cancel, false, true),
        Action::Insert(s) => (Op::Insert(s), false, true),
        Action::ToggleZwnj => (Op::ToggleZwnj, false, true),
    })
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> windows_core::Result<()> {
        guard(|| ITfTextInputProcessorEx_Impl::ActivateEx(self, ptim, tid, 0))
    }

    fn Deactivate(&self) -> windows_core::Result<()> {
        guard(|| {
            // Finish any open preedit synchronously while we still have a client id.
            if let Some((ctx, text)) = self.take_pending() {
                let op = Op::Commit {
                    text,
                    append: String::new(),
                };
                let _ = edit::request(self.to_object(), &ctx, op, true);
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
            self.client_id.set(tid);
            {
                let mut st = self.st.borrow_mut();
                st.thread_mgr = Some(tm.clone());
                st.opts = registry::spike_options();
            }
            let result = (|| unsafe {
                let src = tm.cast::<ITfSource>()?;
                let sink: ITfThreadMgrEventSink = self.to_interface();
                let cookie = src.AdviseSink(&ITfThreadMgrEventSink::IID, &sink)?;
                if cookie != TF_INVALID_COOKIE {
                    self.st.borrow_mut().tm_sink_cookie = Some(cookie);
                }

                let keys: ITfKeyEventSink = self.to_interface();
                tm.cast::<ITfKeystrokeMgr>()?
                    .AdviseKeyEventSink(tid, &keys, true)?;
                self.st.borrow_mut().key_sink_advised = true;

                let cats: ITfCategoryMgr =
                    CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
                self.input_atom
                    .set(cats.RegisterGUID(&GUID_DISPLAY_ATTR_INPUT).unwrap_or(0));
                windows_core::Result::Ok(())
            })();
            if result.is_err() {
                self.teardown();
            }
            result
        })
    }
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> windows_core::Result<()> {
        guard(|| Ok(()))
    }

    fn OnTestKeyDown(
        &self,
        pic: Ref<ITfContext>,
        wparam: WPARAM,
        _l: LPARAM,
    ) -> windows_core::Result<BOOL> {
        guard(|| {
            let Some(ctx) = pic.as_ref() else {
                return Ok(false.into());
            };
            if !self.active() || context_disabled(ctx) {
                return Ok(false.into());
            }
            let key = current_key(wparam);
            let st = self.st.borrow();
            Ok(st.engine.would_eat(key, &st.opts).into())
        })
    }

    fn OnKeyDown(
        &self,
        pic: Ref<ITfContext>,
        wparam: WPARAM,
        _l: LPARAM,
    ) -> windows_core::Result<BOOL> {
        guard(|| {
            let Some(ctx) = pic.as_ref() else {
                return Ok(false.into());
            };
            if !self.active() || context_disabled(ctx) {
                return Ok(false.into());
            }
            let key = current_key(wparam);
            let (action, opts) = {
                let mut st = self.st.borrow_mut();
                let opts = st.opts;
                (st.engine.on_key(key, &opts), opts)
            };
            let Some((op, sync, eaten)) = action_to_op(action, key, &opts) else {
                return Ok(false.into());
            };
            self.st.borrow_mut().eaten_down.set(wparam.0, eaten);
            let res = edit::request(self.to_object(), ctx, op.clone(), sync);
            if !sync {
                let _queued = res?;
            } else if !matches!(res, Ok(hr) if hr.is_ok()) {
                // TF_E_SYNCHRONOUS (document locked): a sync session was refused. Commit
                // async and eat the key rather than let it land before our text.
                self.st.borrow_mut().eaten_down.set(wparam.0, true);
                let _queued = edit::request(self.to_object(), ctx, op, false)?;
                return Ok(true.into());
            }
            Ok(eaten.into())
        })
    }

    fn OnTestKeyUp(
        &self,
        _pic: Ref<ITfContext>,
        w: WPARAM,
        _l: LPARAM,
    ) -> windows_core::Result<BOOL> {
        guard(|| Ok(self.st.borrow().eaten_down.get(w.0).into()))
    }

    fn OnKeyUp(&self, _pic: Ref<ITfContext>, w: WPARAM, _l: LPARAM) -> windows_core::Result<BOOL> {
        guard(|| {
            let mut st = self.st.borrow_mut();
            let was = st.eaten_down.get(w.0);
            st.eaten_down.set(w.0, false);
            Ok(was.into())
        })
    }

    fn OnPreservedKey(&self, _pic: Ref<ITfContext>, _g: *const GUID) -> windows_core::Result<BOOL> {
        guard(|| Ok(false.into()))
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
        _focus: Ref<ITfDocumentMgr>,
        _prev: Ref<ITfDocumentMgr>,
    ) -> windows_core::Result<()> {
        guard(|| {
            // Focus moved: commit what is shown into the document it belongs to (PLAN 2.1).
            if let Some((ctx, text)) = self.take_pending() {
                let op = Op::Commit {
                    text,
                    append: String::new(),
                };
                let _queued = edit::request(self.to_object(), &ctx, op, false)?;
            }
            Ok(())
        })
    }
    fn OnPushContext(&self, _c: Ref<ITfContext>) -> windows_core::Result<()> {
        guard(|| Ok(()))
    }
    fn OnPopContext(&self, _c: Ref<ITfContext>) -> windows_core::Result<()> {
        guard(|| Ok(()))
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    fn OnCompositionTerminated(
        &self,
        ec: u32,
        comp: Ref<ITfComposition>,
    ) -> windows_core::Result<()> {
        guard(|| {
            // The host ended it (click elsewhere, app reset...). Keep its text, drop our state.
            {
                let mut st = self.st.borrow_mut();
                st.engine.reset();
                st.composition = None;
            }
            self.generation.set(self.generation.get().wrapping_add(1));
            if let Some(comp) = comp.as_ref() {
                unsafe {
                    if let Ok(range) = comp.GetRange() {
                        if let Ok(ctx) = range.GetContext() {
                            if let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
                                let _ = prop.Clear(ec, &range);
                            }
                        }
                    }
                }
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
}
