//! The settings window: a pinned "Try typing" lens, four glass tabs (Letters, Keys and
//! list, Word completion, Settings file) and a status pill. Thin glue over the tested
//! library; all decisions live there.
//!
//! The look is painted in software (`glass.rs`): one scene bitmap holds the backdrop,
//! panels and static text, and the buttons, switches and menu fields are owner-drawn
//! on top of the matching piece of that scene, so Tab, Space and focus keep working
//! with stock controls. Only the text boxes are native edit controls.
//!
//! Re-entrancy: Win32 sends messages synchronously from inside calls such as
//! `SetWindowTextW` (an edit control notifies `EN_CHANGE`), so the app state is an
//! `Rc` cloned out of the thread-local and no `RefCell` borrow is ever held across a
//! Win32 call. Every window procedure body runs inside `catch_unwind`.

use pime_core::{OptionValue, ResolvedConfig, GLASS_OPACITY_MAX, GLASS_OPACITY_MIN};
use pime_dict::Runtime;
use pime_settings::backup::{ensure_exists, reset_config, Timestamp};
use pime_settings::dictionary::{preview_completer, start_preview_runtime};
use pime_settings::edit::save_setting;
use pime_settings::glass::{palette, Blob, Canvas, Glass, Rect, Rgb, Theme};
use pime_settings::learned::{
    clear_history, count_text, learned_count, CHECK_ROWS, NUMBER_ROWS, PRIVACY_LINE, SECTION,
};
use pime_settings::options::FORM_OPTIONS;
use pime_settings::paths::Paths;
use pime_settings::reload::signal_config_changed;
use pime_settings::report::{write_errors_file, Check, FileState};
use pime_settings::try_typing::preview_with;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use windows::core::{w, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::CreateBitmap;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, GetDC, InvalidateRect, RedrawWindow, ReleaseDC,
    ScreenToClient, SelectObject, SetBkColor, SetBkMode, SetTextColor, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FW_NORMAL,
    FW_SEMIBOLD, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    RDW_ALLCHILDREN, RDW_INVALIDATE, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::Com::{
    CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED};
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DispatchMessageW, GetMessageW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    IsDialogMessageW, LoadCursorW, LoadIconW, MessageBoxW, PostQuitMessage, RegisterClassExW,
    SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu, TranslateMessage,
    BN_CLICKED, BS_OWNERDRAW, CW_USEDEFAULT, EN_CHANGE, EN_KILLFOCUS, EN_SETFOCUS, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_CENTER, ES_MULTILINE, ES_NUMBER, ES_READONLY, ES_RIGHT, HMENU, IDC_ARROW,
    IDI_APPLICATION, IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_YESNO, MF_CHECKED, MF_STRING,
    MSG, SWP_NOMOVE, SWP_NOZORDER, SW_HIDE, SW_SHOW, SW_SHOWNORMAL, TPM_LEFTALIGN, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_TOPALIGN, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_COMMAND,
    WM_DESTROY, WM_DRAWITEM, WM_ERASEBKGND, WM_MOUSEMOVE, WM_PAINT, WM_SETFONT, WM_SETTINGCHANGE,
    WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_EX_RTLREADING, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO, ICON_BIG, ICON_SMALL, SM_CXICON,
    SM_CXSMICON, WM_SETICON,
};

const CLASS: PCWSTR = w!("PersianPhoneticImeSettings");

/// The pe logo (site/favicon.svg) as 128x128 straight-alpha RGBA.
const LOGO: &[u8] = include_bytes!("../assets/logo-128.rgba");
const LOGO_PX: usize = 128;
const ID_PILL: i32 = 90;
const ID_TAB_BASE: i32 = 91;
const ID_STATUS: i32 = 100;
const ID_OPEN: i32 = 101;
const ID_RESET: i32 = 102;
const ID_CHECK: i32 = 103;
const ID_TRY_IN: i32 = 110;
const ID_TRY_OUT: i32 = 111;
const ID_COMBO_BASE: i32 = 200;
const ID_CHECK_BASE: i32 = 300;
const ID_NUMBER_BASE: i32 = 320;
const ID_CLEAR: i32 = 340;
const ID_OPACITY: i32 = 350;
const WA_INACTIVE: usize = 0;
const MSG_CTLCOLOREDIT: u32 = 0x0133;
const MSG_CTLCOLORSTATIC: u32 = 0x0138;
const MSG_MOUSELEAVE: u32 = 0x02A3;

// Layout, in DIPs.
const WIDTH: i32 = 720;
const M: i32 = 24;
const CARD_W: i32 = WIDTH - 2 * M;
const LABEL_X: i32 = M + 22;
const RIGHT_X: i32 = WIDTH - M - 22;
const ROW: i32 = 38;
const PAD: i32 = 14;
const HERO_Y: i32 = 82;
const HERO_H: i32 = 136;
const TABS_Y: i32 = 232;
const CONTENT_Y: i32 = 282;
const FIELD_X: i32 = 300;
const FOOTER: i32 = 48;

// Fonts, by index into `App::fonts`.
const F_BODY: usize = 0;
const F_SMALL: usize = 1;
const F_BOLD: usize = 2;
const F_TITLE: usize = 3;
const F_PERSIAN: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Letters,
    Keys,
    Completion,
    Appearance,
    File,
}

/// Tab order, label and width in DIPs.
const PAGES: [(Page, &str, i32); 5] = [
    (Page::Letters, "Letters", 96),
    (Page::Keys, "Keys and list", 124),
    (Page::Completion, "Word completion", 150),
    (Page::Appearance, "Appearance", 112),
    (Page::File, "Settings file", 124),
];

/// Which tab an option from the form lives on.
fn page_of(key: &str) -> Page {
    match key {
        "space" | "enter" | "number_select" | "candidate_labels" | "toggle_hotkey"
        | "candidate_position" => Page::Keys,
        "theme" => Page::Appearance,
        _ => Page::Letters,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Health {
    Ok,
    Defaults,
    Problems(usize),
    Unreadable,
}

impl Health {
    fn label(self) -> String {
        match self {
            Health::Ok => "No problems".into(),
            Health::Defaults => "Using defaults".into(),
            Health::Problems(n) => format!("{n} problem{}", if n == 1 { "" } else { "s" }),
            Health::Unreadable => "File unreadable".into(),
        }
    }

    fn color(self) -> Rgb {
        match self {
            Health::Ok => palette::OK,
            Health::Defaults => palette::SAFFRON,
            _ => palette::PROBLEM,
        }
    }
}

/// What the painted parts of the window currently show.
#[derive(Clone)]
struct Dyn {
    theme: Theme,
    page: Page,
    health: Health,
    alts: String,
    note: String,
    learned: String,
    focus: i32,
    /// Current choice label per form option.
    field_text: Vec<String>,
    /// For on/off form options: whether the first value (true) is chosen.
    option_on: Vec<bool>,
    checks_on: Vec<bool>,
}

#[derive(Clone, Copy)]
struct Scene {
    bits: *const u32,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
}

struct App {
    paths: Paths,
    hwnd: HWND,
    scale: f32,
    errors_only: bool,
    size: (i32, i32),
    status: HWND,
    /// Per form option: its tab and row on that tab.
    option_pos: Vec<(Page, i32)>,
    option_is_switch: Vec<bool>,
    fields: Vec<HWND>,
    numbers: Vec<HWND>,
    tabs: Vec<HWND>,
    page_ctls: Vec<(Page, HWND)>,
    runtime: Option<Arc<Runtime>>,
    try_in: Option<HWND>,
    try_out: Option<HWND>,
    cfg: RefCell<Arc<ResolvedConfig>>,
    fonts: Vec<HFONT>,
    brushes: Cell<[HBRUSH; 2]>,
    opacity: Option<HWND>,
    ui: RefCell<Dyn>,
    /// The scene without text: what owner-drawn controls paint over.
    base: RefCell<Canvas>,
    scene: Cell<Option<Scene>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    static HOVER: Cell<i32> = const { Cell::new(0) };
}

fn app() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

/// Detaches from a console window that was created only for this process (the exe
/// was started from Explorer), so no empty console stays behind the window.
pub fn release_own_console() {
    let mut ids = [0u32; 2];
    // SAFETY: plain FFI calls with a valid buffer.
    unsafe {
        if GetConsoleProcessList(&mut ids) == 1 {
            let _ = FreeConsole();
        }
    }
}

/// Opens `path` with the program associated with `.toml`, or Notepad if none is.
pub fn open_in_editor(path: &Path) -> std::io::Result<()> {
    let file = HSTRING::from(path.as_os_str());
    // SAFETY: plain FFI calls; strings outlive them. COM is initialised for shell
    // handlers that need it (a repeated initialisation is harmless).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let r = ShellExecuteW(None, w!("open"), &file, None, None, SW_SHOWNORMAL);
        if r.0 as isize > 32 {
            return Ok(());
        }
        let args = HSTRING::from(format!("\"{}\"", path.display()));
        let r = ShellExecuteW(
            None,
            w!("open"),
            w!("notepad.exe"),
            &args,
            None,
            SW_SHOWNORMAL,
        );
        if r.0 as isize > 32 {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "ShellExecute failed ({})",
                r.0 as isize
            )))
        }
    }
}

