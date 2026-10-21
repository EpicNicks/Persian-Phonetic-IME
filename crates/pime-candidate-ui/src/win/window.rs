//! The popup window itself. One `CandidateWindow` owns one HWND that is created once and
//! then shown, hidden and updated in place.
//!
//! Thread affinity: every method, and the callback, must be used on the thread that
//! created the window (the Win32 rule for HWNDs). Re-entrancy: window messages can arrive
//! synchronously from inside our own `SetWindowPos`/`ShowWindow`, so no `RefCell` borrow
//! is ever held across a Win32 call that can send messages.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

use windows::core::{w, Error, Result, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, UpdateWindow, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClassInfoExW, GetWindowLongPtrW,
    IsWindowVisible, LoadCursorW, RegisterClassW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    UnregisterClassW, CS_DROPSHADOW, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW, MA_NOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SW_HIDE, SW_SHOWNOACTIVATE, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_GETOBJECT, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCDESTROY,
    WM_PAINT, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_THEMECHANGED, WNDCLASSEXW, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::render::Renderer;
use super::sys;
use super::uia::{self, Shared, Snapshot, WM_PIME_SELECT};
use crate::geometry::{Point, Rect};
use crate::layout::{Layout, LayoutInput};
use crate::placement::{place, PlacementInput};
use crate::theme::{Palette, Theme};
use crate::view::CandidateView;

/// Mouse interaction reported through the callback. Indices are 0-based combined rows:
/// alternates (`CandidateView::items`) first, then completions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateEvent {
    /// The pointer moved onto a different row. The popup only tints that row; the
    /// highlight (selection) stays where the owner put it.
    Hover(usize),
    /// Left button pressed on a row, or UI Automation `Select()` on an item.
    Click(usize),
}

const WM_MOUSELEAVE: u32 = 0x02A3;
const CLASS_NAME: PCWSTR = w!("PimeCandidateWindow");
/// Gap between the anchor rectangle and the popup, in DIPs.
const ANCHOR_GAP_DIP: f32 = 2.0;

struct State {
    view: CandidateView,
    layout: Layout,
    scale: f32,
    theme: Theme,
    palette: Palette,
    renderer: Renderer,
    anchor: Option<Rect>,
    prefer_above: bool,
    /// Row under the mouse pointer.
    hot: Option<usize>,
    tracking_leave: bool,
}

struct Inner {
    hwnd: Cell<HWND>,
    state: RefCell<State>,
    callback: RefCell<Box<dyn FnMut(CandidateEvent)>>,
    shared: Shared,
}

pub struct CandidateWindow {
    inner: Box<Inner>,
}

fn module_handle() -> HINSTANCE {
    // The module containing this code (the TIP DLL when embedded), not the host exe.
    let mut h = Default::default();
    // SAFETY: address of a function in this module; refcount unchanged.
    unsafe {
        let _ = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(wnd_proc as *const () as *const u16),
            &mut h,
        );
    }
    h.into()
}

fn register_class(hinst: HINSTANCE) -> Result<()> {
    // SAFETY: class struct is fully initialised; strings are static.
    unsafe {
        // Windows never unregisters a DLL's classes when the DLL unloads. If an earlier copy
        // of this module left the class behind with a dead window procedure, replace it.
        let mut existing = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            ..Default::default()
        };
        if GetClassInfoExW(Some(hinst), CLASS_NAME, &mut existing).is_ok() {
            let ours = wnd_proc as *const () as usize;
            if existing.lpfnWndProc.map(|f| f as *const () as usize) == Some(ours) {
                return Ok(());
            }
            let _ = UnregisterClassW(CLASS_NAME, Some(hinst));
        }
        let wc = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            let e = Error::from_thread();
            if e.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
                return Err(e);
            }
        }
    }
    Ok(())
}

/// Unregister the popup's window class. Optional tidy-up for a DLL host that is about to
/// unload (call after every `CandidateWindow` is dropped; it fails harmlessly while any
/// window still exists). Not required for correctness: `CandidateWindow::new` replaces a
/// stale class left by an unloaded copy of the module.
pub fn unregister_class() -> bool {
    // SAFETY: plain Win32 call with our class name and module.
    unsafe { UnregisterClassW(CLASS_NAME, Some(module_handle())).is_ok() }
}

