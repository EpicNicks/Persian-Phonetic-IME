//! Interactive demo driven by the real engine: keys typed into the host window go through a
//! `pime_core::Session` (default preset), the host draws the preedit and the committed text,
//! and the popup shows the session's candidate list anchored below the preedit, exactly the
//! data the TIP will show. The popup itself never takes focus.
//!
//!   a-z ' Tab Down Up 1-9 Space Enter Backspace Escape   as in the IME (PLAN.md 2.2)
//!   click a candidate   pick it (Session::select_candidate)
//!   F2 labels   F3 prefer above   F4 footer   F5 clear text   F6 word completion on/off
//!   F10 or close: quit
//!
//! Word completion uses a small fixed word list (`pime_core::ListCompleter`): type `sl` to
//! see completions below the divider; Down/Up move into them, Enter or a digit accepts.
//!
//! `--smoke` runs the scripted flow `s`, `Tab`, `2`, `z`, `Down`, `Down`, `Up`, `Space`
//! (one key per 150 ms, printing each state), checks the popup follows it and exits.
//! `--smoke --complete` runs `s`, `l`, `Down`, `Down`, `Up`, `Enter` with completion on.
//! `--smoke --pause N` holds for 3 s after scripted step N (0-based), e.g. for a screenshot.

#[cfg(not(windows))]
fn main() {
    eprintln!("the candidate window demo is Windows-only");
}

#[cfg(windows)]
fn main() {
    std::process::exit(demo::run());
}

#[cfg(windows)]
mod demo {
    use std::cell::RefCell;
    use std::sync::Arc;

    use pime_candidate_ui::{CandidateEvent, CandidateView, CandidateWindow, Rect};
    use pime_core::{
        keys, load_config, CandidateList, Completer, CompletionList, Dictionary, KeyEvent,
        ListCompleter, Output, Session,
    };
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const SCRIPT: &[&str] = &["s", "Tab", "2", "z", "Down", "Down", "Up", "Space"];
    const COMPLETE_SCRIPT: &[&str] = &["s", "l", "Down", "Down", "Up", "Enter"];
    /// Demo word list (word, Zipf-like score).
    const WORDS: &[(&str, f32)] = &[
        ("سلام", 5.0),
        ("سلامت", 4.2),
        ("سلطان", 4.0),
        ("سلیقه", 3.5),
        ("زبان", 5.1),
        ("زندگی", 5.3),
        ("صدا", 4.6),
        ("صبح", 4.8),
        ("می\u{200C}خواهم", 4.8),
        ("میز", 4.5),
    ];
    const VK_F6: u32 = 0x75;
    const VK_F7: u32 = 0x76;
    const VK_F2: u32 = 0x71;
    const VK_F3: u32 = 0x72;
    const VK_F4: u32 = 0x73;
    const VK_F5: u32 = 0x74;
    const VK_F10: u32 = 0x79;

    struct Demo {
        host: HWND,
        popup: CandidateWindow,
        session: Session,
        committed: String,
        preedit: String,
        list: Option<CandidateList>,
        completions: Option<CompletionList>,
        complete: bool,
        labels: bool,
        footer: bool,
        prefer_above: bool,
        /// Screen rect of the preedit as last painted.
        anchor: Rect,
    }

    thread_local! {
        static DEMO: RefCell<Option<Demo>> = const { RefCell::new(None) };
    }

    fn with_demo<R>(f: impl FnOnce(&mut Demo) -> R) -> Option<R> {
        DEMO.with(|d| d.try_borrow_mut().ok()?.as_mut().map(f))
    }

    /// The same mapping the TIP's presenter uses (`pime_tip::candidates::to_view_full`).
    fn to_view(
        alts: Option<&CandidateList>,
        comps: Option<&CompletionList>,
        preedit: &str,
        labels: bool,
        footer: bool,
    ) -> CandidateView {
        let mut v = CandidateView {
            show_numbers: true,
            show_footer: footer,
            ..Default::default()
        };
        if let Some(list) = alts {
            v.items = list.items.clone();
            v.highlighted = list.highlighted;
            v.labels = labels.then(|| {
                list.items
                    .iter()
                    .map(|c| pime_core::candidate_label(c).unwrap_or("").to_owned())
                    .collect()
            });
        }
        if let Some(c) = comps {
            v.completions = c.items.clone();
            v.completion_highlight = c.highlighted;
            let typed = !preedit.is_empty() && c.items.iter().all(|w| w.starts_with(preedit));
            v.completion_prefix_chars = if typed { preedit.chars().count() } else { 0 };
            v.completion_fuzzy = c.edits.iter().map(|&e| e > 0).collect();
        }
        v
    }