/// Runs the window until it is closed; returns the exit code.
pub fn run(paths: Paths, errors_only: bool) -> i32 {
    match create_and_loop(paths, errors_only) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!(
                "pime-settings: couldn't open the settings window ({})",
                e.message()
            );
            pime_settings::report::exit::IO_ERROR
        }
    }
}

// ---------------------------------------------------------------- geometry

fn content_top(errors_only: bool) -> i32 {
    if errors_only {
        HERO_Y
    } else {
        CONTENT_Y
    }
}

fn window_height(errors_only: bool) -> i32 {
    content_top(errors_only) + card_height(Page::File, 0) + FOOTER
}

fn row_y(errors_only: bool, row: i32) -> i32 {
    content_top(errors_only) + PAD + row * ROW
}

/// Height of a tab's glass card; `rows` is the number of option rows on it.
fn card_height(page: Page, rows: i32) -> i32 {
    match page {
        Page::Letters | Page::Keys => 2 * PAD + rows * ROW,
        Page::Appearance => 2 * PAD + (rows + 1) * ROW + 8 + 20,
        Page::Completion => 2 * PAD + 7 * ROW + 8 + 26 + 30,
        Page::File => 262,
    }
}

/// Window height the tallest tab needs.
fn full_height(errors_only: bool, rows: &[i32; 3]) -> i32 {
    if errors_only {
        return window_height(true);
    }
    let tallest = [
        card_height(Page::Letters, rows[0]),
        card_height(Page::Keys, rows[1]),
        card_height(Page::Appearance, rows[2]),
        card_height(Page::Completion, 0),
        card_height(Page::File, 0),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    CONTENT_Y + tallest + FOOTER
}

fn tab_x(i: usize) -> i32 {
    M + 4 + PAGES[..i].iter().map(|p| p.2).sum::<i32>()
}

// ---------------------------------------------------------------- controls

/// Creates child controls in DIP coordinates on one parent.
struct Builder {
    parent: HWND,
    hinst: HINSTANCE,
    scale: f32,
}

/// One control to create: class, text, style bits, extended style, id.
struct Ctl<'a>(PCWSTR, &'a str, i32, WINDOW_EX_STYLE, i32);

impl Builder {
    fn px(&self, dip: i32) -> i32 {
        (dip as f32 * self.scale).round() as i32
    }

    /// Creates `ctl` at (x, y, w, h) DIPs with `font`.
    unsafe fn add(
        &self,
        ctl: Ctl,
        (x, y, w, h): (i32, i32, i32, i32),
        font: HFONT,
    ) -> Result<HWND> {
        let Ctl(class, text, style, ex, id) = ctl;
        let hwnd = CreateWindowExW(
            ex,
            class,
            &HSTRING::from(text),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style as u32),
            self.px(x),
            self.px(y),
            self.px(w),
            self.px(h),
            Some(self.parent),
            Some(HMENU(id as isize as *mut c_void)),
            Some(self.hinst),
            None,
        )?;
        SendMessageW(
            hwnd,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        Ok(hwnd)
    }

    /// An owner-drawn button that also tracks the mouse for hover highlights.
    unsafe fn button(&self, id: i32, rect: (i32, i32, i32, i32), font: HFONT) -> Result<HWND> {
        let style = BS_OWNERDRAW | WS_TABSTOP.0 as i32;
        let hwnd = self.add(
            Ctl(w!("BUTTON"), "", style, WINDOW_EX_STYLE(0), id),
            rect,
            font,
        )?;
        let _ = SetWindowSubclass(hwnd, Some(hover_proc), id as usize, 0);
        Ok(hwnd)
    }
}

extern "system" fn hover_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    let id = id as i32;
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: plain FFI calls with valid handles and a local struct.
        unsafe {
            if msg == WM_MOUSEMOVE && HOVER.get() != id {
                HOVER.set(id);
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut t);
                let _ = InvalidateRect(Some(hwnd), None, false);
            } else if msg == MSG_MOUSELEAVE {
                if HOVER.get() == id {
                    HOVER.set(0);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }));
    // SAFETY: forwarding the unmodified message to the next procedure.
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

