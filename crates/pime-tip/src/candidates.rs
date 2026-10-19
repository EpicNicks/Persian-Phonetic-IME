//! Stream B: the `CandidatePresenter` over `pime_candidate_ui::CandidateWindow`, plus the
//! `ITfCandidateListUIElement` (+ Behavior) object for UI-less hosts (games, full-screen
//! apps, hosts that draw the list themselves).
//!
//! Threading: a presenter and everything it creates live on the TSF thread that created it
//! (one presenter per thread). The popup window is created lazily on the first `show`, on
//! that thread, and reused until the presenter is dropped. Nothing here is global: each
//! presenter owns its window, so any number of host processes / threads can each have one.
//!
//! Window class lifetime: the class is registered against the TIP DLL's HINSTANCE and is
//! not unregistered when the DLL unloads (Windows never does that for DLLs). That is
//! acceptable: `CandidateWindow::new` detects a stale class left by an unloaded copy and
//! replaces it. A host-side tidy-up can call `pime_candidate_ui::unregister_class()` after
//! the last presenter is dropped (e.g. from the last `Deactivate`), never from `DllMain`.

use std::cell::{Cell, RefCell};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::{Rc, Weak};

use pime_candidate_ui::{CandidateEvent, CandidateView, CandidateWindow, Rect};
use pime_core::CandidateList;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::LogicalToPhysicalPointForPerMonitorDPI;
use windows::Win32::UI::TextServices::{
    ITfCandidateListUIElement, ITfCandidateListUIElementBehavior,
    ITfCandidateListUIElementBehavior_Impl, ITfCandidateListUIElement_Impl, ITfDocumentMgr,
    ITfThreadMgr, ITfThreadMgrEx, ITfUIElement, ITfUIElementMgr, ITfUIElement_Impl, TF_CLUIE_COUNT,
    TF_CLUIE_CURRENTPAGE, TF_CLUIE_DOCUMENTMGR, TF_CLUIE_PAGEINDEX, TF_CLUIE_SELECTION,
    TF_CLUIE_STRING, TF_TMF_UIELEMENTENABLEDONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetGUIThreadInfo, GUITHREADINFO};
use windows_core::{implement, Error, Result, BOOL, BSTR, GUID};

use crate::presenter::{CandidatePresenter, PreferredSide, ScreenRect};

/// `ITfUIElement::GetGUID` of our candidate list (identifies the element kind to hosts).
pub const GUID_CANDIDATE_UI_ELEMENT: GUID = GUID::from_u128(0x8ab5e048_eb59_4c30_b501_39bb3c5e2eb1);

const ALL_FLAGS: u32 = TF_CLUIE_DOCUMENTMGR
    | TF_CLUIE_COUNT
    | TF_CLUIE_SELECTION
    | TF_CLUIE_STRING
    | TF_CLUIE_PAGEINDEX
    | TF_CLUIE_CURRENTPAGE;

/// Presenter options. `Default`: no labels, footer on, no UI element.
#[derive(Clone, Default)]
pub struct PresenterOptions {
    /// Config `[options] candidate_labels`: learner hints ("sin", "sad") from
    /// `pime_core::candidate_label` beside each candidate.
    pub candidate_labels: bool,
    /// Hide the "2/3" footer.
    pub hide_footer: bool,
    /// Config `[options] candidate_size` (percent; 0 means 100).
    pub size_percent: u32,
    /// The thread manager from `ActivateEx`. When set, every list is also published as an
    /// `ITfCandidateListUIElement` through `ITfUIElementMgr` (Begin/Update/EndUIElement), and
    /// our own window is drawn only when the host lets us (`pbShow`, `ITfUIElement::Show`,
    /// `TF_TMF_UIELEMENTENABLEDONLY`).
    pub thread_mgr: Option<ITfThreadMgr>,
}

/// PINNED: stream A calls this once per TSF thread. Plain popup, no UI element, no labels.
pub fn create_presenter() -> Box<dyn CandidatePresenter> {
    Box::new(PopupPresenter::new(PresenterOptions::default()))
}

