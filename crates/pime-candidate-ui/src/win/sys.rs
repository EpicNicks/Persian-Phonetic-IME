//! Small read-only system queries: theme, high contrast, monitors.

use windows::core::w;
use windows::Win32::Foundation::{ERROR_SUCCESS, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetSysColor, MonitorFromRect, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

use crate::geometry::Rect;
use crate::theme::{theme_from_apps_use_light, Color, Palette, Theme};

fn sys_color(index: windows::Win32::Graphics::Gdi::SYS_COLOR_INDEX) -> Color {
    // SAFETY: trivial query.
    let c = unsafe { GetSysColor(index) };
    Color::rgb(
        (c & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
    )
}

fn high_contrast_on() -> bool {
    let mut hc = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: `hc` is correctly sized for SPI_GETHIGHCONTRAST.
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some(&mut hc as *mut _ as *mut _),
            Default::default(),
        )
        .is_ok()
            && (hc.dwFlags.0 & HCF_HIGHCONTRASTON.0) != 0
    }
}

fn apps_use_light_theme() -> Option<u32> {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: read-only registry query into a correctly sized DWORD.
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (err == ERROR_SUCCESS).then_some(value)
}

/// Current theme and its palette. High contrast wins over the light/dark app setting.
pub fn current_palette() -> (Theme, Palette) {
    if high_contrast_on() {
        let p = Palette::high_contrast(
            sys_color(COLOR_WINDOW),
            sys_color(COLOR_WINDOWTEXT),
            sys_color(COLOR_HIGHLIGHT),
            sys_color(COLOR_HIGHLIGHTTEXT),
            sys_color(COLOR_GRAYTEXT),
        );
        return (Theme::HighContrast, p);
    }
    match theme_from_apps_use_light(apps_use_light_theme()) {
        Theme::Dark => (Theme::Dark, Palette::DARK),
        t => (t, Palette::LIGHT),
    }
}

/// Work area and DPI scale (dpi/96) of the monitor nearest to `r` (screen coordinates).
pub fn monitor_for_rect(r: Rect) -> (Rect, f32) {
    let rc = RECT {
        left: r.left,
        top: r.top,
        right: r.right.max(r.left + 1),
        bottom: r.bottom.max(r.top + 1),
    };
    // SAFETY: valid RECT / out-params.
    unsafe {
        let mon = MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            Rect::new(
                mi.rcWork.left,
                mi.rcWork.top,
                mi.rcWork.right,
                mi.rcWork.bottom,
            )
        } else {
            Rect::new(0, 0, 1024, 768)
        };
        let (mut dx, mut dy) = (96u32, 96u32);
        let scale = if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() && dx > 0
        {
            dx as f32 / 96.0
        } else {
            1.0
        };
        (work, scale)
    }
}

/// Runs the enclosed Win32 calls in a per-monitor-v2 thread DPI context, restoring the host
/// thread's context on drop. Coordinates passed to `SetWindowPos`/`MonitorFromRect` are
/// interpreted in the calling thread's context, and a TIP runs on the host's thread, which
/// may be DPI-unaware.
pub struct DpiScope(DPI_AWARENESS_CONTEXT);

impl DpiScope {
    pub fn per_monitor() -> Self {
        // SAFETY: thread-local setting; restored in Drop. Returns null on pre-1607 Windows.
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiScope {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            // SAFETY: restores the value returned by the matching Set call.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}