    fn new_session(complete: bool) -> Session {
        let toml = if complete {
            "[autocomplete]\nenabled = true\n"
        } else {
            ""
        };
        let mut s = Session::new(
            Arc::new(load_config(toml).config),
            Arc::new(Dictionary::empty()),
        );
        if complete {
            let words = WORDS.iter().map(|(w, f)| ((*w).to_owned(), *f));
            s.set_completer(Some(
                Arc::new(ListCompleter::new(words)) as Arc<dyn Completer>
            ));
        }
        s
    }

    fn key_event(name: &str) -> KeyEvent {
        let vk = match name {
            "Tab" => keys::VK_TAB,
            "Down" => keys::VK_DOWN,
            "Up" => keys::VK_UP,
            "Space" => keys::VK_SPACE,
            "Enter" => keys::VK_RETURN,
            "Escape" => keys::VK_ESCAPE,
            "Backspace" => keys::VK_BACK,
            s => {
                let c = s.chars().next().expect("key");
                return match keys::vk_for_char(c) {
                    Some((vk, true)) => KeyEvent::shifted(vk),
                    Some((vk, false)) => KeyEvent::plain(vk),
                    None => KeyEvent::plain(0),
                };
            }
        };
        KeyEvent::plain(vk)
    }

    /// Apply a session `Output` the way the TIP does: commit, preedit, then the list.
    /// A highlight-only change is sent as `set_highlight` (no relayout).
    fn apply(d: &mut Demo, out: Output) {
        d.committed.push_str(&out.commit);
        d.preedit = out.preedit;
        // SAFETY: repaint the host so the anchor below reflects the new preedit.
        unsafe {
            let _ = InvalidateRect(Some(d.host), None, true);
            let _ = UpdateWindow(d.host);
        }
        let alts = out.candidates.as_ref().filter(|l| !l.items.is_empty());
        let comps = out.completions.as_ref().filter(|c| !c.items.is_empty());
        if alts.is_some() || comps.is_some() {
            let view = to_view(alts, comps, &d.preedit, d.labels, d.footer);
            let prev_comps = d.completions.as_ref().map(|c| &c.items);
            let same_rows = d.list.as_ref().map(|l| &l.items) == alts.map(|l| &l.items)
                && prev_comps == comps.map(|c| &c.items);
            if same_rows && d.popup.is_visible() {
                d.popup.set_selected_row(view.selected_row());
            } else {
                d.popup.update(&view);
            }
            d.popup.show_at(d.anchor, d.prefer_above);
        } else {
            d.popup.hide();
        }
        d.list = out.candidates;
        d.completions = out.completions;
    }

    fn refresh(d: &mut Demo) {
        let out = d.session.snapshot();
        d.list = None;
        d.completions = None;
        apply(d, out);
    }

    fn describe(d: &Demo) -> String {
        if let Some(c) = &d.completions {
            return format!(
                "preedit={:?} alts={:?} completions={:?} completion_highlight={:?} popup_row={:?} committed={:?}",
                d.preedit,
                d.list.as_ref().map(|l| &l.items),
                c.items,
                c.highlighted,
                d.popup.selected_row(),
                d.committed
            );
        }
        match &d.list {
            Some(l) => format!(
                "preedit={:?} list={:?} highlighted={} popup_visible={} popup_highlight={} committed={:?}",
                d.preedit, l.items, l.highlighted, d.popup.is_visible(), d.popup.highlight(), d.committed
            ),
            None => format!(
                "preedit={:?} list=none popup_visible={} committed={:?}",
                d.preedit,
                d.popup.is_visible(),
                d.committed
            ),
        }
    }

    /// Completion smoke: `s l Down Down Up Enter` over `WORDS` (best first: سلام, سلامت, ...).
    fn check_complete(step: usize, d: &Demo) -> Result<(), String> {
        let n = d.list.as_ref().map_or(0, |l| l.items.len());
        let comps = d.completions.as_ref();
        let first = comps.and_then(|c| c.items.first()).map(String::as_str);
        let hl = comps.and_then(|c| c.highlighted);
        let ok = match step {
            0 => d.popup.is_visible() && comps.is_none(),
            1 => d.popup.is_visible() && first == Some("سلام") && hl.is_none(),
            2 | 4 => hl == Some(0) && d.popup.selected_row() == Some(n),
            3 => hl == Some(1) && d.popup.selected_row() == Some(n + 1),
            5 => !d.popup.is_visible() && d.committed == "سلام",
            _ => true,
        };
        if ok {
            Ok(())
        } else {
            Err(format!(
                "step {step} ({}): {}",
                COMPLETE_SCRIPT[step],
                describe(d)
            ))
        }
    }