fn bgr(c: Rgb) -> COLORREF {
    COLORREF(((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF))
}

/// The window's fonts at `scale`, indexed by the `F_*` constants.
fn make_fonts(scale: f32) -> Vec<HFONT> {
    // SAFETY: plain GDI calls; the fonts are freed by the caller.
    unsafe {
        let font = |pt: i32, weight: i32| {
            CreateFontW(
                -((pt * 4) as f32 / 3.0 * scale).round() as i32,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Segoe UI"),
            )
        };
        let normal = FW_NORMAL.0 as i32;
        let semibold = FW_SEMIBOLD.0 as i32;
        let fonts = vec![
            font(10, normal),
            font(9, normal),
            font(10, semibold),
            font(18, semibold),
            font(15, normal),
        ];
        fonts
    }
}

fn create_and_loop(paths: Paths, errors_only: bool) -> Result<()> {
    // SAFETY: standard Win32 window creation and message loop on this thread; all
    // handles passed are ones created here.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let scale = GetDpiForSystem() as f32 / 96.0;
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let fonts = make_fonts(scale);

        // Option rows per tab, in form order.
        let mut counts = [0i32; 3];
        let option_pos: Vec<(Page, i32)> = FORM_OPTIONS
            .iter()
            .map(|o| {
                let page = page_of(o.key);
                let slot = match page {
                    Page::Keys => 1,
                    Page::Appearance => 2,
                    _ => 0,
                };
                counts[slot] += 1;
                (page, counts[slot] - 1)
            })
            .collect();
        let option_is_switch: Vec<bool> = FORM_OPTIONS
            .iter()
            .map(|o| o.value(0) == Some(OptionValue::Bool(true)) && o.len() == 2)
            .collect();

        let height = full_height(errors_only, &counts);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let title = if errors_only {
            w!("Persian Phonetic IME: settings problems")
        } else {
            w!("Persian Phonetic IME settings")
        };
        // Created hidden with a provisional size; resized to fit once laid out.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            title,
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            None,
            None,
            Some(hinst),
            None,
        )?;
        let b = Builder {
            parent: hwnd,
            hinst,
            scale,
        };
        let (body, persian) = (fonts[F_BODY], fonts[F_PERSIAN]);

        let (ed, none) = (w!("EDIT"), WINDOW_EX_STYLE(0));
        let tab = WS_TABSTOP.0 as i32;
        let mut page_ctls: Vec<(Page, HWND)> = Vec::new();
        let (mut fields, mut numbers, mut tabs) = (vec![], vec![], vec![]);
        let (mut try_in, mut try_out, mut opacity) = (None, None, None);

        if !errors_only {
            let pill = (WIDTH - M - 190, 26, 190, 32);
            b.button(ID_PILL, pill, body)?;
            let in_style = ES_AUTOHSCROLL | tab;
            try_in = Some(b.add(
                Ctl(ed, "", in_style, none, ID_TRY_IN),
                (M + 36, HERO_Y + 53, 276, 20),
                body,
            )?);
            let out_style = ES_READONLY | ES_RIGHT | ES_AUTOHSCROLL;
            try_out = Some(b.add(
                Ctl(ed, "", out_style, WS_EX_RTLREADING, ID_TRY_OUT),
                (M + 346, HERO_Y + 46, 290, 32),
                persian,
            )?);
            for (i, (_, _, width)) in PAGES.iter().enumerate() {
                let rect = (tab_x(i), TABS_Y + 4, *width, 30);
                tabs.push(b.button(ID_TAB_BASE + i as i32, rect, body)?);
            }
            for i in 0..FORM_OPTIONS.len() {
                let (page, row) = option_pos[i];
                let y = row_y(false, row);
                let rect = if option_is_switch[i] {
                    (M + 10, y + 2, CARD_W - 20, ROW - 4)
                } else {
                    (FIELD_X, y + 4, RIGHT_X - FIELD_X, 30)
                };
                let h = b.button(ID_COMBO_BASE + i as i32, rect, body)?;
                fields.push(h);
                page_ctls.push((page, h));
            }
            for (i, _) in CHECK_ROWS.iter().enumerate() {
                let rect = (M + 10, row_y(false, i as i32) + 2, CARD_W - 20, ROW - 4);
                let h = b.button(ID_CHECK_BASE + i as i32, rect, body)?;
                page_ctls.push((Page::Completion, h));
            }
            for (i, _) in NUMBER_ROWS.iter().enumerate() {
                let y = row_y(false, 4 + i as i32);
                let style = ES_NUMBER | ES_AUTOHSCROLL | ES_CENTER | tab;
                let h = b.add(
                    Ctl(ed, "", style, none, ID_NUMBER_BASE + i as i32),
                    (RIGHT_X - 54, y + 11, 44, 18),
                    body,
                )?;
                numbers.push(h);
                page_ctls.push((Page::Completion, h));
            }
            let y = row_y(false, counts[2]);
            let style = ES_NUMBER | ES_AUTOHSCROLL | ES_CENTER | tab;
            let h = b.add(
                Ctl(ed, "", style, none, ID_OPACITY),
                (RIGHT_X - 54, y + 11, 44, 18),
                body,
            )?;
            opacity = Some(h);
            page_ctls.push((Page::Appearance, h));
            let y0 = row_y(false, 7) + 8 + 26;
            let h = b.button(ID_CLEAR, (RIGHT_X - 190, y0, 190, 30), body)?;
            page_ctls.push((Page::Completion, h));
        }

        // The settings-file tab (the only page in errors-only mode).
        let top = content_top(errors_only);
        let status_style = ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | WS_VSCROLL.0 as i32 | tab;
        let status = b.add(
            Ctl(ed, "", status_style, none, ID_STATUS),
            (LABEL_X + 10, top + 112, CARD_W - 64, 84),
            body,
        )?;
        page_ctls.push((Page::File, status));
        let buttons = [
            (ID_OPEN, 46, 170),
            (ID_RESET, 226, 170),
            (ID_CHECK, 406, 130),
        ];
        for (id, x, width) in buttons {
            let h = b.button(id, (x, top + 214, width, 32), body)?;
            page_ctls.push((Page::File, h));
        }

        let client_w = b.px(WIDTH);
        let client_h = b.px(height);
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: client_w,
            bottom: client_h,
        };
        AdjustWindowRectEx(&mut rc, style, false, WINDOW_EX_STYLE(0))?;
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_NOMOVE | SWP_NOZORDER,
        )?;

        let theme0 = Theme::new(false, 100);
        let dyn_state = Dyn {
            theme: theme0,
            page: if errors_only {
                Page::File
            } else {
                Page::Letters
            },
            health: Health::Ok,
            alts: String::new(),
            note: String::new(),
            learned: String::new(),
            focus: 0,
            field_text: vec![String::new(); FORM_OPTIONS.len()],
            option_on: vec![false; FORM_OPTIONS.len()],
            checks_on: vec![false; CHECK_ROWS.len()],
        };
        let state = Rc::new(App {
            runtime: (!errors_only).then(|| start_preview_runtime(&paths)),
            paths,
            hwnd,
            scale,
            errors_only,
            size: (client_w, client_h),
            status,
            option_pos,
            option_is_switch,
            fields,
            numbers,
            tabs,
            page_ctls,
            try_in,
            try_out,
            cfg: RefCell::new(Arc::new(ResolvedConfig::default())),
            fonts,
            brushes: Cell::new(theme_brushes(&theme0)),
            opacity,
            ui: RefCell::new(dyn_state),
            base: RefCell::new(Canvas::new(1, 1, theme0.base)),
            scene: Cell::new(None),
        });
        APP.with(|a| *a.borrow_mut() = Some(state.clone()));
        apply_page(&state);
        refresh(&state);
        set_window_icons(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        if let Some(t) = state.try_in {
            let _ = SetFocus(Some(t));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if let Some(rt) = &state.runtime {
            rt.shutdown();
        }
        APP.with(|a| a.borrow_mut().take());
        if let Some(scene) = state.scene.take() {
            free_scene(scene);
        }
        for f in &state.fonts {
            let _ = DeleteObject(HGDIOBJ(f.0));
        }
        for br in &state.brushes.get() {
            let _ = DeleteObject(HGDIOBJ(br.0));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- icon

/// The logo as an icon of `n` x `n` pixels.
fn logo_icon(n: i32) -> Option<HICON> {
    let size = n as usize;
    let sub = (LOGO_PX / size).clamp(1, 8);
    let mut bgra = vec![0u32; size * size];
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for j in 0..sub {
                for i in 0..sub {
                    let sx = (x * LOGO_PX / size + i * LOGO_PX / size / sub).min(LOGO_PX - 1);
                    let sy = (y * LOGO_PX / size + j * LOGO_PX / size / sub).min(LOGO_PX - 1);
                    let k = (sy * LOGO_PX + sx) * 4;
                    let al = LOGO[k + 3] as f32 / 255.0;
                    r += LOGO[k] as f32 * al;
                    g += LOGO[k + 1] as f32 * al;
                    b += LOGO[k + 2] as f32 * al;
                    a += al;
                }
            }
            let t = (sub * sub) as f32;
            let (a8, r8, g8, b8) = (
                (a / t * 255.0).round() as u32,
                (r / t).round() as u32,
                (g / t).round() as u32,
                (b / t).round() as u32,
            );
            // Premultiplied BGRA, as Windows expects for a 32-bit icon.
            bgra[y * size + x] = (a8 << 24) | (r8 << 16) | (g8 << 8) | b8;
        }
    }
    let info = BITMAPINFO {
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
    // SAFETY: plain GDI calls; the DIB holds n*n pixels and both bitmaps are freed after
    // the icon has copied them.
    unsafe {
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(color.0));
            return None;
        }
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u32, bgra.len());
        let mask = CreateBitmap(n, n, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: BOOL::from(true),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        });
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon.ok()
    }
}

/// Gives the window (title bar, taskbar, Alt+Tab) the pe logo.
fn set_window_icons(hwnd: HWND) {
    for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
        // SAFETY: plain FFI calls; the icon lives as long as the window.
        unsafe {
            if let Some(icon) = logo_icon(GetSystemMetrics(metric).max(16)) {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }
    }
}

// ---------------------------------------------------------------- theme

/// A recessed groove (tab track, switch that is off) in the theme's colours.
fn groove(p: &mut Painter, t: &Theme, r: Rect, radius: f32, depth: f32) {
    if t.dark {
        p.canvas.inset(r, radius, depth * 3.5, 0x000000, 0.1);
    } else {
        p.canvas.inset(r, radius, depth, t.line, 0.7);
    }
}

fn theme_brushes(t: &Theme) -> [HBRUSH; 2] {
    // SAFETY: plain GDI calls; the brushes are freed with the app or on a theme change.
    unsafe {
        [
            CreateSolidBrush(bgr(t.field)),
            CreateSolidBrush(bgr(t.field_lens)),
        ]
    }
}

/// Whether Windows apps are set to the dark mode.
fn system_uses_dark() -> bool {
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: plain FFI call with a valid out-buffer of the stated size.
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    err.is_ok() && value == 0
}

/// Dark or light title bar to match the window.
fn set_caption_dark(hwnd: HWND, dark: bool) {
    let v = BOOL::from(dark);
    // SAFETY: plain FFI call; the value outlives it. Fails harmlessly on old Windows.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &v as *const BOOL as *const c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
    }
}

