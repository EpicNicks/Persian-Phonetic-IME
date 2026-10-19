//! The settings window: the settings-problems page (status, file path, "Open config
//! file", "Reset to defaults", "Check again"), the main options form and a "Try
//! typing" box. Thin glue over the tested library; all decisions live there.
//!
//! Re-entrancy: Win32 sends messages synchronously from inside calls such as
//! `SetWindowTextW` (an edit control notifies `EN_CHANGE`), so the app state is an
//! `Rc` cloned out of the thread-local and no `RefCell` borrow is ever held across a
//! Win32 call. Every window procedure body runs inside `catch_unwind`.

use pime_core::ResolvedConfig;
use pime_settings::backup::{ensure_exists, reset_config, Timestamp};
use pime_settings::edit::{save_setting, Section};
use pime_settings::options::FORM_OPTIONS;
use pime_settings::paths::Paths;
use pime_settings::reload::signal_config_changed;
use pime_settings::report::{write_errors_file, Check, FileState};
use pime_settings::try_typing::preview;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE,
    DEFAULT_CHARSET, DEFAULT_PITCH, FW_NORMAL, HBRUSH, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::Com::{
    CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, IsDialogMessageW, LoadCursorW, LoadIconW, MessageBoxW,
    PostQuitMessage, RegisterClassExW, SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow,
    TranslateMessage, BN_CLICKED, BS_PUSHBUTTON, CBN_SELCHANGE, CBS_DROPDOWNLIST, CB_ADDSTRING,
    CB_GETCURSEL, CB_SETCURSEL, CW_USEDEFAULT, EN_CHANGE, ES_AUTOHSCROLL, ES_AUTOVSCROLL,
    ES_MULTILINE, ES_READONLY, ES_RIGHT, HMENU, IDC_ARROW, IDI_APPLICATION, IDYES, MB_ICONERROR,
    MB_ICONQUESTION, MB_OK, MB_YESNO, MSG, SWP_NOMOVE, SWP_NOZORDER, SW_SHOWNORMAL,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WNDCLASSEXW,
    WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_RTLREADING, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

const CLASS: PCWSTR = w!("PersianPhoneticImeSettings");
const ID_STATUS: i32 = 100;
const ID_OPEN: i32 = 101;
const ID_RESET: i32 = 102;
const ID_CHECK: i32 = 103;
const ID_TRY_IN: i32 = 110;
const ID_TRY_OUT: i32 = 111;
const ID_TRY_ALTS: i32 = 112;
const ID_NOTE: i32 = 113;
const ID_COMBO_BASE: i32 = 200;
const WA_INACTIVE: usize = 0;
/// Static-control styles: shorten long paths with "...", show "&" literally.
const SS_PATHELLIPSIS: i32 = 0x8000;
const SS_NOPREFIX: i32 = 0x80;

struct App {
    paths: Paths,
    status: HWND,
    note: HWND,
    combos: Vec<HWND>,
    try_in: Option<HWND>,
    try_out: Option<HWND>,
    try_alts: Option<HWND>,
    cfg: RefCell<Arc<ResolvedConfig>>,
    fonts: Vec<HFONT>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
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
}

const WIDTH: i32 = 640;

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
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut c_void),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let font = |pt: i32| {
            CreateFontW(
                -((pt * 4) as f32 / 3.0 * scale).round() as i32,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
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
        let ui_font = font(9);
        let big_font = font(14);

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
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

        let (st, ed, bt, cb) = (w!("STATIC"), w!("EDIT"), w!("BUTTON"), w!("COMBOBOX"));
        let none = WINDOW_EX_STYLE(0);
        let tab = WS_TABSTOP.0 as i32;
        let m = 12;
        let full = WIDTH - 2 * m;
        let mut y = m;
        let path_text = format!("Settings file: {}", paths.config_file.display());
        b.add(
            Ctl(st, &path_text, SS_PATHELLIPSIS | SS_NOPREFIX, none, -1),
            (m, y, full, 20),
            ui_font,
        )?;
        y += 22;
        let status_style = ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | WS_VSCROLL.0 as i32 | tab;
        let status = b.add(
            Ctl(ed, "", status_style, WS_EX_CLIENTEDGE, ID_STATUS),
            (m, y, full, 130),
            ui_font,
        )?;
        y += 140;
        let bw = 160;
        let buttons = [
            (ID_OPEN, "Open config file"),
            (ID_RESET, "Reset to defaults"),
            (ID_CHECK, "Check again"),
        ];
        for (i, (id, text)) in buttons.into_iter().enumerate() {
            let x = m + i as i32 * (bw + 10);
            b.add(
                Ctl(bt, text, BS_PUSHBUTTON | tab, none, id),
                (x, y, bw, 30),
                ui_font,
            )?;
        }
        y += 40;

        let mut combos = Vec::new();
        let (mut try_in, mut try_out, mut try_alts) = (None, None, None);
        if !errors_only {
            let heading = "Options (saved as soon as you change them):";
            b.add(Ctl(st, heading, 0, none, -1), (m, y, full, 20), ui_font)?;
            y += 28;
            for (i, opt) in FORM_OPTIONS.iter().enumerate() {
                b.add(
                    Ctl(st, opt.label, 0, none, -1),
                    (m, y + 4, 220, 20),
                    ui_font,
                )?;
                let combo_style = CBS_DROPDOWNLIST | tab | WS_VSCROLL.0 as i32;
                let id = ID_COMBO_BASE + i as i32;
                let combo = b.add(
                    Ctl(cb, "", combo_style, none, id),
                    (m + 230, y, full - 230, 200),
                    ui_font,
                )?;
                for label in (0..opt.len()).filter_map(|i| opt.label_of(i)) {
                    let s = HSTRING::from(label);
                    SendMessageW(combo, CB_ADDSTRING, None, Some(LPARAM(s.as_ptr() as isize)));
                }
                combos.push(combo);
                y += 30;
            }
            y += 8;
            let heading = "Try typing (Latin letters; nothing you type here is saved):";
            b.add(Ctl(st, heading, 0, none, -1), (m, y, full, 20), ui_font)?;
            y += 22;
            let in_style = ES_AUTOHSCROLL | tab;
            try_in = Some(b.add(
                Ctl(ed, "", in_style, WS_EX_CLIENTEDGE, ID_TRY_IN),
                (m, y, full, 26),
                ui_font,
            )?);
            y += 34;
            let out_style = ES_READONLY | ES_RIGHT | ES_AUTOHSCROLL | WS_BORDER.0 as i32;
            try_out = Some(b.add(
                Ctl(ed, "", out_style, WS_EX_RTLREADING, ID_TRY_OUT),
                (m, y, full, 40),
                big_font,
            )?);
            y += 46;
            try_alts = Some(b.add(Ctl(st, "", 0, none, ID_TRY_ALTS), (m, y, full, 20), ui_font)?);
            y += 30;
        }
        let note = b.add(Ctl(st, "", 0, none, ID_NOTE), (m, y, full, 20), ui_font)?;
        y += 20 + m;

        let mut rc = RECT {
            left: 0,
            top: 0,
            right: b.px(WIDTH),
            bottom: b.px(y),
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

        let state = Rc::new(App {
            paths,
            status,
            note,
            combos,
            try_in,
            try_out,
            try_alts,
            cfg: RefCell::new(Arc::new(ResolvedConfig::default())),
            fonts: vec![ui_font, big_font],
        });
        APP.with(|a| *a.borrow_mut() = Some(state.clone()));
        refresh(&state);
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
        APP.with(|a| a.borrow_mut().take());
        for f in &state.fonts {
            let _ = DeleteObject(HGDIOBJ(f.0));
        }
        Ok(())
    }
}

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

/// Re-reads the file, updates the status text, the form and the preview, and keeps
/// `config-errors.txt` in sync.
fn refresh(app: &App) {
    let status = match Check::load(&app.paths.config_file) {
        Ok(check) => {
            let _ = write_errors_file(
                &app.paths.errors_file,
                &app.paths.config_file,
                check.issues(),
            );
            let text = if check.state == FileState::Missing {
                "There is no settings file yet, so the default settings are in use. Change an \
                 option below or click \"Open config file\" to create one."
                    .to_string()
            } else if check.issues().is_empty() {
                "Your settings file has no problems.".to_string()
            } else {
                check.issues_text()
            };
            let cfg = Arc::new(check.loaded.config);
            *app.cfg.borrow_mut() = cfg;
            text
        }
        Err(e) => format!("The settings file couldn't be read: {e}"),
    };
    set_text(app.status, &status);
    let cfg = app.cfg.borrow().clone();
    for (opt, &combo) in FORM_OPTIONS.iter().zip(&app.combos) {
        let index = opt.current(&cfg).map_or(usize::MAX, |i| i);
        // SAFETY: valid combo handle; CB_SETCURSEL does not notify the parent.
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), None);
        }
    }
    update_preview(app);
}