    /// Smoke expectations after each scripted key: (list highlight or None, committed).
    fn check(step: usize, d: &Demo) -> Result<(), String> {
        let hl = d.list.as_ref().map(|l| l.highlighted);
        let ok = match step {
            0 => hl == Some(0) && d.popup.is_visible() && d.preedit == "س",
            1 => hl == Some(1) && d.popup.highlight() == 1 && d.preedit == "ص",
            2 => hl.is_none() && !d.popup.is_visible() && d.committed == "ص",
            3 => hl == Some(0) && d.popup.is_visible(),
            4 | 6 => hl == Some(1) && d.popup.highlight() == 1,
            5 => hl == Some(2) && d.popup.highlight() == 2,
            7 => !d.popup.is_visible() && d.committed == "صذ ",
            _ => true,
        };
        if ok {
            Ok(())
        } else {
            Err(format!("step {step} ({}): {}", SCRIPT[step], describe(d)))
        }
    }

    fn font(hwnd: HWND, px96: i32) -> HFONT {
        // SAFETY: plain GDI font creation.
        unsafe {
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            CreateFontW(
                -(px96 * dpi / 96),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            )
        }
    }

    /// Draw committed text + underlined preedit right-aligned (RTL) and record the preedit's
    /// screen rect as the popup anchor.
    fn paint(hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: standard paint cycle.
        unsafe {
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let small = font(hwnd, 13);
            let big = font(hwnd, 30);
            let old = SelectObject(hdc, small.into());
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x606060));
            let mut help: Vec<u16> = "Type Latin letters (s, z, t, h, a, gh ...); try \"sl\" for word completions. Tab/Down/Up move, 1-9 or click picks, Space commits. F2 labels, F3 above, F4 footer, F5 clear, F6 completion on/off, F10 quits."
                .encode_utf16()
                .collect();
            let mut hr = RECT {
                left: 16,
                top: 10,
                right: rc.right - 16,
                bottom: 80,
            };
            DrawTextW(hdc, &mut help, &mut hr, DT_WORDBREAK);