/// Like `create_presenter` with explicit options (labels, UI-less support).
pub fn create_presenter_with(opts: PresenterOptions) -> Box<dyn CandidatePresenter> {
    Box::new(PopupPresenter::new(opts))
}

/// Presenter that also publishes the list as a TSF UI element (UI-less hosts). Call from
/// `ActivateEx` with the thread manager it received; same as `create_presenter_with` with
/// `thread_mgr` set.
pub fn register_ui_element(
    thread_mgr: &ITfThreadMgr,
    candidate_labels: bool,
    size_percent: u32,
) -> Box<dyn CandidatePresenter> {
    create_presenter_with(PresenterOptions {
        candidate_labels,
        size_percent,
        thread_mgr: Some(thread_mgr.clone()),
        ..Default::default()
    })
}

/// Map the engine's list to what the popup draws.
pub fn to_view(
    list: &CandidateList,
    candidate_labels: bool,
    footer: bool,
    size_percent: u32,
) -> CandidateView {
    CandidateView {
        items: list.items.clone(),
        highlighted: list.highlighted,
        show_numbers: true,
        labels: candidate_labels.then(|| {
            list.items
                .iter()
                .map(|c| pime_core::candidate_label(c).unwrap_or("").to_owned())
                .collect()
        }),
        show_footer: footer,
        size_percent,
    }
}

pub fn to_rect(r: ScreenRect) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

/// `GetTextExt` failed or returned nothing usable (TSF hosts report an all-zero rect).
pub fn is_degenerate(r: ScreenRect) -> bool {
    r == ScreenRect::default() || r.right < r.left || r.bottom < r.top
}

/// Convert a rect in `hwnd`'s logical coordinates (what `GetTextExt` returns in a
/// DPI-unaware host) to physical screen pixels. A no-op for DPI-aware hosts. For stream A:
/// `hwnd` is `ITfContextView::GetWnd`.
pub fn to_physical(hwnd: HWND, r: ScreenRect) -> ScreenRect {
    let conv = |x: i32, y: i32| {
        let mut p = POINT { x, y };
        // SAFETY: valid out-param; failure leaves the point as is.
        let _ = unsafe { LogicalToPhysicalPointForPerMonitorDPI(Some(hwnd), &mut p) };
        p
    };
    let (a, b) = (conv(r.left, r.top), conv(r.right, r.bottom));
    ScreenRect {
        left: a.x,
        top: a.y,
        right: b.x,
        bottom: b.y,
    }
}

/// The system caret of this thread in physical screen pixels (`GetGUIThreadInfo`), for
/// hosts where `GetTextExt` keeps failing with `TS_E_NOLAYOUT`.
pub fn caret_fallback_anchor() -> Option<ScreenRect> {
    let mut gui = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: correctly sized struct; plain queries.
    unsafe {
        GetGUIThreadInfo(GetCurrentThreadId(), &mut gui).ok()?;
        if gui.hwndCaret.is_invalid() {
            return None;
        }
        let c = gui.rcCaret;
        let mut tl = POINT {
            x: c.left,
            y: c.top,
        };
        let mut br = POINT {
            x: c.right,
            y: c.bottom,
        };
        if !ClientToScreen(gui.hwndCaret, &mut tl).as_bool()
            || !ClientToScreen(gui.hwndCaret, &mut br).as_bool()
        {
            return None;
        }
        let r = ScreenRect {
            left: tl.x,
            top: tl.y,
            right: br.x.max(tl.x + 1),
            bottom: br.y,
        };
        Some(to_physical(gui.hwndCaret, r))
    }
}

fn cursor_anchor() -> ScreenRect {
    let mut p = POINT::default();
    // SAFETY: out-param is a local.
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    ScreenRect {
        left: p.x,
        top: p.y,
        right: p.x + 1,
        bottom: p.y + 20,
    }
}