impl CandidateWindow {
    /// Create the (hidden) popup. `on_event` runs on this thread, from inside window
    /// messages; do not destroy the `CandidateWindow` from within it.
    pub fn new(on_event: impl FnMut(CandidateEvent) + 'static) -> Result<Self> {
        let hinst = module_handle();
        register_class(hinst)?;

        let (theme, palette) = sys::current_palette();
        let inner = Box::new(Inner {
            hwnd: Cell::new(HWND::default()),
            state: RefCell::new(State {
                view: CandidateView::default(),
                layout: Layout::default(),
                scale: 1.0,
                theme,
                palette,
                renderer: Renderer::new()?,
                anchor: None,
                prefer_above: false,
                hot: None,
                tracking_leave: false,
            }),
            callback: RefCell::new(Box::new(on_event)),
            shared: Arc::new(Mutex::new(Snapshot::default())),
        });

        // SAFETY: standard window creation. The window is per-monitor-v2 aware; the scope
        // restores the host thread's DPI context right after.
        let hwnd = unsafe {
            let _dpi = sys::DpiScope::per_monitor();
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinst),
                None,
            )?
        };
        inner.hwnd.set(hwnd);
        inner.shared.lock().unwrap_or_else(|e| e.into_inner()).hwnd = hwnd.0 as isize;
        // SAFETY: the Box outlives the window (Drop destroys it first).
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*inner as *const Inner as _) };

        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            inner.state.borrow_mut().scale = dpi as f32 / 96.0;
        }
        Ok(Self { inner })
    }

    pub fn hwnd(&self) -> HWND {
        self.inner.hwnd.get()
    }

    pub fn is_visible(&self) -> bool {
        // SAFETY: our window.
        unsafe { IsWindowVisible(self.hwnd()).as_bool() }
    }

    /// Replace the content. Resizes and repaints in place if visible. An empty list hides
    /// the popup.
    pub fn update(&self, view: &CandidateView) {
        let old_highlight;
        {
            let mut st = self.inner.state.borrow_mut();
            old_highlight = st.view.selected_row();
            let was_empty = st.view.visible_len() == 0;
            st.view = view.clone();
            // Normalise (clamp) the selection once, so later comparisons see stored values.
            if let Some(r) = view.selected_row() {
                st.view.select_row(Some(r));
            }
            st.hot = None;
            self.inner.relayout(&mut st);
            if view.visible_len() == 0 && !was_empty {
                drop(st);
                self.hide();
                return;
            }
        }
        if self.is_visible() {
            self.inner.reposition();
            self.inner.repaint_now();
            let sel = self.inner.state.borrow().view.selected_row();
            if let Some(i) = sel.filter(|_| sel != old_highlight) {
                uia::raise_highlight(&self.inner.shared, i);
            }
        }
    }

    /// Move only the highlight (e.g. arrow keys) to combined row `index`. Cheaper than
    /// `update`.
    pub fn set_highlight(&self, index: usize) {
        self.set_selected_row(Some(index));
    }

    /// Move the selection to combined row `row` (clamped); `None` clears the completion
    /// highlight (see `CandidateView::select_row`). Cheaper than `update`.
    pub fn set_selected_row(&self, row: Option<usize>) {
        let sel = {
            let mut st = self.inner.state.borrow_mut();
            let before = (st.view.highlighted, st.view.completion_highlight);
            st.view.select_row(row);
            if (st.view.highlighted, st.view.completion_highlight) == before {
                return;
            }
            let sel = st.view.selected_row();
            self.inner.shared_lock().highlighted = sel;
            sel
        };
        self.inner.repaint_now();
        if let Some(i) = sel {
            uia::raise_highlight(&self.inner.shared, i);
        }
    }

    /// Show the popup below `anchor` (caret / composition rectangle, physical screen
    /// pixels) without activating it. Safe to call repeatedly as the caret moves.
    pub fn show(&self, anchor: Rect) {
        self.show_at(anchor, false);
    }

    /// Like `show`, choosing the preferred side; the popup still flips to the other side
    /// when the preferred one has no room on the monitor's work area.
    pub fn show_at(&self, anchor: Rect, prefer_above: bool) {
        if self.inner.state.borrow().view.visible_len() == 0 {
            self.hide();
            return;
        }
        {
            let mut st = self.inner.state.borrow_mut();
            st.anchor = Some(anchor);
            st.prefer_above = prefer_above;
        }
        self.inner.reposition();
        let first = !self.is_visible();
        if first {
            // SAFETY: our window; SW_SHOWNOACTIVATE never takes focus.
            unsafe {
                let _ = ShowWindow(self.hwnd(), SW_SHOWNOACTIVATE);
            }
        }
        self.inner.repaint_now();
        if first {
            let sel = self.inner.state.borrow().view.selected_row();
            if let Some(i) = sel {
                uia::raise_highlight(&self.inner.shared, i);
            }
        }
    }

    pub fn hide(&self) {
        // SAFETY: our window.
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_HIDE);
        }
    }

    /// Currently highlighted row (clamped; 0 when nothing is selected), as last set by
    /// `update`/`set_highlight`.
    pub fn highlight(&self) -> usize {
        self.inner.state.borrow().view.highlight()
    }

    /// Selected combined row, or `None` when nothing is highlighted (completions only, none
    /// chosen yet).
    pub fn selected_row(&self) -> Option<usize> {
        self.inner.state.borrow().view.selected_row()
    }

    /// Re-read the light/dark/high-contrast setting. Done automatically on
    /// `WM_SETTINGCHANGE`; exposed for hosts that want to force it.
    pub fn refresh_theme(&self) {
        self.inner.refresh_theme();
    }

    pub fn theme(&self) -> Theme {
        self.inner.state.borrow().theme
    }
}