/// Theme from the settings: `theme` (or the Windows mode) and `glass_opacity`.
fn theme_from(cfg: &ResolvedConfig) -> Theme {
    let dark = match cfg.option_value("theme") {
        Some(OptionValue::Text(t)) if t == "dark" => true,
        Some(OptionValue::Text(t)) if t == "light" => false,
        _ => system_uses_dark(),
    };
    let pct = match cfg.option_value("glass_opacity") {
        Some(OptionValue::Integer(i)) => i,
        _ => 100,
    };
    Theme::new(dark, pct)
}

// ---------------------------------------------------------------- painting

/// Text to draw over a canvas with GDI, in the canvas's pixel coordinates.
struct Run {
    rect: RECT,
    text: String,
    font: usize,
    color: Rgb,
    fmt: DRAW_TEXT_FORMAT,
}

const LEFT: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(
    DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0 | DT_END_ELLIPSIS.0,
);
const CENTRE: DRAW_TEXT_FORMAT =
    DRAW_TEXT_FORMAT(DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0);
const RIGHT: DRAW_TEXT_FORMAT =
    DRAW_TEXT_FORMAT(DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0);
const PATH: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(
    DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_NOPREFIX.0 | DT_PATH_ELLIPSIS.0,
);
const WRAP: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_LEFT.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0);

/// Shapes and text for one surface, with DIP-to-pixel conversion.
struct Painter {
    canvas: Canvas,
    runs: Vec<Run>,
    scale: f32,
}

impl Painter {
    fn new(w: usize, h: usize, scale: f32, fill: Rgb) -> Painter {
        Painter {
            canvas: Canvas::new(w, h, fill),
            runs: Vec::new(),
            scale,
        }
    }

    /// DIP rectangle to pixels.
    fn r(&self, x: i32, y: i32, w: i32, h: i32) -> Rect {
        self.rf(x as f32, y as f32, w as f32, h as f32)
    }

    fn rf(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let s = self.scale;
        Rect::new(x * s, y * s, w * s, h * s)
    }

    fn px(&self, v: f32) -> f32 {
        v * self.scale
    }

    fn text(&mut self, r: Rect, text: &str, font: usize, color: Rgb, fmt: DRAW_TEXT_FORMAT) {
        if text.is_empty() {
            return;
        }
        self.runs.push(Run {
            rect: RECT {
                left: r.x.round() as i32,
                top: r.y.round() as i32,
                right: (r.x + r.w).round() as i32,
                bottom: (r.y + r.h).round() as i32,
            },
            text: text.to_string(),
            font,
            color,
            fmt,
        });
    }
}

/// Draws `canvas` into a new memory bitmap and lays `runs` on top with GDI.
unsafe fn compose(p: &Painter, fonts: &[HFONT]) -> Option<Scene> {
    let c = &p.canvas;
    let screen = GetDC(None);
    let dc = CreateCompatibleDC(Some(screen));
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: c.w as i32,
            biHeight: -(c.h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0);
    ReleaseDC(None, screen);
    let bmp = match bmp {
        Ok(b) if !bits.is_null() => b,
        _ => {
            let _ = DeleteDC(dc);
            return None;
        }
    };
    // SAFETY: the DIB section holds exactly w*h 32-bit pixels at `bits`.
    std::ptr::copy_nonoverlapping(c.px.as_ptr(), bits as *mut u32, c.px.len());
    let old = SelectObject(dc, HGDIOBJ(bmp.0));
    SetBkMode(dc, TRANSPARENT);
    for run in &p.runs {
        SelectObject(dc, HGDIOBJ(fonts[run.font].0));
        SetTextColor(dc, bgr(run.color));
        let mut rect = run.rect;
        let mut text: Vec<u16> = run.text.encode_utf16().collect();
        DrawTextW(dc, &mut text, &mut rect, run.fmt);
    }
    Some(Scene {
        bits: bits as *const u32,
        dc,
        bmp,
        old,
    })
}

fn free_scene(s: Scene) {
    // SAFETY: handles created by `compose`, released once.
    unsafe {
        SelectObject(s.dc, s.old);
        let _ = DeleteObject(HGDIOBJ(s.bmp.0));
        let _ = DeleteDC(s.dc);
    }
}

fn blobs(w: f32, h: f32, light: f32) -> [Blob; 4] {
    let blob = |cx, cy, radius, color, alpha| Blob {
        cx,
        cy,
        radius,
        color,
        alpha: alpha * light,
    };
    [
        blob(w * 0.94, h * 0.02, w * 0.62, 0xFFB52E, 0.3),
        blob(w * 0.08, h * 0.0, w * 0.34, 0xFFD27A, 0.16),
        blob(w * 0.02, h * 0.58, w * 0.5, 0x7FD1C7, 0.2),
        blob(w * 0.78, h * 1.0, w * 0.5, 0xFF9C7A, 0.16),
    ]
}

