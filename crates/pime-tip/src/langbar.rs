//! Stream C. Language-bar / input-indicator button. PINNED interface:
//!
//! ```ignore
//! pub struct LangBar { .. }
//! impl LangBar {
//!     pub fn new(thread_mgr: &ITfThreadMgr, client_id: u32) -> windows::core::Result<Self>;
//!     pub fn set_open(&self, open: bool);                       // Persian (ف) / Latin (A)
//!     pub fn set_error_badge(&self, text: Option<String>);      // friendly config problem tooltip
//!     pub fn shutdown(&self);
//! }
//! ```
//!
//! Additive: [`LangBar::set_on_recheck`], [`LangBar::is_open`], [`LangBar::item_guid`].
//!
//! The button is registered with `guidItem = GUID_LBI_INPUTMODE`, which is what the Windows 8+
//! input indicator shows; if `AddItem` refuses that, it falls back to our own
//! `pime_ids::GUID_LANGBAR_INPUT_MODE` (floating language bar only). Left click toggles
//! `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE`; right click shows a menu (Settings..., Fix settings
//! problem... while a badge is set, Check settings again). The icon is drawn at runtime with
//! GDI. Every COM method runs inside [`guard`] (windows-rs does not catch panics). Single
//! threaded: created, used and shut down on the text service's UI thread.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Longest tooltip (in characters) we hand to TSF.
pub const TOOLTIP_MAX_CHARS: usize = 250;
/// `TF_LANGBARITEMINFO::szDescription` (TF_LBI_DESC_MAXLEN = 32, NUL included).
pub const DESCRIPTION: &str = "Persian Phonetic input mode";

/// Menu command ids (also used by `ITfMenu` / `OnMenuSelect`).
pub const MENU_SETTINGS: u32 = 1;
pub const MENU_FIX: u32 = 2;
pub const MENU_RECHECK: u32 = 3;

/// Collapses whitespace and cuts `s` to at most `max` characters, preferring a word boundary,
/// with a trailing ellipsis when shortened.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    if max == 0 {
        return String::new();
    }
    let cut: String = flat.chars().take(max - 1).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if cut[..i].chars().count() >= max * 2 / 3 => &cut[..i],
        _ => cut.as_str(),
    };
    format!("{}\u{2026}", cut.trim_end())
}

/// Which icon to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IconKind {
    pub open: bool,
    pub warning: bool,
}

/// What the button shows; pure so it can be tested without TSF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ButtonState {
    pub open: bool,
    pub badge: Option<String>,
}

impl Default for ButtonState {
    fn default() -> Self {
        Self {
            open: true,
            badge: None,
        }
    }
}

const UPDATE_ALL: u32 = windows::Win32::UI::TextServices::TF_LBI_ICON
    | windows::Win32::UI::TextServices::TF_LBI_TEXT
    | windows::Win32::UI::TextServices::TF_LBI_TOOLTIP;

impl ButtonState {
    pub fn text(&self) -> &'static str {
        if self.open {
            "\u{0641}"
        } else {
            "A"
        }
    }

    pub fn tooltip(&self) -> String {
        let mode = if self.open {
            "Persian Phonetic: typing Persian (\u{0641}). Click to type Latin letters."
        } else {
            "Persian Phonetic: typing Latin letters (A). Click to type Persian."
        };
        match &self.badge {
            Some(b) => truncate_chars(&format!("{b} {mode}"), TOOLTIP_MAX_CHARS),
            None => mode.to_string(),
        }
    }

    pub fn icon(&self) -> IconKind {
        IconKind {
            open: self.open,
            warning: self.badge.is_some(),
        }
    }

    /// `(id, label, enabled)`. `can_launch`: see [`launch_allowed`].
    pub fn menu(&self, can_launch: bool) -> Vec<(u32, &'static str, bool)> {
        let settings = if can_launch {
            "Settings..."
        } else {
            "Settings... (open from the Start menu)"
        };
        let mut m = vec![(MENU_SETTINGS, settings, can_launch)];
        if self.badge.is_some() {
            m.push((MENU_FIX, "Fix settings problem...", can_launch));
        }
        m.push((MENU_RECHECK, "Check settings again", true));
        m
    }

    /// Returns the `TF_LBI_*` flags to send to sinks (0 = nothing changed).
    pub fn set_open(&mut self, open: bool) -> u32 {
        if self.open == open {
            return 0;
        }
        self.open = open;
        UPDATE_ALL
    }

    /// Blank text clears the badge. Returns the `TF_LBI_*` flags to send (0 = unchanged).
    pub fn set_badge(&mut self, text: Option<String>) -> u32 {
        let text = text
            .map(|t| truncate_chars(&t, TOOLTIP_MAX_CHARS))
            .filter(|t| !t.is_empty());
        if self.badge == text {
            return 0;
        }
        self.badge = text;
        UPDATE_ALL
    }
}

const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x2000;