impl Drop for CandidateWindow {
    fn drop(&mut self) {
        let hwnd = self.inner.hwnd.get();
        // SAFETY: our window; USERDATA is cleared first so late messages see no state.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(hwnd);
        }
    }
}

impl Inner {
    fn shared_lock(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Recompute fonts, layout and the UIA snapshot for the current view and scale.
    fn relayout(&self, st: &mut State) {
        let scale = st.scale * st.view.size_factor();
        let _ = st.renderer.set_scale(scale);
        let (gw, lw) = st.renderer.measure(&st.view);
        st.layout = Layout::compute(&LayoutInput {
            rows: st.view.visible_len(),
            show_numbers: st.view.show_numbers,
            has_labels: st.view.has_labels(),
            footer: st.view.show_footer,
            divider_after: st.view.divider_after(),
            glyph_width: gw,
            label_width: lw,
            scale,
        });
        let mut s = self.shared_lock();
        let n = st.view.visible_len();
        s.items = st.view.rows().map(str::to_owned).collect();
        s.labels = (0..n).map(|i| st.view.label(i).to_owned()).collect();
        s.first_completion = st.view.alt_len();
        s.highlighted = st.view.selected_row();
        s.rows = st.layout.rows.iter().map(|r| r.row).collect();
    }

    /// Size (and, with an anchor, position) the window. Takes the monitor DPI into account
    /// so the very first show is already the right size.
    fn reposition(&self) {
        let (size, pos) = {
            let mut st = self.state.borrow_mut();
            let Some(anchor) = st.anchor else {
                let sz = st.layout.size;
                drop(st);
                self.set_pos(None, sz.w, sz.h);
                return;
            };
            let (work, mon_scale) = {
                let _dpi = sys::DpiScope::per_monitor();
                sys::monitor_for_rect(anchor)
            };
            if (mon_scale - st.scale).abs() > f32::EPSILON {
                st.scale = mon_scale;
                self.relayout(&mut st);
            }
            let size = st.layout.size;
            let p = place(&PlacementInput {
                anchor,
                size,
                work,
                rtl: true,
                gap: crate::layout::px(ANCHOR_GAP_DIP, st.scale),
                prefer_above: st.prefer_above,
            });
            (size, p.origin)
        };
        self.set_pos(Some(pos), size.w, size.h);
    }

    fn set_pos(&self, pos: Option<Point>, w: i32, h: i32) {
        let mut flags = SWP_NOACTIVATE;
        let p = pos.unwrap_or_default();
        if pos.is_none() {
            flags |= SWP_NOMOVE;
        } else {
            self.shared_lock().origin = (p.x, p.y);
        }
        // SAFETY: our window. May synchronously send WM_DPICHANGED; no borrow is held.
        unsafe {
            let _dpi = sys::DpiScope::per_monitor();
            let _ = SetWindowPos(self.hwnd.get(), Some(HWND_TOPMOST), p.x, p.y, w, h, flags);
        }
    }

    fn repaint_now(&self) {
        // SAFETY: our window.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
            let _ = UpdateWindow(self.hwnd.get());
        }
    }