/// Builds the window's shapes and static text for the current state.
fn build_scene(app: &App, d: &Dyn) -> Painter {
    let t = d.theme;
    let (w, h) = app.size;
    let mut p = Painter::new(w as usize, h as usize, app.scale, t.base);
    p.canvas
        .backdrop(t.base, &blobs(w as f32, h as f32, t.light));
    let eo = app.errors_only;
    let ink = t.ink;
    let soft = t.ink_soft;

    // Header: the saffron drop and the title.
    let logo = p.r(M, 18, 44, 44);
    p.canvas
        .shadow(logo, logo.w * 0.25, 10.0, 4.0, palette::SAFFRON_DEEP, 0.25);
    p.canvas.blit_rgba(LOGO, LOGO_PX, LOGO_PX, logo);
    let r = p.r(M + 58, 16, 420, 30);
    p.text(r, "Persian Phonetic IME", F_TITLE, ink, LEFT);
    let sub = if eo {
        "These problems in your settings file need a look."
    } else {
        "Changes are saved as soon as you make them."
    };
    let r = p.r(M + 58, 44, 520, 20);
    p.text(r, sub, F_SMALL, soft, LEFT);

    if !eo {
        // The lens: live preview, always visible.
        let hero = p.r(M, HERO_Y, CARD_W, HERO_H);
        p.canvas.glass(hero, p.px(24.0), t.glass(Glass::PANEL));
        let r = p.r(LABEL_X, HERO_Y + 12, 200, 22);
        p.text(r, "Try typing", F_BOLD, ink, LEFT);
        let r = p.r(LABEL_X + 120, HERO_Y + 12, CARD_W - 44 - 120, 22);
        p.text(
            r,
            "Latin letters in, Persian out. Nothing here is saved.",
            F_SMALL,
            soft,
            RIGHT,
        );
        let input = p.r(LABEL_X, HERO_Y + 40, 300, 44);
        let radius = p.px(14.0);
        p.canvas.fill(input, radius, t.field, 1.0);
        p.canvas
            .stroke(input, radius, 1.0, t.line, t.line_alpha(0.1));
        if d.focus == ID_TRY_IN {
            p.canvas
                .ring(input, radius, p.px(2.0), palette::SAFFRON, 0.95);
        }
        let lens = p.r(LABEL_X + 312, HERO_Y + 40, CARD_W - 44 - 312, 44);
        p.canvas.fill(lens, radius, t.field_lens, 1.0);
        p.canvas.stroke(lens, radius, 1.2, palette::SAFFRON, 0.55);
        let r = p.r(LABEL_X, HERO_Y + 92, CARD_W - 44, 36);
        p.text(r, &d.alts, F_SMALL, soft, WRAP);

        // Tab track.
        let track_w = tab_x(PAGES.len()) - M + 4;
        let track = p.r(M, TABS_Y, track_w, 38);
        groove(&mut p, &t, track, track.h / 2.0, 0.07);
    }

    // The card for the current tab.
    let top = content_top(eo);
    let rows_here = d_rows(app, d.page);
    let card_h = card_height(d.page, rows_here);
    let card = p.r(M, top, CARD_W, card_h);
    p.canvas.glass(card, p.px(24.0), t.glass(Glass::PANEL));
    let rule = |p: &mut Painter, y: i32| {
        let line = p.rf(LABEL_X as f32, y as f32, (RIGHT_X - LABEL_X) as f32, 1.0);
        p.canvas.fill(line, 0.0, t.line, t.line_alpha(0.07));
    };
    match d.page {
        Page::Letters | Page::Keys | Page::Appearance => {
            for (i, o) in FORM_OPTIONS.iter().enumerate() {
                let (page, row) = app.option_pos[i];
                if page != d.page {
                    continue;
                }
                let y = row_y(eo, row);
                if !app.option_is_switch[i] {
                    let r = p.r(LABEL_X, y, FIELD_X - LABEL_X - 8, ROW);
                    p.text(r, o.label, F_BODY, ink, LEFT);
                }
                if row + 1 < rows_here || d.page == Page::Appearance {
                    rule(&mut p, y + ROW);
                }
            }
        }
        Page::Completion => {
            for row in 0..6 {
                rule(&mut p, row_y(eo, row) + ROW);
            }
            for (i, n) in NUMBER_ROWS.iter().enumerate() {
                let y = row_y(eo, 4 + i as i32);
                let r = p.r(LABEL_X, y, 360, ROW);
                p.text(r, n.label, F_BODY, ink, LEFT);
                let frame = p.r(RIGHT_X - 64, y + 4, 64, 30);
                let radius = p.px(10.0);
                p.canvas.fill(frame, radius, t.field, 1.0);
                p.canvas
                    .stroke(frame, radius, 1.0, t.line, t.line_alpha(0.1));
                if d.focus == ID_NUMBER_BASE + i as i32 {
                    p.canvas
                        .ring(frame, radius, p.px(2.0), palette::SAFFRON, 0.95);
                }
            }
            let y0 = row_y(eo, 7) + 8;
            let r = p.r(LABEL_X, y0, RIGHT_X - LABEL_X, 20);
            p.text(r, PRIVACY_LINE, F_SMALL, soft, LEFT);
            let r = p.r(LABEL_X, y0 + 26, 280, 30);
            p.text(r, &d.learned, F_BODY, ink, LEFT);
        }
        Page::File => {
            let r = p.r(LABEL_X, top + 16, 300, 20);
            p.text(r, "Settings file", F_BOLD, ink, LEFT);
            let frame = p.r(LABEL_X, top + 38, RIGHT_X - LABEL_X, 34);
            let radius = p.px(12.0);
            p.canvas.fill(frame, radius, t.field, 1.0);
            p.canvas
                .stroke(frame, radius, 1.0, t.line, t.line_alpha(0.1));
            let r = p.r(LABEL_X + 12, top + 38, RIGHT_X - LABEL_X - 24, 34);
            p.text(
                r,
                &app.paths.config_file.display().to_string(),
                F_BODY,
                ink,
                PATH,
            );
            let r = p.r(LABEL_X, top + 84, 300, 20);
            p.text(r, "Status", F_BOLD, ink, LEFT);
            let frame = p.r(LABEL_X, top + 106, RIGHT_X - LABEL_X, 96);
            p.canvas.fill(frame, radius, t.field, 1.0);
            p.canvas
                .stroke(frame, radius, 1.0, t.line, t.line_alpha(0.1));
        }
    }

    if d.page == Page::Appearance {
        let y = row_y(eo, rows_here);
        let r = p.r(LABEL_X, y, 360, ROW);
        p.text(r, "Glass opacity (percent)", F_BODY, t.ink, LEFT);
        let frame = p.r(RIGHT_X - 64, y + 4, 64, 30);
        let radius = p.px(10.0);
        p.canvas.fill(frame, radius, t.field, 1.0);
        p.canvas
            .stroke(frame, radius, 1.0, t.line, t.line_alpha(0.1));
        if d.focus == ID_OPACITY {
            p.canvas
                .ring(frame, radius, p.px(2.0), palette::SAFFRON, 0.95);
        }
        let r = p.r(LABEL_X, y + ROW + 8, RIGHT_X - LABEL_X, 20);
        let hint = "Lower values show more of the background. Applies when you leave the box.";
        p.text(r, hint, F_SMALL, t.ink_soft, LEFT);
    }

    // The note about the last action.
    if !d.note.is_empty() {
        let dot = p.rf(M as f32 + 4.0, (h as f32 / p.scale) - 33.0, 8.0, 8.0);
        p.canvas.fill(dot, dot.w / 2.0, palette::SAFFRON, 1.0);
        let r = p.r(M + 20, (h as f32 / app.scale) as i32 - 40, 640, 22);
        p.text(r, &d.note, F_SMALL, soft, LEFT);
    }
    p
}

/// Number of option rows on a form tab.
fn d_rows(app: &App, page: Page) -> i32 {
    app.option_pos.iter().filter(|(p, _)| *p == page).count() as i32
}