type ClickSlot = Rc<RefCell<Option<Box<dyn FnMut(usize)>>>>;

/// Call the click callback without holding a borrow across it, so the callback may call
/// back into the presenter (including `set_on_click`).
fn fire_click(slot: &ClickSlot, index: usize) {
    let Some(mut f) = slot.try_borrow_mut().ok().and_then(|mut s| s.take()) else {
        return;
    };
    f(index);
    if let Ok(mut s) = slot.try_borrow_mut() {
        if s.is_none() {
            *s = Some(f);
        }
    }
}

/// State shared by the presenter, its window callback and the UI element.
struct Core {
    window: RefCell<Option<CandidateWindow>>,
    create_failed: Cell<bool>,
    on_click: ClickSlot,
    view: RefCell<CandidateView>,
    anchor: Cell<Option<(Rect, bool)>>,
    /// `show` was called and no `hide` since.
    active: Cell<bool>,
    /// The host lets us draw our own window (UI element `pbShow` / `Show`).
    host_allows: Cell<bool>,
}

impl Core {
    fn ensure_window(&self) -> bool {
        if self.window.borrow().is_some() {
            return true;
        }
        if self.create_failed.get() {
            return false;
        }
        let slot = self.on_click.clone();
        match CandidateWindow::new(move |ev| {
            if let CandidateEvent::Click(i) = ev {
                fire_click(&slot, i);
            }
        }) {
            Ok(w) => {
                *self.window.borrow_mut() = Some(w);
                true
            }
            Err(_) => {
                // Don't retry on every key; typing still works without the popup.
                self.create_failed.set(true);
                false
            }
        }
    }

    /// Show or hide our own window to match `active && host_allows`.
    fn sync_window(&self) {
        let want = self.active.get() && self.host_allows.get();
        if want && !self.ensure_window() {
            return;
        }
        let Ok(w) = self.window.try_borrow() else {
            return;
        };
        let Some(w) = w.as_ref() else { return };
        if !want {
            w.hide();
            return;
        }
        if let Ok(view) = self.view.try_borrow() {
            w.update(&view);
        }
        if let Some((anchor, above)) = self.anchor.get() {
            w.show_at(anchor, above);
        }
    }

    fn set_highlight(&self, index: usize) {
        if let Ok(mut v) = self.view.try_borrow_mut() {
            v.highlighted = index;
        }
        if let Ok(w) = self.window.try_borrow() {
            if let Some(w) = w.as_ref() {
                w.set_highlight(index);
            }
        }
    }
}

/// The real presenter. Stream A can hold this concrete type instead of the boxed trait to
/// reach `set_candidate_labels` on config reload (or simply recreate the presenter).
pub struct PopupPresenter {
    core: Rc<Core>,
    labels: bool,
    footer: bool,
    size_percent: u32,
    ui: Option<UiLink>,
}

struct UiLink {
    thread_mgr: ITfThreadMgr,
    mgr: ITfUIElementMgr,
    element: ITfUIElement,
    state: Rc<ElementState>,
    id: Option<u32>,
}

impl PopupPresenter {
    pub fn new(opts: PresenterOptions) -> Self {
        let core = Rc::new(Core {
            window: RefCell::new(None),
            create_failed: Cell::new(false),
            on_click: Rc::new(RefCell::new(None)),
            view: RefCell::new(CandidateView::default()),
            anchor: Cell::new(None),
            active: Cell::new(false),
            host_allows: Cell::new(true),
        });
        let ui = opts.thread_mgr.and_then(|tm| {
            let mgr: ITfUIElementMgr = windows_core::Interface::cast(&tm).ok()?;
            let state = Rc::new(ElementState::new(Rc::downgrade(&core)));
            let element: ITfUIElement = CandidateUiElement {
                state: state.clone(),
            }
            .into();
            Some(UiLink {
                thread_mgr: tm,
                mgr,
                element,
                state,
                id: None,
            })
        });
        Self {
            core,
            labels: opts.candidate_labels,
            footer: !opts.hide_footer,
            size_percent: opts.size_percent,
            ui,
        }
    }