fn update_preview(app: &App) {
    let (Some(input), Some(out), Some(alts)) = (app.try_in, app.try_out, app.try_alts) else {
        return;
    };
    let p = preview(app.cfg.borrow().clone(), &get_text(input));
    set_text(out, &p.text_line());
    set_text(alts, &p.alternatives_line());
}

fn notify_saved(app: &App, note: &str) {
    std::thread::spawn(|| {
        let _ = signal_config_changed();
    });
    set_text(app.note, note);
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

fn on_combo(app: &App, hwnd: HWND, i: usize) {
    let (Some(opt), Some(&combo)) = (FORM_OPTIONS.get(i), app.combos.get(i)) else {
        return;
    };
    // SAFETY: valid combo handle.
    let sel = unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) }.0;
    let Some(value) = usize::try_from(sel).ok().and_then(|s| opt.value(s)) else {
        return;
    };
    match save_setting(&app.paths.config_file, Section::Options, opt.key, &value) {
        Ok(()) => notify_saved(app, &format!("Saved: {} changed.", opt.key)),
        Err(e) => {
            message(hwnd, &e.to_string(), false);
        }
    }
    refresh(app);
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

fn handle(hwnd: HWND, msg: u32, wp: WPARAM) -> Option<LRESULT> {
    match msg {
        WM_COMMAND => {
            let app = app()?;
            let id = (wp.0 & 0xFFFF) as i32;
            let code = ((wp.0 >> 16) & 0xFFFF) as u32;
            match (id, code) {
                (ID_OPEN, BN_CLICKED) => on_open(&app, hwnd),
                (ID_RESET, BN_CLICKED) => on_reset(&app, hwnd),
                (ID_CHECK, BN_CLICKED) => {
                    set_text(app.note, "Checked again.");
                    refresh(&app);
                }
                (ID_TRY_IN, EN_CHANGE) => update_preview(&app),
                (id, CBN_SELCHANGE) if id >= ID_COMBO_BASE => {
                    on_combo(&app, hwnd, (id - ID_COMBO_BASE) as usize)
                }
                _ => return None,
            }
            Some(LRESULT(0))
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
    let r = std::panic::catch_unwind(|| handle(hwnd, msg, wp))
        .ok()
        .flatten();
    // SAFETY: forwarding the unmodified message to the default procedure.
    r.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}