/// Rebuilds the scene from the current state and repaints the window.
fn repaint(app: &App) {
    let d = app.ui.borrow().clone();
    let painter = build_scene(app, &d);
    // SAFETY: plain GDI calls on handles owned by this module.
    unsafe {
        let scene = compose(&painter, &app.fonts);
        // Only the shapes are kept for the controls; text under a control would show
        // through, and the scene paints none there.
        *app.base.borrow_mut() = painter.canvas;
        if let Some(old) = app.scene.replace(scene) {
            free_scene(old);
        }
        let _ = RedrawWindow(Some(app.hwnd), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
}

/// Look of one owner-drawn control right now.
#[derive(Clone, Copy)]
struct Look {
    hover: bool,
    down: bool,
    focus: bool,
}

fn glass_for(t: &Theme, look: Look) -> Glass {
    if look.hover || look.down {
        t.glass(Glass::HOVER)
    } else {
        t.glass(Glass::CONTROL)
    }
}

fn paint_control(app: &App, d: &Dyn, id: i32, p: &mut Painter, look: Look) {
    let t = d.theme;
    let (w, h) = (p.canvas.w as f32, p.canvas.h as f32);
    let all = Rect::new(0.0, 0.0, w, h);
    let s = app.scale;
    let ring = |p: &mut Painter, r: Rect, radius: f32| {
        if look.focus {
            p.canvas.stroke(r, radius, 2.0 * s, palette::SAFFRON, 0.95);
        }
    };
    let option = id - ID_COMBO_BASE;
    let check = id - ID_CHECK_BASE;
    if id == ID_PILL {
        let radius = h / 2.0;
        p.canvas.glass(all, radius, glass_for(&t, look));
        let dot = Rect::new(16.0 * s, (h - 9.0 * s) / 2.0, 9.0 * s, 9.0 * s);
        p.canvas.fill(dot, dot.w / 2.0, d.health.color(), 1.0);
        p.text(
            Rect::new(32.0 * s, 0.0, w - 40.0 * s, h),
            &d.health.label(),
            F_BODY,
            t.ink,
            LEFT,
        );
        ring(p, all, radius);
    } else if (ID_TAB_BASE..ID_TAB_BASE + PAGES.len() as i32).contains(&id) {
        let (page, label, _) = PAGES[(id - ID_TAB_BASE) as usize];
        let radius = h / 2.0;
        let on = d.page == page;
        if on {
            p.canvas.glass(all, radius, t.glass(Glass::RAISED));
        } else if look.hover || look.down {
            p.canvas.glass(all, radius, t.glass(Glass::FAINT));
        }
        let (font, color) = if on {
            (F_BOLD, t.accent_text)
        } else if look.hover {
            (F_BODY, t.ink)
        } else {
            (F_BODY, t.ink_soft)
        };
        p.text(all, label, font, color, CENTRE);
        ring(p, all, radius);
    } else if id == ID_OPEN {
        let radius = h / 2.0;
        p.canvas.saffron(all, radius, false);
        if look.down {
            p.canvas.fill(all, radius, palette::SAFFRON_DEEP, 0.16);
        } else if look.hover {
            p.canvas.fill(all, radius, 0xFFFFFF, 0.18);
        }
        p.text(all, "Open config file", F_BOLD, palette::ON_SAFFRON, CENTRE);
        ring(p, all, radius);
    } else if id == ID_RESET || id == ID_CHECK || id == ID_CLEAR {
        let radius = h / 2.0;
        p.canvas.glass(all, radius, glass_for(&t, look));
        if look.down {
            p.canvas.fill(all, radius, t.line, t.line_alpha(0.07));
        }
        let (label, color) = match id {
            ID_RESET => ("Reset to defaults", t.ink),
            ID_CHECK => ("Check again", t.ink),
            _ => ("Clear learned words", palette::DANGER),
        };
        p.text(all, label, F_BODY, color, CENTRE);
        ring(p, all, radius);
    } else if let Some(&on) = usize::try_from(check)
        .ok()
        .and_then(|i| d.checks_on.get(i))
        .filter(|_| (0..CHECK_ROWS.len() as i32).contains(&check))
    {
        paint_switch_row(p, &t, s, look, CHECK_ROWS[check as usize].label, on);
    } else if (0..FORM_OPTIONS.len() as i32).contains(&option) {
        let i = option as usize;
        if app.option_is_switch[i] {
            paint_switch_row(p, &t, s, look, FORM_OPTIONS[i].label, d.option_on[i]);
        } else {
            let radius = 11.0 * s;
            p.canvas.glass(all, radius, glass_for(&t, look));
            if look.down {
                p.canvas.fill(all, radius, t.line, t.line_alpha(0.06));
            }
            p.text(
                Rect::new(14.0 * s, 0.0, w - 48.0 * s, h),
                &d.field_text[i],
                F_BODY,
                t.ink,
                LEFT,
            );
            let (cx, cy) = (w - 20.0 * s, h / 2.0);
            let (dx, dy) = (4.5 * s, 2.5 * s);
            let tint = t.accent_text;
            p.canvas
                .line((cx - dx, cy - dy), (cx, cy + dy), 1.8 * s, tint, 0.95);
            p.canvas
                .line((cx, cy + dy), (cx + dx, cy - dy), 1.8 * s, tint, 0.95);
            ring(p, all, radius);
        }
    }
}

/// A row with its label on the left and a switch on the right.
fn paint_switch_row(p: &mut Painter, t: &Theme, s: f32, look: Look, label: &str, on: bool) {
    let (w, h) = (p.canvas.w as f32, p.canvas.h as f32);
    let all = Rect::new(0.0, 0.0, w, h);
    if look.hover || look.down {
        p.canvas.glass(all, 14.0 * s, t.glass(Glass::FAINT));
    }
    p.text(
        Rect::new(12.0 * s, 0.0, w - 80.0 * s, h),
        label,
        F_BODY,
        t.ink,
        LEFT,
    );
    let track = Rect::new(
        w - 12.0 * s - 44.0 * s,
        (h - 24.0 * s) / 2.0,
        44.0 * s,
        24.0 * s,
    );
    let radius = track.h / 2.0;
    if on {
        p.canvas.saffron(track, radius, false);
    } else {
        groove(p, t, track, radius, 0.16);
    }
    let size = 18.0 * s;
    let kx = if on {
        track.x + track.w - size - 3.0 * s
    } else {
        track.x + 3.0 * s
    };
    let knob = Rect::new(kx, track.y + 3.0 * s, size, size);
    p.canvas
        .shadow(knob, size / 2.0, 4.0 * s, 1.5 * s, t.shadow, 0.4);
    p.canvas.fill(knob, size / 2.0, 0xFFFFFF, 1.0);
    p.canvas
        .stroke(knob, size / 2.0, 1.0, t.line, t.line_alpha(0.1));
    if look.focus {
        p.canvas
            .stroke(all, 14.0 * s, 2.0 * s, palette::SAFFRON, 0.95);
    }
}

fn draw_item(app: &App, di: &DRAWITEMSTRUCT) {
    let w = (di.rcItem.right - di.rcItem.left).max(1) as usize;
    let h = (di.rcItem.bottom - di.rcItem.top).max(1) as usize;
    let mut rc = RECT::default();
    // SAFETY: valid control handle and local out-parameters.
    let origin = unsafe {
        let _ = GetWindowRect(di.hwndItem, &mut rc);
        let mut pt = POINT {
            x: rc.left,
            y: rc.top,
        };
        let _ = ScreenToClient(app.hwnd, &mut pt);
        pt
    };
    let id = di.CtlID as i32;
    let state = di.itemState.0;
    let look = Look {
        hover: HOVER.get() == id,
        down: state & ODS_SELECTED.0 != 0,
        focus: state & ODS_FOCUS.0 != 0 && state & ODS_NOFOCUSRECT.0 == 0,
    };
    let mut p = Painter::new(w, h, app.scale, 0);
    p.canvas = app.base.borrow().crop(origin.x, origin.y, w, h);
    let d = app.ui.borrow().clone();
    paint_control(app, &d, id, &mut p, look);
    // SAFETY: plain GDI calls; the bitmap is freed before returning.
    unsafe {
        if let Some(scene) = compose(&p, &app.fonts) {
            let _ = BitBlt(
                di.hDC,
                0,
                0,
                w as i32,
                h as i32,
                Some(scene.dc),
                0,
                0,
                SRCCOPY,
            );
            free_scene(scene);
        }
    }
}

// ---------------------------------------------------------------- state

fn set_text(hwnd: HWND, text: &str) {
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    // SAFETY: valid window handle; the string outlives the call.
    unsafe {
        let _ = SetWindowTextW(hwnd, &HSTRING::from(text));
    }
}

fn get_text(hwnd: HWND) -> String {
    // SAFETY: valid window handle and a buffer of the reported length.
    unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

/// Shows the controls of the current tab and hides the others.
fn apply_page(app: &App) {
    let page = app.ui.borrow().page;
    for &(p, hwnd) in &app.page_ctls {
        // SAFETY: valid child handle.
        unsafe {
            let _ = ShowWindow(hwnd, if p == page { SW_SHOW } else { SW_HIDE });
        }
    }
}

fn set_page(app: &App, page: Page) {
    if app.errors_only {
        return;
    }
    app.ui.borrow_mut().page = page;
    apply_page(app);
    repaint(app);
}

/// Re-reads the file, updates the status text, the form and the preview, and keeps
/// `config-errors.txt` in sync.
fn refresh(app: &App) {
    let (status, health) = match Check::load(&app.paths.config_file) {
        Ok(check) => {
            let _ = write_errors_file(
                &app.paths.errors_file,
                &app.paths.config_file,
                check.issues(),
            );
            let n = check.issues().len();
            let (text, health) = if check.state == FileState::Missing {
                (
                    "There is no settings file yet, so the default settings are in use. Change an \
                     option or click \"Open config file\" to create one."
                        .to_string(),
                    Health::Defaults,
                )
            } else if n == 0 {
                (
                    "Your settings file has no problems.".to_string(),
                    Health::Ok,
                )
            } else {
                (check.issues_text(), Health::Problems(n))
            };
            let cfg = Arc::new(check.loaded.config);
            *app.cfg.borrow_mut() = cfg;
            (text, health)
        }
        Err(e) => (
            format!("The settings file couldn't be read: {e}"),
            Health::Unreadable,
        ),
    };
    set_text(app.status, &status);
    let cfg = app.cfg.borrow().clone();
    let theme = theme_from(&cfg);
    let old_brushes = {
        let mut d = app.ui.borrow_mut();
        let changed = d.theme != theme;
        d.theme = theme;
        d.health = health;
        for (i, opt) in FORM_OPTIONS.iter().enumerate() {
            let current = opt.current(&cfg);
            d.field_text[i] = current.and_then(|c| opt.label_of(c)).unwrap_or_default();
            d.option_on[i] = current == Some(0);
        }
        for (i, row) in CHECK_ROWS.iter().enumerate() {
            d.checks_on[i] = cfg.option_value(&format!("autocomplete.{}", row.key))
                == Some(OptionValue::Bool(true));
        }
        d.learned = count_text(learned_count(&app.paths).unwrap_or(0));
        changed
    };
    let old_brushes = old_brushes.then(|| app.brushes.replace(theme_brushes(&theme)));
    set_caption_dark(app.hwnd, theme.dark);
    if let Some(op) = app.opacity {
        set_text(op, &((theme.opacity * 100.0).round() as i64).to_string());
    }
    for (row, &num) in NUMBER_ROWS.iter().zip(&app.numbers) {
        if let Some(OptionValue::Integer(n)) =
            cfg.option_value(&format!("autocomplete.{}", row.key))
        {
            set_text(num, &n.to_string());
        }
    }
    update_preview(app);
    repaint(app);
    for br in old_brushes.iter().flatten() {
        // SAFETY: the old brushes are no longer used by any control after the repaint.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(br.0));
        }
    }
}

/// Updates the preview text; the caller repaints.
fn update_preview(app: &App) {
    let (Some(input), Some(out)) = (app.try_in, app.try_out) else {
        return;
    };
    let completer = app.runtime.as_ref().map(preview_completer);
    let p = preview_with(app.cfg.borrow().clone(), &get_text(input), completer);
    set_text(out, &p.text_line());
    let lines = [p.alternatives_line(), p.completions_line()];
    let both: Vec<String> = lines.into_iter().filter(|l| !l.is_empty()).collect();
    app.ui.borrow_mut().alts = both.join("\n");
}

fn notify_saved(app: &App, note: &str) {
    std::thread::spawn(|| {
        let _ = signal_config_changed();
    });
    app.ui.borrow_mut().note = note.to_string();
}

fn message(hwnd: HWND, text: &str, yes_no: bool) -> bool {
    let style = if yes_no {
        MB_YESNO | MB_ICONQUESTION
    } else {
        MB_OK | MB_ICONERROR
    };
    // SAFETY: valid owner window; strings outlive the call.
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            w!("Persian Phonetic IME"),
            style,
        ) == IDYES
    }
}