    /// Config `candidate_labels` changed; takes effect on the next `show`.
    pub fn set_candidate_labels(&mut self, on: bool) {
        self.labels = on;
    }

    /// True once the popup window exists (created by the first `show`).
    pub fn has_window(&self) -> bool {
        self.core.window.borrow().is_some()
    }

    pub fn is_window_visible(&self) -> bool {
        self.core
            .window
            .borrow()
            .as_ref()
            .is_some_and(CandidateWindow::is_visible)
    }

    fn ui_only_mode(&self) -> bool {
        let Some(ui) = &self.ui else { return false };
        let Ok(ex) = windows_core::Interface::cast::<ITfThreadMgrEx>(&ui.thread_mgr) else {
            return false;
        };
        // SAFETY: plain query.
        unsafe { ex.GetActiveFlags() }.is_ok_and(|f| f & TF_TMF_UIELEMENTENABLEDONLY != 0)
    }

    /// Begin or update the UI element; returns whether our own window may be drawn.
    fn publish(&mut self, list: &CandidateList) -> bool {
        let ui_only = self.ui_only_mode();
        let Some(ui) = &mut self.ui else { return true };
        ui.state.set_items(&list.items, list.highlighted);
        match ui.id {
            Some(id) => {
                // SAFETY: plain TSF call; the element only touches its own cells.
                let _ = unsafe { ui.mgr.UpdateUIElement(id) };
            }
            None => {
                // SAFETY: as above.
                *ui.state.doc_mgr.borrow_mut() = unsafe { ui.thread_mgr.GetFocus() }.ok();
                ui.state.flags.set(ALL_FLAGS);
                let mut show = BOOL(1);
                let mut id = 0u32;
                // SAFETY: valid out-params; the host may call back into the element.
                match unsafe { ui.mgr.BeginUIElement(&ui.element, &mut show, &mut id) } {
                    Ok(()) => {
                        ui.id = Some(id);
                        ui.state.host_shows.set(show.as_bool());
                    }
                    Err(_) => ui.state.host_shows.set(true),
                }
            }
        }
        ui.state.host_shows.get() && !ui_only
    }

    fn end_ui_element(&mut self) {
        if let Some(ui) = &mut self.ui {
            if let Some(id) = ui.id.take() {
                // SAFETY: plain TSF call.
                let _ = unsafe { ui.mgr.EndUIElement(id) };
            }
            *ui.state.doc_mgr.borrow_mut() = None;
        }
    }
}

impl CandidatePresenter for PopupPresenter {
    fn show(&mut self, list: &CandidateList, anchor: ScreenRect, side: PreferredSide) {
        if list.items.is_empty() {
            self.hide();
            return;
        }
        let anchor = if !is_degenerate(anchor) {
            to_rect(anchor)
        } else if let Some((last, _)) = self.core.anchor.get() {
            last
        } else {
            to_rect(caret_fallback_anchor().unwrap_or_else(cursor_anchor))
        };
        *self.core.view.borrow_mut() = to_view(list, self.labels, self.footer, self.size_percent);
        self.core
            .anchor
            .set(Some((anchor, side == PreferredSide::Above)));
        let allowed = self.publish(list);
        self.core.host_allows.set(allowed);
        self.core.active.set(true);
        self.core.sync_window();
    }

    fn set_highlight(&mut self, index: usize) {
        self.core.set_highlight(index);
        if let Some(ui) = &self.ui {
            ui.state.selection.set(index as u32);
            ui.state
                .flags
                .set(ui.state.flags.get() | TF_CLUIE_SELECTION);
            if let Some(id) = ui.id {
                // SAFETY: plain TSF call.
                let _ = unsafe { ui.mgr.UpdateUIElement(id) };
            }
        }
    }