            SelectObject(hdc, big.into());
            SetTextColor(hdc, COLORREF(0x202020));
            let (committed, preedit) =
                with_demo(|d| (d.committed.clone(), d.preedit.clone())).unwrap_or_default();
            let c: Vec<u16> = committed.encode_utf16().collect();
            let p: Vec<u16> = preedit.encode_utf16().collect();
            let mut ps_size = SIZE::default();
            let mut cs_size = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &p, &mut ps_size);
            let _ = GetTextExtentPoint32W(hdc, &c, &mut cs_size);
            let base_y = 110;
            let right = rc.right - 40;
            // RTL line: committed text at the right, the preedit continues to its left.
            let pre_right = right - cs_size.cx;
            let pre_left = pre_right - ps_size.cx.max(2);
            SetTextAlign(hdc, TA_RTLREADING);
            let _ = TextOutW(hdc, pre_right, base_y, &c);
            let _ = TextOutW(hdc, pre_left, base_y, &p);
            let line_h = ps_size.cy.max(cs_size.cy).max(30);
            let underline = CreateSolidBrush(COLORREF(0x202020));
            let ur = RECT {
                left: pre_left,
                top: base_y + line_h - 2,
                right: pre_right,
                bottom: base_y + line_h,
            };
            if !p.is_empty() {
                FillRect(hdc, &ur, underline);
            }
            let _ = DeleteObject(underline.into());
            SelectObject(hdc, old);
            let _ = DeleteObject(small.into());
            let _ = DeleteObject(big.into());
            let _ = EndPaint(hwnd, &ps);

            let mut tl = POINT {
                x: pre_left,
                y: base_y,
            };
            let mut br = POINT {
                x: pre_right,
                y: base_y + line_h,
            };
            let _ = ClientToScreen(hwnd, &mut tl);
            let _ = ClientToScreen(hwnd, &mut br);
            with_demo(|d| d.anchor = Rect::new(tl.x, tl.y, br.x, br.y));
        }
    }

    extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = wp.0 as u32;
                // SAFETY: keyboard state queries.
                let (shift, ctrl) = unsafe {
                    (
                        GetKeyState(VK_SHIFT.0 as i32) < 0,
                        GetKeyState(VK_CONTROL.0 as i32) < 0,
                    )
                };
                with_demo(|d| match vk {
                    VK_F10 => unsafe { PostQuitMessage(0) },
                    VK_F2 => {
                        d.labels = !d.labels;
                        refresh(d);
                    }
                    VK_F3 => {
                        d.prefer_above = !d.prefer_above;
                        refresh(d);
                    }
                    VK_F4 => {
                        d.footer = !d.footer;
                        refresh(d);
                    }
                    VK_F5 => {
                        d.committed.clear();
                        refresh(d);
                    }
                    // Static fuzzy scenario: typed "سلم", two exact rows, then two approximate
                    // rows (no dimming, marker at the far end).
                    VK_F7 => {
                        let view = CandidateView {
                            show_numbers: true,
                            show_footer: d.footer,
                            completions: ["سلمان", "سلمه", "سلام", "سلیم"]
                                .map(str::to_owned)
                                .to_vec(),
                            completion_highlight: Some(2),
                            completion_prefix_chars: 3,
                            completion_fuzzy: vec![false, false, true, true],
                            ..Default::default()
                        };
                        d.popup.update(&view);
                        d.popup.show_at(d.anchor, d.prefer_above);
                    }
                    VK_F6 => {
                        d.complete = !d.complete;
                        d.session = new_session(d.complete);
                        d.preedit.clear();
                        refresh(d);
                    }
                    _ => {
                        let k = KeyEvent {
                            vk,
                            shift,
                            ctrl,
                            alt: msg == WM_SYSKEYDOWN,
                        };
                        let out = d.session.on_key(&k);
                        apply(d, out);
                    }
                });
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: ends the loop.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            // SAFETY: default handling.
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    pub fn run() -> i32 {
        let args: Vec<String> = std::env::args().collect();
        let smoke = args.iter().any(|a| a == "--smoke");
        // Interactive runs start with completion on; the classic smoke script runs without.
        let complete = !smoke || args.iter().any(|a| a == "--complete");
        let script = if smoke && complete {
            COMPLETE_SCRIPT
        } else {
            SCRIPT
        };
        let pause_after = args
            .iter()
            .position(|a| a == "--pause")
            .and_then(|i| args.get(i + 1)?.parse::<usize>().ok());
        // SAFETY: process-wide DPI awareness set once, before any window exists.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let popup = CandidateWindow::new(|ev| match ev {
            CandidateEvent::Hover(i) => println!("hover {i}"),
            CandidateEvent::Click(i) => {
                println!("click {i}");
                // The window callback runs inside a window message, not inside a Demo
                // borrow, so the session can be driven from here.
                with_demo(|d| {
                    let out = d.session.select_candidate(i);
                    apply(d, out);
                });
            }
        })
        .expect("create candidate window");

        let mut failures = Vec::new();
        // SAFETY: plain Win32 host window and message loop.
        unsafe {
            let hinst = GetModuleHandleW(PCWSTR::null()).expect("module").into();
            let class = w!("PimeCandidateDemoHost");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(host_proc),
                hInstance: hinst,
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW).expect("cursor"),
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                ..Default::default()
            });
            let host = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Persian Phonetic IME: candidate popup demo (real Session data)"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                820,
                420,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("host window");

            DEMO.with(|d| {
                *d.borrow_mut() = Some(Demo {
                    host,
                    popup,
                    session: new_session(complete),
                    committed: String::new(),
                    preedit: String::new(),
                    list: None,
                    completions: None,
                    complete,
                    labels: true,
                    footer: true,
                    prefer_above: false,
                    anchor: Rect::new(400, 300, 401, 330),
                })
            });
            let _ = UpdateWindow(host);
            if smoke {
                SetTimer(Some(host), 1, 150, None);
            }

            let mut msg = MSG::default();
            let mut step = 0;
            let mut held = 0;
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if smoke && msg.message == WM_TIMER {
                    if pause_after.is_some_and(|p| step == p + 1) && held < 20 {
                        held += 1;
                    } else if step < script.len() {
                        let key = script[step];
                        let res = with_demo(|d| {
                            let out = d.session.on_key(&key_event(key));
                            apply(d, out);
                            println!("{key:>6} -> {}", describe(d));
                            if complete {
                                check_complete(step, d)
                            } else {
                                check(step, d)
                            }
                        });
                        if let Some(Err(e)) = res {
                            failures.push(e);
                        }
                        step += 1;
                    } else {
                        PostQuitMessage(0);
                    }
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = DestroyWindow(host);
        }
        DEMO.with(|d| d.borrow_mut().take());
        for f in &failures {
            eprintln!("smoke check failed: {f}");
        }
        i32::from(!failures.is_empty())
    }
}