/// Whether `pime-settings.exe` may be started from this host with `CreateProcessW`.
///
/// The child inherits the host's token. From an AppContainer (UWP / packaged apps) it could
/// not read `%APPDATA%`; from an elevated or low-integrity host it would run elevated (with
/// over-the-shoulder UAC, as another user writing another profile) or sandboxed. Starting it
/// unelevated from there needs the shell (`IShellDispatch2`) and still fails in AppContainers,
/// so instead the menu item is greyed and points at the Start-menu shortcut. Allowed only
/// when the host runs at the shell's integrity band (so UAC-off machines still work).
/// `shell_il` is `None` when explorer is not found or not readable: medium is assumed.
pub(crate) fn launch_allowed(
    app_container: bool,
    host_il: Option<u32>,
    shell_il: Option<u32>,
) -> bool {
    let band = |rid: u32| rid & 0xF000; // medium-plus (0x2100) counts as medium
    let shell = shell_il.unwrap_or(SECURITY_MANDATORY_MEDIUM_RID);
    !app_container && host_il.is_some_and(|h| band(h) == band(shell))
}

/// Command line for `pime-settings.exe` (the exe path quoted, then the arguments).
pub(crate) fn settings_command_line(exe: &std::path::Path, args: &[&str]) -> String {
    let mut s = format!("\"{}\"", exe.display());
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// Straight-alpha ARGB icon pixels from a white-on-black glyph rendering: coverage becomes
/// alpha, colour becomes `fg` (0xRRGGBB); a warning variant adds an amber dot bottom-right.
pub(crate) fn compose_icon_pixels(px: &mut [u32], n: usize, fg: u32, warning: bool) {
    for p in px.iter_mut() {
        let cov = ((*p >> 16) & 0xFF).max((*p >> 8) & 0xFF).max(*p & 0xFF);
        *p = if cov == 0 {
            0
        } else {
            (cov << 24) | (fg & 0x00FF_FFFF)
        };
    }
    if !warning || n < 8 {
        return;
    }
    let r = (n as f32 / 4.0).max(3.0);
    let c = n as f32 - r - 0.5;
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let i = y * n + x;
            if d <= r - 1.0 {
                px[i] = 0xFFFF_B900; // amber
            } else if d <= r + 0.5 {
                px[i] = 0xFF20_2020; // dark ring keeps the dot visible on any taskbar colour
            }
        }
    }
}