    fn hide(&mut self) {
        self.core.active.set(false);
        self.core.anchor.set(None);
        self.end_ui_element();
        self.core.sync_window();
    }

    fn set_on_click(&mut self, f: Box<dyn FnMut(usize)>) {
        if let Ok(mut s) = self.core.on_click.try_borrow_mut() {
            *s = Some(f);
        }
    }
}

impl Drop for PopupPresenter {
    fn drop(&mut self) {
        self.end_ui_element();
    }
}

// ------------------------------------------------------------------ UI element

/// Data behind the COM object. Only cells, so host callbacks that arrive inside our own
/// Begin/UpdateUIElement calls never collide with a borrow.
struct ElementState {
    items: RefCell<Vec<String>>,
    selection: Cell<u32>,
    flags: Cell<u32>,
    host_shows: Cell<bool>,
    doc_mgr: RefCell<Option<ITfDocumentMgr>>,
    core: Weak<Core>,
}

impl ElementState {
    fn new(core: Weak<Core>) -> Self {
        Self {
            items: RefCell::new(Vec::new()),
            selection: Cell::new(0),
            flags: Cell::new(0),
            host_shows: Cell::new(true),
            doc_mgr: RefCell::new(None),
            core,
        }
    }

    fn set_items(&self, items: &[String], selection: usize) {
        let mut f = self.flags.get();
        if let Ok(mut cur) = self.items.try_borrow_mut() {
            if cur.as_slice() != items {
                f |= TF_CLUIE_COUNT | TF_CLUIE_STRING | TF_CLUIE_PAGEINDEX;
                *cur = items.to_vec();
            }
        }
        if self.selection.get() != selection as u32 {
            f |= TF_CLUIE_SELECTION;
            self.selection.set(selection as u32);
        }
        self.flags.set(f);
    }

    fn count(&self) -> u32 {
        self.items.try_borrow().map_or(0, |v| v.len() as u32)
    }
}

/// COM entry points must never unwind into the host.
fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_FAIL.into()))
}

#[implement(
    ITfCandidateListUIElementBehavior,
    ITfCandidateListUIElement,
    ITfUIElement
)]
struct CandidateUiElement {
    state: Rc<ElementState>,
}

impl ITfUIElement_Impl for CandidateUiElement_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        guard(|| Ok(BSTR::from("Persian Phonetic candidates")))
    }

    fn GetGUID(&self) -> Result<GUID> {
        Ok(GUID_CANDIDATE_UI_ELEMENT)
    }

    fn Show(&self, bshow: BOOL) -> Result<()> {
        guard(|| {
            self.state.host_shows.set(bshow.as_bool());
            if let Some(core) = self.state.core.upgrade() {
                core.host_allows.set(bshow.as_bool());
                core.sync_window();
            }
            Ok(())
        })
    }

    fn IsShown(&self) -> Result<BOOL> {
        guard(|| {
            let visible = self.state.core.upgrade().is_some_and(|c| {
                c.window
                    .try_borrow()
                    .ok()
                    .and_then(|w| w.as_ref().map(CandidateWindow::is_visible))
                    .unwrap_or(false)
            });
            Ok(BOOL::from(visible))
        })
    }
}