    fn refresh_theme(&self) {
        let (theme, palette) = sys::current_palette();
        {
            let Ok(mut st) = self.state.try_borrow_mut() else {
                return;
            };
            if st.theme == theme && st.palette == palette {
                return;
            }
            st.theme = theme;
            st.palette = palette;
        }
        self.repaint_now();
    }

    fn emit(&self, ev: CandidateEvent) {
        if let Ok(mut cb) = self.callback.try_borrow_mut() {
            cb(ev);
        }
    }

    fn paint(&self) {
        let hwnd = self.hwnd.get();
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: standard paint cycle on our window.
        unsafe {
            BeginPaint(hwnd, &mut ps);
            if let Ok(mut guard) = self.state.try_borrow_mut() {
                let st = &mut *guard;
                let hi = st.view.selected_row();
                let res = st
                    .renderer
                    .paint(hwnd, &st.layout, &st.view, hi, st.hot, &st.palette);
                if res.is_err() {
                    st.renderer.drop_target();
                }
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }

    fn mouse_move(&self, x: i32, y: i32) {
        let hit = {
            let Ok(mut st) = self.state.try_borrow_mut() else {
                return;
            };
            if !st.tracking_leave {
                st.tracking_leave = true;
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.hwnd.get(),
                    dwHoverTime: 0,
                };
                // SAFETY: valid struct.
                unsafe {
                    let _ = TrackMouseEvent(&mut t);
                }
            }
            let hit = st.layout.hit_test(x, y);
            if hit == st.hot {
                return;
            }
            st.hot = hit;
            hit
        };
        self.repaint_now();
        if let Some(i) = hit {
            self.emit(CandidateEvent::Hover(i));
        }
    }

    fn mouse_leave(&self) {
        let had_hot = {
            let Ok(mut st) = self.state.try_borrow_mut() else {
                return;
            };
            st.tracking_leave = false;
            st.hot.take().is_some()
        };
        if had_hot {
            self.repaint_now();
        }
    }

    fn click(&self, x: i32, y: i32) {
        let hit = self
            .state
            .try_borrow()
            .ok()
            .and_then(|st| st.layout.hit_test(x, y));
        if let Some(i) = hit {
            self.emit(CandidateEvent::Click(i));
        }
    }

    fn dpi_changed(&self) {
        // The anchor drives sizing; reposition() re-derives the scale from the monitor.
        let has_anchor = self
            .state
            .try_borrow()
            .map(|s| s.anchor.is_some())
            .unwrap_or(false);
        if has_anchor {
            self.reposition();
            self.repaint_now();
        }
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // A panic must never unwind into the host's message loop (that aborts the host process).
    std::panic::catch_unwind(|| handle_message(hwnd, msg, wp, lp)).unwrap_or(LRESULT(0))
}

fn handle_message(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: USERDATA is either 0 or a pointer to the live boxed `Inner`.
    let inner = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Inner).as_ref() };
    let Some(inner) = inner else {
        // SAFETY: default handling.
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    };
    let (x, y) = (
        (lp.0 & 0xFFFF) as i16 as i32,
        ((lp.0 >> 16) & 0xFFFF) as i16 as i32,
    );
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            inner.paint();
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            inner.mouse_move(x, y);
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            inner.mouse_leave();
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            inner.click(x, y);
            return LRESULT(0);
        }
        WM_PIME_SELECT => {
            inner.emit(CandidateEvent::Click(wp.0));
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            inner.dpi_changed();
            return LRESULT(0);
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => inner.refresh_theme(),
        WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => {
            let provider = uia::root_provider(&inner.shared);
            // SAFETY: provider is a live COM object.
            return unsafe { UiaReturnRawElementProvider(hwnd, wp, lp, &provider) };
        }
        WM_DESTROY => {
            // SAFETY: documented way to disconnect the provider on teardown.
            unsafe {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
            }
        }
        WM_NCDESTROY => {
            // SAFETY: stop further dispatch into `Inner`.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        }
        _ => {}
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