#[cfg(windows)]
pub use win::LangBar;

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::path::PathBuf;
    use windows::core::{
        implement, ComObject, Interface, Ref, Result, BOOL, BSTR, GUID, HSTRING, PCWSTR, PWSTR,
    };
    use windows::Win32::Foundation::{
        CloseHandle, COLORREF, E_ACCESSDENIED, E_FAIL, E_INVALIDARG, HANDLE, HMODULE, POINT, RECT,
    };
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject,
        DrawTextW, GdiFlush, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
        ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS,
        DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
        FW_SEMIBOLD, OUT_DEFAULT_PRECIS, TRANSPARENT,
    };
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TokenIsAppContainer, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    use windows::Win32::System::Ole::{CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows::Win32::System::Threading::{
        CreateProcessW, GetCurrentProcess, OpenProcess, OpenProcessToken, CREATE_NO_WINDOW,
        PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::TextServices::{
        ITfCompartmentMgr, ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItemMgr,
        ITfLangBarItemSink, ITfLangBarItem_Impl, ITfMenu, ITfSource, ITfSource_Impl, ITfThreadMgr,
        TfLBIClick, GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, GUID_LBI_INPUTMODE, TF_LANGBARITEMINFO,
        TF_LBI_CLK_LEFT, TF_LBI_CLK_RIGHT, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_SHOWNINTRAY,
        TF_LBMENUF_GRAYED,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, AppendMenuW, CreateIconIndirect, CreatePopupMenu,
        CreateWindowExW, DestroyMenu, DestroyWindow, GetShellWindow, GetSystemMetrics,
        GetWindowThreadProcessId, TrackPopupMenuEx, HICON, ICONINFO, MF_GRAYED, MF_STRING,
        SM_CXSMICON, TPMPARAMS, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_VERTICAL,
        WS_EX_TOOLWINDOW, WS_POPUP,
    };

    use crate::exports::{guard, ObjToken};

    #[implement(ITfLangBarItemButton, ITfSource)]
    pub(crate) struct Button {
        // Never borrowed across a call out to TSF or a sink: they call straight back in.
        state: RefCell<ButtonState>,
        sinks: RefCell<Vec<(u32, ITfLangBarItemSink)>>,
        next_cookie: Cell<u32>,
        item_guid: Cell<GUID>,
        thread_mgr: RefCell<Option<ITfThreadMgr>>,
        client_id: u32,
        on_recheck: RefCell<Option<Rc<dyn Fn()>>>,
        _tok: ObjToken,
    }

    impl Button {
        pub(crate) fn new(thread_mgr: Option<ITfThreadMgr>, client_id: u32) -> Self {
            Self {
                state: RefCell::new(ButtonState::default()),
                sinks: RefCell::new(Vec::new()),
                next_cookie: Cell::new(1),
                item_guid: Cell::new(GUID_LBI_INPUTMODE),
                thread_mgr: RefCell::new(thread_mgr),
                client_id,
                on_recheck: RefCell::new(None),
                _tok: ObjToken::new(),
            }
        }

        fn notify(&self, flags: u32) {
            if flags == 0 {
                return;
            }
            let sinks: Vec<ITfLangBarItemSink> =
                self.sinks.borrow().iter().map(|(_, s)| s.clone()).collect();
            for s in sinks {
                // SAFETY: plain COM call on a sink TSF gave us.
                let _ = unsafe { s.OnUpdate(flags) };
            }
        }

        fn set_open(&self, open: bool) {
            let f = self.state.borrow_mut().set_open(open);
            self.notify(f);
        }

        fn set_badge(&self, text: Option<String>) {
            let f = self.state.borrow_mut().set_badge(text);
            self.notify(f);
        }

        fn toggle(&self) -> Result<()> {
            let open = !self.state.borrow().open;
            let tm = self.thread_mgr.borrow().clone();
            if let Some(tm) = tm {
                let cm: ITfCompartmentMgr = tm.cast()?;
                // SAFETY: COM calls on live TSF objects. SetValue fires the shell's compartment
                // sink synchronously, which may call set_open on us: no borrow is held here.
                unsafe {
                    let c = cm.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)?;
                    c.SetValue(self.client_id, &VARIANT::from(i32::from(open)))?;
                }
            }
            self.set_open(open);
            Ok(())
        }

        fn run_command(&self, id: u32) -> Result<()> {
            match id {
                MENU_SETTINGS | MENU_FIX if !settings_launch_allowed() => {
                    Err(E_ACCESSDENIED.into())
                }
                MENU_SETTINGS => launch_settings(&[]),
                MENU_FIX => launch_settings(&["--show-errors"]),
                MENU_RECHECK => {
                    crate::reload::request_recheck();
                    let cb = self.on_recheck.borrow().clone();
                    if let Some(cb) = cb {
                        cb();
                    }
                    Ok(())
                }
                _ => Err(E_INVALIDARG.into()),
            }
        }

        fn show_menu(&self, pt: POINT, area: *const RECT) -> Result<()> {
            let items = self.state.borrow().menu(settings_launch_allowed());
            // SAFETY: Win32 calls on objects created and destroyed here; `area` comes from TSF
            // and is only read when non-null.
            let cmd = unsafe {
                let menu = CreatePopupMenu()?;
                for (id, label, enabled) in items {
                    let flags = if enabled {
                        MF_STRING
                    } else {
                        MF_STRING | MF_GRAYED
                    };
                    let _ = AppendMenuW(menu, flags, id as usize, &HSTRING::from(label));
                }
                // TrackPopupMenuEx needs an owner window on this thread; a hidden STATIC popup
                // needs no class registration (nothing left behind when the DLL unloads).
                let owner = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    &HSTRING::from("STATIC"),
                    &HSTRING::new(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                );
                let cmd = match owner {
                    Ok(owner) => {
                        let mut tpm = TPMPARAMS {
                            cbSize: std::mem::size_of::<TPMPARAMS>() as u32,
                            ..Default::default()
                        };
                        if !area.is_null() {
                            tpm.rcExclude = *area;
                        }
                        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_VERTICAL;
                        let r = TrackPopupMenuEx(menu, flags.0, pt.x, pt.y, owner, Some(&tpm));
                        let _ = DestroyWindow(owner);
                        r.0
                    }
                    Err(_) => 0,
                };
                let _ = DestroyMenu(menu);
                cmd
            };
            if cmd > 0 {
                self.run_command(cmd as u32)?;
            }
            Ok(())
        }
    }

    impl ITfLangBarItem_Impl for Button_Impl {
        fn GetInfo(&self, pinfo: *mut TF_LANGBARITEMINFO) -> Result<()> {
            guard(|| {
                if pinfo.is_null() {
                    return Err(E_INVALIDARG.into());
                }
                let mut info = TF_LANGBARITEMINFO {
                    clsidService: GUID::from_u128(pime_ids::CLSID_TEXT_SERVICE),
                    guidItem: self.item_guid.get(),
                    dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
                    ulSort: 0,
                    szDescription: [0; 32],
                };
                for (d, s) in info
                    .szDescription
                    .iter_mut()
                    .zip(DESCRIPTION.encode_utf16().take(31))
                {
                    *d = s;
                }
                // SAFETY: checked non-null; TSF passes a writable struct.
                unsafe { pinfo.write(info) };
                Ok(())
            })
        }

        fn GetStatus(&self) -> Result<u32> {
            guard(|| Ok(0))
        }

        fn Show(&self, _fshow: BOOL) -> Result<()> {
            guard(|| Ok(()))
        }

        fn GetTooltipString(&self) -> Result<BSTR> {
            guard(|| Ok(BSTR::from(self.state.borrow().tooltip())))
        }
    }

    impl ITfLangBarItemButton_Impl for Button_Impl {
        fn OnClick(&self, click: TfLBIClick, pt: &POINT, prcarea: *const RECT) -> Result<()> {
            guard(|| match click {
                TF_LBI_CLK_LEFT => self.toggle(),
                TF_LBI_CLK_RIGHT => self.show_menu(*pt, prcarea),
                _ => Ok(()),
            })
        }

        fn InitMenu(&self, pmenu: Ref<ITfMenu>) -> Result<()> {
            guard(|| {
                let menu = pmenu.ok()?;
                let items = self.state.borrow().menu(settings_launch_allowed());
                for (id, label, enabled) in items {
                    let w: Vec<u16> = label.encode_utf16().collect();
                    let flags = if enabled { 0 } else { TF_LBMENUF_GRAYED };
                    // SAFETY: COM call; null bitmaps and no submenu out-pointer are allowed.
                    unsafe {
                        menu.AddMenuItem(
                            id,
                            flags,
                            Default::default(),
                            Default::default(),
                            &w,
                            std::ptr::null_mut(),
                        )?
                    };
                }
                Ok(())
            })
        }

        fn OnMenuSelect(&self, wid: u32) -> Result<()> {
            guard(|| self.run_command(wid))
        }

        fn GetIcon(&self) -> Result<HICON> {
            // TSF owns (and destroys) the icon we return, so draw a fresh one each time.
            guard(|| draw_icon(self.state.borrow().icon()))
        }

        fn GetText(&self) -> Result<BSTR> {
            guard(|| Ok(BSTR::from(self.state.borrow().text())))
        }
    }

    impl ITfSource_Impl for Button_Impl {
        fn AdviseSink(&self, riid: *const GUID, punk: Ref<windows::core::IUnknown>) -> Result<u32> {
            guard(|| {
                // SAFETY: TSF passes a valid IID pointer; null is rejected.
                if riid.is_null() || unsafe { *riid } != ITfLangBarItemSink::IID {
                    return Err(CONNECT_E_CANNOTCONNECT.into());
                }
                let sink: ITfLangBarItemSink = punk.ok()?.cast()?;
                let cookie = self.next_cookie.get();
                self.next_cookie.set(cookie.wrapping_add(1).max(1));
                self.sinks.borrow_mut().push((cookie, sink));
                Ok(cookie)
            })
        }

        fn UnadviseSink(&self, dwcookie: u32) -> Result<()> {
            guard(|| {
                let removed = {
                    let mut sinks = self.sinks.borrow_mut();
                    let before = sinks.len();
                    let kept: Vec<_> = sinks.drain(..).filter(|(c, _)| *c != dwcookie).collect();
                    *sinks = kept;
                    before != sinks.len()
                };
                if removed {
                    Ok(())
                } else {
                    Err(CONNECT_E_NOCONNECTION.into())
                }
            })
        }
    }

    /// The language-bar button owned by one text-service instance.
    pub struct LangBar {
        button: ComObject<Button>,
        mgr: RefCell<Option<ITfLangBarItemMgr>>,
    }

    impl LangBar {
        /// Adds the button to the thread's language bar (call from `ActivateEx`).
        pub fn new(thread_mgr: &ITfThreadMgr, client_id: u32) -> Result<Self> {
            guard(|| {
                let mgr: ITfLangBarItemMgr = thread_mgr.cast()?;
                let button = ComObject::new(Button::new(Some(thread_mgr.clone()), client_id));
                let item: ITfLangBarItemButton = button.to_interface();
                // SAFETY: COM calls on the thread's own language-bar manager.
                if unsafe { mgr.AddItem(&item) }.is_err() {
                    button
                        .item_guid
                        .set(GUID::from_u128(pime_ids::GUID_LANGBAR_INPUT_MODE));
                    if let Err(e) = unsafe { mgr.AddItem(&item) } {
                        button.thread_mgr.borrow_mut().take();
                        return Err(e);
                    }
                }
                Ok(Self {
                    button,
                    mgr: RefCell::new(Some(mgr)),
                })
            })
        }

        /// A button not attached to any TSF manager (tests).
        pub(crate) fn detached() -> Self {
            Self {
                button: ComObject::new(Button::new(None, 0)),
                mgr: RefCell::new(None),
            }
        }

        /// Persian (ف, open) or Latin (A, closed). Call from the open/close compartment sink.
        pub fn set_open(&self, open: bool) {
            let _ = guard(|| {
                self.button.set_open(open);
                Ok(())
            });
        }

        /// `Some(text)` shows the warning icon and puts `text` (truncated) in the tooltip and a
        /// "Fix settings problem..." menu item; `None` clears it.
        pub fn set_error_badge(&self, text: Option<String>) {
            let _ = guard(|| {
                self.button.set_badge(text);
                Ok(())
            });
        }

        /// Called after "Check settings again" (and after `reload::request_recheck`). The shell
        /// should run `ConfigHandle::reload_if_changed` and refresh session + badge.
        pub fn set_on_recheck(&self, f: impl Fn() + 'static) {
            *self.button.on_recheck.borrow_mut() = Some(Rc::new(f));
        }

        pub fn is_open(&self) -> bool {
            self.button.state.borrow().open
        }

        /// `GUID_LBI_INPUTMODE`, or `GUID_LANGBAR_INPUT_MODE` if the first `AddItem` failed.
        pub fn item_guid(&self) -> GUID {
            self.button.item_guid.get()
        }

        pub(crate) fn item(&self) -> ITfLangBarItemButton {
            self.button.to_interface()
        }

        /// Removes the button (call from `Deactivate`). Idempotent; also run on drop.
        pub fn shutdown(&self) {
            let _ = guard(|| {
                let mgr = self.mgr.borrow_mut().take();
                if let Some(mgr) = mgr {
                    let item: ITfLangBarItemButton = self.button.to_interface();
                    // SAFETY: COM call; failure is irrelevant at shutdown.
                    let _ = unsafe { mgr.RemoveItem(&item) };
                }
                self.button.thread_mgr.borrow_mut().take();
                self.button.on_recheck.borrow_mut().take();
                let sinks = std::mem::take(&mut *self.button.sinks.borrow_mut());
                drop(sinks);
                Ok(())
            });
        }
    }

    impl Drop for LangBar {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn module_path() -> Option<PathBuf> {
        let mut h = HMODULE::default();
        // SAFETY: any address inside this module identifies it; no refcount change.
        unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(module_path as *const () as *const u16),
                &mut h,
            )
            .ok()?;
            let mut buf = vec![0u16; 32768];
            let n = GetModuleFileNameW(Some(h), &mut buf) as usize;
            if n == 0 || n >= buf.len() {
                return None;
            }
            Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
        }
    }

    /// `pime-settings.exe` next to this DLL (or next to the test exe in tests).
    pub(crate) fn settings_exe() -> Option<PathBuf> {
        Some(module_path()?.parent()?.join("pime-settings.exe"))
    }

    /// `(is AppContainer, integrity RID)` of a process token.
    fn token_facts(process: HANDLE) -> Option<(bool, u32)> {
        // SAFETY: token queries into correctly sized locals; the SID pointer points into `buf`,
        // which outlives its use; the token handle is closed below.
        unsafe {
            let mut tok = HANDLE::default();
            OpenProcessToken(process, TOKEN_QUERY, &mut tok).ok()?;
            let facts = (|| {
                let mut len = 0u32;
                let mut ac = 0u32;
                GetTokenInformation(
                    tok,
                    TokenIsAppContainer,
                    Some(&mut ac as *mut u32 as *mut c_void),
                    std::mem::size_of::<u32>() as u32,
                    &mut len,
                )
                .ok()?;
                // TOKEN_MANDATORY_LABEL plus its SID; u64 for alignment.
                let mut buf = [0u64; 16];
                GetTokenInformation(
                    tok,
                    TokenIntegrityLevel,
                    Some(buf.as_mut_ptr() as *mut c_void),
                    std::mem::size_of_val(&buf) as u32,
                    &mut len,
                )
                .ok()?;
                let sid = (*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
                let count = *GetSidSubAuthorityCount(sid);
                if count == 0 {
                    return None;
                }
                Some((ac != 0, *GetSidSubAuthority(sid, u32::from(count - 1))))
            })();
            let _ = CloseHandle(tok);
            facts
        }
    }

    fn shell_integrity() -> Option<u32> {
        // SAFETY: plain queries; the process handle is closed below.
        unsafe {
            let shell = GetShellWindow();
            if shell.is_invalid() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(shell, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let facts = token_facts(h);
            let _ = CloseHandle(h);
            facts.map(|f| f.1)
        }
    }

    /// [`launch_allowed`] for this process; computed once (only on a menu click).
    pub(crate) fn settings_launch_allowed() -> bool {
        static ALLOWED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ALLOWED.get_or_init(|| {
            // SAFETY: pseudo-handle, never closed.
            let host = token_facts(unsafe { GetCurrentProcess() });
            launch_allowed(
                host.is_none_or(|h| h.0),
                host.map(|h| h.1),
                shell_integrity(),
            )
        })
    }

    /// Starts `pime-settings.exe` without a console window (it is a console-subsystem exe).
    fn launch_settings(args: &[&str]) -> Result<()> {
        let exe = settings_exe()
            .filter(|p| p.is_file())
            .ok_or(windows::core::Error::from(E_FAIL))?;
        let dir = exe
            .parent()
            .map(|d| HSTRING::from(d.as_os_str()))
            .unwrap_or_default();
        let mut cmd: Vec<u16> = settings_command_line(&exe, args)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        // SAFETY: all buffers outlive the call; handles are closed below.
        unsafe {
            CreateProcessW(
                &HSTRING::from(exe.as_os_str()),
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW,
                None,
                &dir,
                &si,
                &mut pi,
            )?;
            // Our host usually holds the foreground; let the settings window take it.
            let _ = AllowSetForegroundWindow(pi.dwProcessId);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        }
        Ok(())
    }

    /// Glyph colour: dark on a light taskbar, else white (Windows 10 default taskbar is dark).
    fn glyph_colour() -> u32 {
        let mut v: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: plain registry read into a u32.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
                &HSTRING::from("SystemUsesLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut v as *mut u32 as *mut c_void),
                Some(&mut size),
            )
        };
        if r.is_ok() && v != 0 {
            0x000000
        } else {
            0xFFFFFF
        }
    }

    /// Draws "ف" or "A" (plus a warning dot) into a new HICON. The caller owns the icon.
    pub(crate) fn draw_icon(kind: IconKind) -> Result<HICON> {
        // SAFETY: GDI objects are created, used and released within this function.
        unsafe {
            let n = GetSystemMetrics(SM_CXSMICON).clamp(16, 64);
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            if dc.is_invalid() {
                return Err(E_FAIL.into());
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: n,
                    biHeight: -n, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let color = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    return Err(E_FAIL.into());
                }
            };
            let len = (n * n) as usize;
            let px = std::slice::from_raw_parts_mut(bits as *mut u32, len);
            px.fill(0);
            let old_bmp = SelectObject(dc, color.into());
            let font = CreateFontW(
                -(n * 7 / 8),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                &HSTRING::from("Segoe UI"),
            );
            let old_font = SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let mut text: Vec<u16> = (if kind.open { "\u{0641}" } else { "A" })
                .encode_utf16()
                .collect();
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: n,
                bottom: n,
            };
            DrawTextW(
                dc,
                &mut text,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            let _ = GdiFlush();
            compose_icon_pixels(px, n as usize, glyph_colour(), kind.warning);
            SelectObject(dc, old_font);
            SelectObject(dc, old_bmp);
            let _ = DeleteObject(font.into());
            let _ = DeleteDC(dc);

            // All-zero AND mask: with a 32-bit colour bitmap the alpha channel decides.
            let stride = (((n + 15) / 16) * 2) as usize;
            let zeros = vec![0u8; stride * n as usize];
            let mask = CreateBitmap(n, n, 1, 1, Some(zeros.as_ptr() as *const c_void));
            let info = ICONINFO {
                fIcon: true.into(),
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            };
            let icon = CreateIconIndirect(&info);
            let _ = DeleteObject(mask.into());
            let _ = DeleteObject(color.into());
            icon
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::Graphics::Gdi::{GetDIBits, GetObjectW, BITMAP};
        use windows::Win32::UI::TextServices::{
            ITfLangBarItem, ITfLangBarItemSink_Impl, TF_LBI_ICON,
        };
        use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo};

        #[implement(ITfLangBarItemSink)]
        struct Sink {
            seen: Seen,
            item: RefCell<Option<ITfLangBarItemButton>>,
        }

        impl ITfLangBarItemSink_Impl for Sink_Impl {
            fn OnUpdate(&self, flags: u32) -> Result<()> {
                // Like TSF: read the new text back from inside the notification.
                let text = match self.item.borrow().as_ref() {
                    Some(i) => unsafe { i.GetText() }?.to_string(),
                    None => String::new(),
                };
                self.seen.borrow_mut().push((flags, text));
                Ok(())
            }
        }

        type Seen = Rc<RefCell<Vec<(u32, String)>>>;

        fn attach(bar: &LangBar) -> (Seen, u32) {
            let seen = Rc::new(RefCell::new(Vec::new()));
            let sink: ITfLangBarItemSink = Sink {
                seen: seen.clone(),
                item: RefCell::new(Some(bar.item())),
            }
            .into();
            let src: ITfSource = bar.item().cast().unwrap();
            let cookie = unsafe { src.AdviseSink(&ITfLangBarItemSink::IID, &sink) }.unwrap();
            (seen, cookie)
        }

        #[test]
        fn info_text_and_tooltip() {
            let bar = LangBar::detached();
            let item = bar.item();
            let mut info = TF_LANGBARITEMINFO::default();
            unsafe { item.GetInfo(&mut info) }.unwrap();
            assert_eq!(info.guidItem, GUID_LBI_INPUTMODE);
            assert_eq!(
                info.clsidService,
                GUID::from_u128(pime_ids::CLSID_TEXT_SERVICE)
            );
            assert_ne!(info.dwStyle & TF_LBI_STYLE_SHOWNINTRAY, 0);
            let desc = String::from_utf16_lossy(&info.szDescription);
            assert!(desc.starts_with(DESCRIPTION));
            assert!(unsafe { item.GetInfo(std::ptr::null_mut()) }.is_err());
            assert_eq!(unsafe { item.GetText() }.unwrap().to_string(), "\u{0641}");
            bar.set_open(false);
            assert_eq!(unsafe { item.GetText() }.unwrap().to_string(), "A");
            assert!(unsafe { item.GetTooltipString() }
                .unwrap()
                .to_string()
                .contains("Latin"));
            assert_eq!(unsafe { item.GetStatus() }.unwrap(), 0);
            // A plain langbar item QI works too (TSF asks for both).
            item.cast::<ITfLangBarItem>().unwrap();
        }

        #[test]
        fn sinks_get_updates_and_can_reenter() {
            let bar = LangBar::detached();
            let (seen, cookie) = attach(&bar);
            bar.set_open(true); // unchanged: no update
            assert!(seen.borrow().is_empty());
            bar.set_open(false);
            bar.set_error_badge(Some("Your settings file has a problem.".into()));
            bar.set_error_badge(Some("Your settings file has a problem.".into()));
            bar.set_error_badge(None);
            let s = seen.borrow().clone();
            assert_eq!(s.len(), 3, "{s:?}");
            assert!(s.iter().all(|(f, _)| f & TF_LBI_ICON != 0));
            assert_eq!(s[0].1, "A");
            let src: ITfSource = bar.item().cast().unwrap();
            unsafe { src.UnadviseSink(cookie) }.unwrap();
            assert!(unsafe { src.UnadviseSink(cookie) }.is_err());
            bar.set_open(true);
            assert_eq!(seen.borrow().len(), 3);
            // Break the sink -> item cycle.
            bar.shutdown();
        }

        #[test]
        fn wrong_sink_iid_is_refused() {
            let bar = LangBar::detached();
            let src: ITfSource = bar.item().cast().unwrap();
            let sink: ITfLangBarItemSink = Sink {
                seen: Default::default(),
                item: RefCell::new(None),
            }
            .into();
            let r = unsafe { src.AdviseSink(&ITfLangBarItem::IID, &sink) };
            assert_eq!(r.unwrap_err().code(), CONNECT_E_CANNOTCONNECT);
        }

        #[test]
        fn detached_left_click_toggles_and_recheck_runs_callback() {
            let bar = LangBar::detached();
            let item = bar.item();
            unsafe { item.OnClick(TF_LBI_CLK_LEFT, POINT::default(), std::ptr::null()) }.unwrap();
            assert!(!bar.is_open());
            let hits = Rc::new(Cell::new(0));
            let h = hits.clone();
            bar.set_on_recheck(move || h.set(h.get() + 1));
            let before = crate::reload::signal_counter().load(std::sync::atomic::Ordering::SeqCst);
            unsafe { item.OnMenuSelect(MENU_RECHECK) }.unwrap();
            assert_eq!(hits.get(), 1);
            assert!(
                crate::reload::signal_counter().load(std::sync::atomic::Ordering::SeqCst) > before
            );
            assert!(unsafe { item.OnMenuSelect(99) }.is_err());
        }

        #[test]
        fn own_token_is_readable() {
            // SAFETY: pseudo-handle.
            let (ac, rid) = token_facts(unsafe { GetCurrentProcess() }).unwrap();
            assert!(!ac, "tests do not run in an AppContainer");
            assert!(rid >= 0x1000, "at least low integrity: {rid:#x}");
            // Value depends on whether the test shell is elevated; it must just not panic.
            let _ = settings_launch_allowed();
        }

        #[test]
        fn live_button_blocks_dll_unload() {
            let bar = LangBar::detached();
            assert_eq!(
                crate::exports::DllCanUnloadNow(),
                windows::Win32::Foundation::S_FALSE
            );
            drop(bar);
        }

        #[test]
        fn settings_exe_is_next_to_module() {
            let exe = settings_exe().unwrap();
            assert_eq!(exe.file_name().unwrap(), "pime-settings.exe");
            let me = std::env::current_exe().unwrap();
            assert_eq!(exe.parent().unwrap(), me.parent().unwrap());
        }

        #[test]
        fn icons_are_drawn_with_alpha() {
            for open in [true, false] {
                for warning in [false, true] {
                    let icon = draw_icon(IconKind { open, warning }).unwrap();
                    let mut ii = ICONINFO::default();
                    unsafe { GetIconInfo(icon, &mut ii) }.unwrap();
                    let mut bm = BITMAP::default();
                    unsafe {
                        GetObjectW(
                            ii.hbmColor.into(),
                            std::mem::size_of::<BITMAP>() as i32,
                            Some(&mut bm as *mut BITMAP as *mut c_void),
                        )
                    };
                    let n = bm.bmWidth;
                    assert!(n >= 16);
                    let mut bmi = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: n,
                            biHeight: -n,
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    let mut px = vec![0u32; (n * n) as usize];
                    let dc = unsafe { CreateCompatibleDC(None) };
                    let lines = unsafe {
                        GetDIBits(
                            dc,
                            ii.hbmColor,
                            0,
                            n as u32,
                            Some(px.as_mut_ptr() as *mut c_void),
                            &mut bmi,
                            DIB_RGB_COLORS,
                        )
                    };
                    unsafe {
                        let _ = DeleteDC(dc);
                        let _ = DeleteObject(ii.hbmColor.into());
                        let _ = DeleteObject(ii.hbmMask.into());
                        DestroyIcon(icon).unwrap();
                    }
                    assert_eq!(lines, n);
                    let opaque = px.iter().filter(|p| *p >> 24 > 0x80).count();
                    assert!(opaque > 8, "glyph drawn ({open}, {warning}): {opaque}");
                    let amber = px.iter().filter(|p| **p == 0xFFFF_B900).count();
                    assert_eq!(amber > 0, warning);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation() {
        assert_eq!(truncate_chars("  a\n b\t c ", 10), "a b c");
        let long = "word ".repeat(30);
        let t = truncate_chars(&long, 40);
        assert!(t.chars().count() <= 40, "{t}");
        assert!(t.ends_with("word\u{2026}"), "{t}");
        // No space to cut at: hard cut, still within the limit, multibyte safe.
        let fa = "\u{0641}".repeat(50);
        let t = truncate_chars(&fa, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('\u{2026}'));
        assert_eq!(truncate_chars("abc", 0), "");
    }

    #[test]
    fn badge_state_machine() {
        let mut s = ButtonState::default();
        assert!(s.open);
        assert_eq!(s.text(), "\u{0641}");
        assert_eq!(
            s.icon(),
            IconKind {
                open: true,
                warning: false
            }
        );
        assert_eq!(
            s.menu(true).iter().map(|m| m.0).collect::<Vec<_>>(),
            [MENU_SETTINGS, MENU_RECHECK]
        );
        assert!(s.menu(true).iter().all(|m| m.2));

        assert_ne!(s.set_badge(Some("Problem here.".into())), 0);
        assert_eq!(
            s.set_badge(Some("Problem   here.".into())),
            0,
            "same after normalising"
        );
        assert!(s.icon().warning);
        assert!(s.tooltip().starts_with("Problem here. Persian Phonetic"));
        assert_eq!(
            s.menu(true).iter().map(|m| m.0).collect::<Vec<_>>(),
            [MENU_SETTINGS, MENU_FIX, MENU_RECHECK]
        );
        let greyed = s.menu(false);
        assert_eq!(
            greyed.iter().map(|m| (m.0, m.2)).collect::<Vec<_>>(),
            [
                (MENU_SETTINGS, false),
                (MENU_FIX, false),
                (MENU_RECHECK, true)
            ]
        );
        assert!(greyed[0].1.contains("Start menu"));
        assert_ne!(s.set_open(false), 0);
        assert_eq!(s.set_open(false), 0);
        assert_eq!(s.text(), "A");
        assert_eq!(
            s.icon(),
            IconKind {
                open: false,
                warning: true
            }
        );

        assert_ne!(s.set_badge(Some("   ".into())), 0, "blank clears");
        assert_eq!(s.badge, None);
        assert_eq!(s.set_badge(None), 0);
        assert!(!s.tooltip().contains("Problem"));

        s.set_badge(Some("x ".repeat(400)));
        assert!(s.tooltip().chars().count() <= TOOLTIP_MAX_CHARS);
    }

    #[test]
    fn launch_policy() {
        let (low, med, med_plus, high) = (0x1000, 0x2000, 0x2100, 0x3000);
        assert!(launch_allowed(false, Some(med), Some(med)));
        assert!(launch_allowed(false, Some(med_plus), Some(med)));
        assert!(
            launch_allowed(false, Some(med), None),
            "no shell: medium assumed"
        );
        assert!(launch_allowed(false, Some(high), Some(high)), "UAC off");
        assert!(
            !launch_allowed(false, Some(high), Some(med)),
            "elevated host"
        );
        assert!(!launch_allowed(false, Some(high), None));
        assert!(
            !launch_allowed(false, Some(low), Some(med)),
            "low integrity host"
        );
        assert!(!launch_allowed(true, Some(low), Some(med)), "AppContainer");
        assert!(!launch_allowed(true, Some(med), Some(med)));
        assert!(!launch_allowed(false, None, Some(med)), "token unreadable");
    }

    #[test]
    fn command_line_quotes_exe() {
        let p = std::path::Path::new(r"C:\Program Files\Persian Phonetic IME\pime-settings.exe");
        assert_eq!(
            settings_command_line(p, &["--show-errors"]),
            r#""C:\Program Files\Persian Phonetic IME\pime-settings.exe" --show-errors"#
        );
        assert_eq!(
            settings_command_line(p, &[]),
            format!("\"{}\"", p.display())
        );
    }

    #[test]
    fn pixels_alpha_from_coverage_and_warning_dot() {
        let n = 16;
        let mut px = vec![0u32; n * n];
        px[0] = 0x00FF_FFFF;
        px[1] = 0x0040_4040;
        compose_icon_pixels(&mut px, n, 0x123456, false);
        assert_eq!(px[0], 0xFF12_3456);
        assert_eq!(px[1], 0x4012_3456);
        assert_eq!(px[2], 0);
        let mut w = vec![0u32; n * n];
        compose_icon_pixels(&mut w, n, 0xFFFFFF, true);
        assert_eq!(w[(n - 3) * n + (n - 3)], 0xFFFF_B900, "dot bottom-right");
        assert_eq!(w[0], 0);
    }
}