impl ITfCandidateListUIElement_Impl for CandidateUiElement_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        guard(|| Ok(self.state.flags.replace(0)))
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        guard(|| {
            self.state
                .doc_mgr
                .try_borrow()
                .ok()
                .and_then(|d| d.clone())
                .ok_or_else(|| Error::from(E_FAIL))
        })
    }

    fn GetCount(&self) -> Result<u32> {
        guard(|| Ok(self.state.count()))
    }

    fn GetSelection(&self) -> Result<u32> {
        guard(|| Ok(self.state.selection.get()))
    }

    fn GetString(&self, uindex: u32) -> Result<BSTR> {
        guard(|| {
            let items = self
                .state
                .items
                .try_borrow()
                .map_err(|_| Error::from(E_FAIL))?;
            items
                .get(uindex as usize)
                .map(|s| BSTR::from(s.as_str()))
                .ok_or_else(|| E_INVALIDARG.into())
        })
    }

    fn GetPageIndex(&self, pindex: *mut u32, usize: u32, pupagecnt: *mut u32) -> Result<()> {
        // Every list fits one page (at most 9 rows, the 1-9 hot keys).
        guard(|| {
            if pupagecnt.is_null() {
                return Err(E_INVALIDARG.into());
            }
            // SAFETY: caller-provided out-params, checked for null; `pindex` holds `usize`.
            unsafe {
                *pupagecnt = 1;
                if !pindex.is_null() && usize >= 1 {
                    *pindex = 0;
                }
            }
            Ok(())
        })
    }

    fn SetPageIndex(&self, _pindex: *const u32, _upagecnt: u32) -> Result<()> {
        Ok(())
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(0)
    }
}

impl ITfCandidateListUIElementBehavior_Impl for CandidateUiElement_Impl {
    fn SetSelection(&self, nindex: u32) -> Result<()> {
        guard(|| {
            if nindex >= self.state.count() {
                return Err(E_INVALIDARG.into());
            }
            self.state.selection.set(nindex);
            if let Some(core) = self.state.core.upgrade() {
                core.set_highlight(nindex as usize);
            }
            Ok(())
        })
    }

    /// Commit the selection: same path as a mouse click on that row.
    fn Finalize(&self) -> Result<()> {
        guard(|| {
            if self.state.count() == 0 {
                return Ok(());
            }
            if let Some(core) = self.state.core.upgrade() {
                fire_click(&core.on_click, self.state.selection.get() as usize);
            }
            Ok(())
        })
    }