/// Saves choice `choice` of form option `i`.
fn apply_option(app: &App, hwnd: HWND, i: usize, choice: usize) {
    let Some(opt) = FORM_OPTIONS.get(i) else {
        return;
    };
    let Some(value) = opt.value(choice) else {
        return;
    };
    match save_setting(
        &app.paths.config_file,
        pime_settings::edit::Section::Options,
        opt.key,
        &value,
    ) {
        Ok(()) => notify_saved(app, &format!("Saved: {}", opt.label)),
        Err(e) => {
            message(hwnd, &e.to_string(), false);
        }
    }
    refresh(app);
}

/// A click on an option row: flips a switch, or opens the menu of choices.
fn on_option(app: &App, hwnd: HWND, i: usize) {
    let (Some(opt), Some(&field)) = (FORM_OPTIONS.get(i), app.fields.get(i)) else {
        return;
    };
    let current = opt.current(&app.cfg.borrow());
    if app.option_is_switch[i] {
        apply_option(app, hwnd, i, usize::from(current == Some(0)));
        return;
    }
    // SAFETY: plain FFI calls; the menu is destroyed before returning.
    let picked = unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        for k in 0..opt.len() {
            let label = opt.label_of(k).unwrap_or_default().replace('&', "&&");
            let flags = if current == Some(k) {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(menu, flags, k + 1, &HSTRING::from(label));
        }
        let mut rc = RECT::default();
        let _ = GetWindowRect(field, &mut rc);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_NONOTIFY,
            rc.left,
            rc.bottom + 4,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0
    };
    if let Some(choice) = usize::try_from(picked).ok().and_then(|k| k.checked_sub(1)) {
        apply_option(app, hwnd, i, choice);
    }
}

fn save_autocomplete(app: &App, hwnd: HWND, label: &str, key: &str, value: OptionValue) {
    match save_setting(&app.paths.config_file, SECTION, key, &value) {
        Ok(()) => notify_saved(app, &format!("Saved: {label}")),
        Err(e) => {
            message(hwnd, &e.to_string(), false);
        }
    }
    refresh(app);
}

fn on_check(app: &App, hwnd: HWND, i: usize) {
    let Some(row) = CHECK_ROWS.get(i) else {
        return;
    };
    let on = app.ui.borrow().checks_on.get(i).copied().unwrap_or(false);
    save_autocomplete(app, hwnd, row.label, row.key, OptionValue::Bool(!on));
}

fn on_number(app: &App, hwnd: HWND, i: usize) {
    let (Some(row), Some(&num)) = (NUMBER_ROWS.get(i), app.numbers.get(i)) else {
        return;
    };
    // Empty or invalid text: put the saved value back.
    if let Some(n) = row.parse(&get_text(num)) {
        save_autocomplete(app, hwnd, row.label, row.key, OptionValue::Integer(n));
    } else {
        refresh(app);
    }
}

fn on_opacity(app: &App, hwnd: HWND) {
    let Some(op) = app.opacity else {
        return;
    };
    let Ok(n) = get_text(op).trim().parse::<i64>() else {
        refresh(app);
        return;
    };
    let n = n.clamp(GLASS_OPACITY_MIN as i64, GLASS_OPACITY_MAX as i64);
    match save_setting(
        &app.paths.config_file,
        pime_settings::edit::Section::Options,
        "glass_opacity",
        &OptionValue::Integer(n),
    ) {
        Ok(()) => notify_saved(app, "Saved: Glass opacity"),
        Err(e) => {
            message(hwnd, &e.to_string(), false);
        }
    }
    refresh(app);
}

fn on_clear(app: &App, hwnd: HWND) {
    if !message(
        hwnd,
        "Forget all the words the IME has learned from your choices?\n\nThis can't be undone. \
         Your settings are not changed.",
        true,
    ) {
        return;
    }
    match clear_history(&app.paths) {
        Ok(_) => notify_saved(app, "Learned words cleared."),
        Err(e) => {
            message(
                hwnd,
                &format!("The learned words couldn't be cleared ({e})."),
                false,
            );
        }
    }
    app.ui.borrow_mut().learned = count_text(learned_count(&app.paths).unwrap_or(0));
    repaint(app);
}

fn on_reset(app: &App, hwnd: HWND) {
    if !message(
        hwnd,
        "Replace your settings with the defaults?\n\nYour current file is saved as a backup \
         next to it first.",
        true,
    ) {
        return;
    }
    match reset_config(&app.paths.config_file, Timestamp::now()) {
        Ok(saved) => {
            let note = match saved {
                Some(p) => format!(
                    "Defaults restored. Old file saved as {}",
                    p.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                ),
                None => "Defaults restored.".to_string(),
            };
            notify_saved(app, &note);
        }
        Err(e) => {
            message(
                hwnd,
                &format!("The settings file couldn't be reset ({e})."),
                false,
            );
        }
    }
    refresh(app);
}

fn on_open(app: &App, hwnd: HWND) {
    let path = &app.paths.config_file;
    if let Err(e) = ensure_exists(path).and_then(|_| open_in_editor(path)) {
        message(
            hwnd,
            &format!("The settings file couldn't be opened ({e})."),
            false,
        );
    }
    refresh(app);
}

fn set_focus_id(app: &App, id: i32, on: bool) {
    {
        let mut d = app.ui.borrow_mut();
        if on {
            d.focus = id;
        } else if d.focus == id {
            d.focus = 0;
        } else {
            return;
        }
    }
    repaint(app);
}

fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            let app = app()?;
            let scene = app.scene.get();
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: standard paint sequence on this window; the scene bitmap is
            // owned by the app and outlives the call.
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                if let Some(s) = scene {
                    let _ = BitBlt(hdc, 0, 0, app.size.0, app.size.1, Some(s.dc), 0, 0, SRCCOPY);
                }
                let _ = EndPaint(hwnd, &ps);
            }
            Some(LRESULT(0))
        }
        WM_DRAWITEM => {
            let app = app()?;
            // SAFETY: for WM_DRAWITEM, lParam points to a DRAWITEMSTRUCT that is valid
            // for the duration of the message.
            let di = unsafe { (lp.0 as *const DRAWITEMSTRUCT).as_ref()? };
            draw_item(&app, di);
            Some(LRESULT(1))
        }
        MSG_CTLCOLOREDIT | MSG_CTLCOLORSTATIC => {
            let app = app()?;
            let lens = app.try_out.is_some_and(|h| h.0 == lp.0 as *mut c_void);
            let t = app.ui.borrow().theme;
            let brushes = app.brushes.get();
            let (fill, brush) = if lens {
                (t.field_lens, brushes[1])
            } else {
                (t.field, brushes[0])
            };
            let hdc = HDC(wp.0 as *mut c_void);
            // SAFETY: the DC belongs to the edit control being painted.
            unsafe {
                SetTextColor(hdc, bgr(t.ink));
                SetBkColor(hdc, bgr(fill));
            }
            Some(LRESULT(brush.0 as isize))
        }
        WM_COMMAND => {
            let app = app()?;
            let id = (wp.0 & 0xFFFF) as i32;
            let code = ((wp.0 >> 16) & 0xFFFF) as u32;
            match (id, code) {
                (ID_OPEN, BN_CLICKED) => on_open(&app, hwnd),
                (ID_RESET, BN_CLICKED) => on_reset(&app, hwnd),
                (ID_CHECK, BN_CLICKED) => {
                    app.ui.borrow_mut().note = "Checked again.".to_string();
                    refresh(&app);
                }
                (ID_CLEAR, BN_CLICKED) => on_clear(&app, hwnd),
                (ID_PILL, BN_CLICKED) => set_page(&app, Page::File),
                (id, BN_CLICKED)
                    if (ID_TAB_BASE..ID_TAB_BASE + PAGES.len() as i32).contains(&id) =>
                {
                    let i = (id - ID_TAB_BASE) as usize;
                    set_page(&app, PAGES[i].0);
                    if let Some(&t) = app.tabs.get(i) {
                        // SAFETY: valid control handle.
                        unsafe {
                            let _ = SetFocus(Some(t));
                        }
                    }
                }
                (id, BN_CLICKED) if (ID_CHECK_BASE..ID_NUMBER_BASE).contains(&id) => {
                    on_check(&app, hwnd, (id - ID_CHECK_BASE) as usize)
                }
                (id, BN_CLICKED) if (ID_COMBO_BASE..ID_CHECK_BASE).contains(&id) => {
                    on_option(&app, hwnd, (id - ID_COMBO_BASE) as usize)
                }
                (id, EN_SETFOCUS)
                    if id == ID_TRY_IN
                        || id == ID_OPACITY
                        || (ID_NUMBER_BASE..ID_CLEAR).contains(&id) =>
                {
                    set_focus_id(&app, id, true)
                }
                (ID_TRY_IN, EN_KILLFOCUS) => set_focus_id(&app, ID_TRY_IN, false),
                (ID_OPACITY, EN_KILLFOCUS) => {
                    set_focus_id(&app, ID_OPACITY, false);
                    on_opacity(&app, hwnd)
                }
                (id, EN_KILLFOCUS) if (ID_NUMBER_BASE..ID_CLEAR).contains(&id) => {
                    set_focus_id(&app, id, false);
                    on_number(&app, hwnd, (id - ID_NUMBER_BASE) as usize)
                }
                (ID_TRY_IN, EN_CHANGE) => {
                    update_preview(&app);
                    repaint(&app);
                }
                _ => return None,
            }
            Some(LRESULT(0))
        }
        // Windows switched between light and dark mode (or another setting changed).
        WM_SETTINGCHANGE => {
            if let Some(app) = app() {
                refresh(&app);
            }
            None
        }
        // Coming back from the editor: show the file's new state.
        WM_ACTIVATE if (wp.0 & 0xFFFF) != WA_INACTIVE => {
            if let Some(app) = app() {
                refresh(&app);
            }
            None
        }
        WM_DESTROY => {
            // SAFETY: plain FFI call.
            unsafe { PostQuitMessage(0) };
            Some(LRESULT(0))
        }
        _ => None,
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let r = std::panic::catch_unwind(|| handle(hwnd, msg, wp, lp))
        .ok()
        .flatten();
    // SAFETY: forwarding the unmodified message to the default procedure.
    r.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

// ---------------------------------------------------------------- previews

const PREVIEW_W: usize = 360;
const PREVIEW_H: usize = 216;

/// A miniature of the window in one look, for the installer's appearance page.
fn render_preview(dark: bool, fonts: &[HFONT]) -> Option<Canvas> {
    let t = Theme::new(dark, 100);
    let (w, h) = (PREVIEW_W, PREVIEW_H);
    let mut p = Painter::new(w, h, 1.0, t.base);
    p.canvas
        .backdrop(t.base, &blobs(w as f32, h as f32, t.light));
    p.canvas
        .blit_rgba(LOGO, LOGO_PX, LOGO_PX, Rect::new(16.0, 10.0, 32.0, 32.0));
    let title = Rect::new(58.0, 10.0, 290.0, 34.0);
    p.text(title, "Persian Phonetic IME", F_TITLE, t.ink, LEFT);

    let lens_card = Rect::new(16.0, 54.0, 328.0, 66.0);
    p.canvas.glass(lens_card, 18.0, t.glass(Glass::PANEL));
    p.text(
        Rect::new(32.0, 60.0, 200.0, 20.0),
        "Try typing",
        F_BOLD,
        t.ink,
        LEFT,
    );
    let input = Rect::new(32.0, 84.0, 140.0, 28.0);
    p.canvas.fill(input, 10.0, t.field, 1.0);
    p.canvas.stroke(input, 10.0, 1.0, t.line, t.line_alpha(0.1));
    p.text(
        Rect::new(44.0, 84.0, 120.0, 28.0),
        "salaam",
        F_BODY,
        t.ink,
        LEFT,
    );
    let lens = Rect::new(182.0, 84.0, 146.0, 28.0);
    p.canvas.fill(lens, 10.0, t.field_lens, 1.0);
    p.canvas.stroke(lens, 10.0, 1.2, palette::SAFFRON, 0.55);
    let rtl = DRAW_TEXT_FORMAT(RIGHT.0 | 0x0002_0000);
    let out = Rect::new(188.0, 82.0, 134.0, 30.0);
    p.text(
        out,
        "\u{0633}\u{0644}\u{0627}\u{0645}",
        F_PERSIAN,
        t.ink,
        rtl,
    );

    let card = Rect::new(16.0, 130.0, 328.0, 72.0);
    p.canvas.glass(card, 18.0, t.glass(Glass::PANEL));
    p.text(
        Rect::new(32.0, 136.0, 110.0, 30.0),
        "Digits",
        F_BODY,
        t.ink,
        LEFT,
    );
    let field = Rect::new(150.0, 139.0, 178.0, 24.0);
    p.canvas.glass(field, 9.0, t.glass(Glass::CONTROL));
    let digits = "Persian \u{06F0}\u{06F1}\u{06F2}\u{06F3}";
    p.text(
        Rect::new(162.0, 139.0, 140.0, 24.0),
        digits,
        F_BODY,
        t.ink,
        LEFT,
    );
    p.canvas
        .line((310.0, 148.0), (314.0, 153.0), 1.6, t.accent_text, 0.95);
    p.canvas
        .line((314.0, 153.0), (318.0, 148.0), 1.6, t.accent_text, 0.95);
    let label = Rect::new(32.0, 168.0, 220.0, 28.0);
    p.text(label, "Suggest word completions", F_BODY, t.ink, LEFT);
    p.canvas
        .saffron(Rect::new(284.0, 170.0, 44.0, 24.0), 12.0, false);
    let knob = Rect::new(305.0, 173.0, 18.0, 18.0);
    p.canvas.shadow(knob, 9.0, 4.0, 1.5, t.shadow, 0.4);
    p.canvas.fill(knob, 9.0, 0xFFFFFF, 1.0);

    // SAFETY: plain GDI calls; the bitmap is freed after the pixels were copied out.
    unsafe {
        let scene = compose(&p, fonts)?;
        let px = std::slice::from_raw_parts(scene.bits, w * h)
            .iter()
            .map(|c| c & 0x00FF_FFFF)
            .collect();
        free_scene(scene);
        Some(Canvas { w, h, px })
    }
}

/// Writes `theme-light.bmp`, `theme-dark.bmp` and `theme-system.bmp` (the two looks
/// joined along a diagonal) to `dir`, for the installer's appearance page.
pub fn render_previews(dir: &Path) -> std::io::Result<()> {
    let fonts = make_fonts(1.0);
    let light = render_preview(false, &fonts);
    let dark = render_preview(true, &fonts);
    for f in &fonts {
        // SAFETY: fonts created above, no longer selected anywhere.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(f.0));
        }
    }
    let (Some(light), Some(dark)) = (light, dark) else {
        return Err(std::io::Error::other("couldn't draw the previews"));
    };
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("theme-light.bmp"), light.to_bmp())?;
    std::fs::write(dir.join("theme-dark.bmp"), dark.to_bmp())?;
    let both = Canvas::split_diagonal(&light, &dark);
    std::fs::write(dir.join("theme-system.bmp"), both.to_bmp())
}