    /// Escape-like cancel from the host. The session owns the list's lifetime and the
    /// pinned trait has no cancel callback, so this is a no-op (the list stays until the
    /// next key or commit).
    fn Abort(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::CandidateKind;

    fn list(items: &[&str], highlighted: usize) -> CandidateList {
        CandidateList {
            items: items.iter().map(|s| (*s).to_owned()).collect(),
            highlighted,
            kind: CandidateKind::Alternates,
        }
    }

    #[test]
    fn view_mapping() {
        let v = to_view(&list(&["س", "ص", "ث"], 1), false, true, 50);
        assert_eq!(v.items, ["س", "ص", "ث"]);
        assert_eq!(v.highlighted, 1);
        assert!(v.show_numbers && v.show_footer);
        assert_eq!(v.labels, None);
        assert_eq!(v.footer_text().as_deref(), Some("2/3"));
        let v = to_view(&list(&["س", "ص", "?"], 0), true, false, 50);
        let labels = v.labels.unwrap();
        assert_eq!(labels[0], "sin");
        assert_eq!(labels[1], "sad");
        assert_eq!(labels[2], "", "unknown candidates get no label");
        assert!(!v.show_footer);
    }

    #[test]
    fn anchors() {
        let r = ScreenRect {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
        };
        assert_eq!(to_rect(r), Rect::new(1, 2, 3, 4));
        assert!(!is_degenerate(r));
        assert!(is_degenerate(ScreenRect::default()));
        assert!(is_degenerate(ScreenRect { right: 0, ..r }));
    }

    #[test]
    fn click_slot_survives_reentrant_replace() {
        let slot: ClickSlot = Rc::new(RefCell::new(None));
        let hits = Rc::new(Cell::new(0usize));
        let (s2, h2) = (slot.clone(), hits.clone());
        *slot.borrow_mut() = Some(Box::new(move |i| {
            h2.set(h2.get() + i);
            // Replacing the callback from inside it must not panic and must win.
            let h3 = h2.clone();
            *s2.borrow_mut() = Some(Box::new(move |i| h3.set(h3.get() + 100 * i)));
        }));
        fire_click(&slot, 2);
        fire_click(&slot, 1);
        assert_eq!(hits.get(), 102);
    }

    fn element() -> (
        ITfCandidateListUIElementBehavior,
        Rc<ElementState>,
        Rc<Core>,
    ) {
        let p = PopupPresenter::new(PresenterOptions::default());
        let state = Rc::new(ElementState::new(Rc::downgrade(&p.core)));
        let el: ITfCandidateListUIElementBehavior = CandidateUiElement {
            state: state.clone(),
        }
        .into();
        (el, state, p.core.clone())
    }

    #[test]
    fn ui_element_reports_list() {
        let (el, state, _core) = element();
        state.flags.set(ALL_FLAGS);
        state.set_items(&["ز".into(), "ذ".into(), "ض".into(), "ظ".into()], 2);
        // SAFETY: calls on our own COM object.
        unsafe {
            assert_eq!(el.GetUpdatedFlags().unwrap(), ALL_FLAGS);
            assert_eq!(el.GetUpdatedFlags().unwrap(), 0, "flags reset after read");
            assert_eq!(el.GetCount().unwrap(), 4);
            assert_eq!(el.GetSelection().unwrap(), 2);
            assert_eq!(el.GetString(1).unwrap().to_string(), "ذ");
            assert!(el.GetString(9).is_err());
            let (mut idx, mut cnt) = ([7u32], 0u32);
            el.GetPageIndex(&mut idx, &mut cnt).unwrap();
            assert_eq!((idx[0], cnt), (0, 1));
            assert_eq!(el.GetCurrentPage().unwrap(), 0);
            assert!(el.GetDocumentMgr().is_err(), "no focus document in tests");
            assert_eq!(el.GetGUID().unwrap(), GUID_CANDIDATE_UI_ELEMENT);
        }
        state.set_items(&["ز".into(), "ذ".into(), "ض".into(), "ظ".into()], 3);
        // SAFETY: as above.
        unsafe { assert_eq!(el.GetUpdatedFlags().unwrap(), TF_CLUIE_SELECTION) };
    }

    #[test]
    fn ui_element_behavior_selects_and_finalizes() {
        let (el, state, core) = element();
        state.set_items(&["ت".into(), "ط".into()], 0);
        let picked = Rc::new(Cell::new(None));
        let p2 = picked.clone();
        *core.on_click.borrow_mut() = Some(Box::new(move |i| p2.set(Some(i))));
        // SAFETY: calls on our own COM object.
        unsafe {
            assert!(el.SetSelection(5).is_err());
            el.SetSelection(1).unwrap();
            assert_eq!(el.GetSelection().unwrap(), 1);
            el.Finalize().unwrap();
            el.Abort().unwrap();
        }
        assert_eq!(picked.get(), Some(1));
        assert_eq!(core.view.borrow().highlighted, 1);
    }

    /// Creates a real (briefly visible) popup on this thread, then destroys it.
    #[test]
    fn presenter_creates_window_lazily() {
        let mut p = PopupPresenter::new(PresenterOptions {
            candidate_labels: true,
            ..Default::default()
        });
        assert!(!p.has_window());
        p.hide();
        assert!(!p.has_window(), "hide does not create the window");
        let anchor = ScreenRect {
            left: 200,
            top: 200,
            right: 260,
            bottom: 224,
        };
        p.show(&list(&["س", "ص", "ث"], 0), anchor, PreferredSide::Below);
        assert!(p.has_window());
        assert!(p.is_window_visible());
        p.set_highlight(2);
        assert_eq!(p.core.view.borrow().highlighted, 2);
        p.show(&list(&["ا", "آ"], 1), anchor, PreferredSide::Above);
        p.show(&list(&[], 0), anchor, PreferredSide::Below);
        assert!(!p.is_window_visible(), "an empty list hides");
        p.show(&list(&["ا", "آ"], 0), anchor, PreferredSide::Below);
        assert!(p.is_window_visible());
        p.hide();
        assert!(!p.is_window_visible());
        assert!(p.has_window(), "window is reused, not destroyed");
    }
}
