//! Where the candidate popup goes: the composition's screen rectangle (PLAN 5.3), converted to
//! physical pixels for the presenter (`presenter::ScreenRect` contract). The DPI conversion and
//! the caret fallback are stream B's helpers in `candidates.rs`.

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::TextServices::{ITfContext, ITfRange};
use windows_core::BOOL;

use crate::candidates::{caret_fallback_anchor, is_degenerate, to_physical};
use crate::presenter::ScreenRect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub rect: ScreenRect,
    /// False when GetTextExt had no layout yet (TS_E_NOLAYOUT or an empty rect) and the caret
    /// fallback was used: the caller retries from ITfTextLayoutSink::OnLayoutChange.
    pub exact: bool,
}

/// # Safety
/// Must run inside an edit session holding `ec` on `ctx`.
pub unsafe fn compute(ctx: &ITfContext, ec: u32, range: Option<&ITfRange>) -> Anchor {
    if let (Ok(view), Some(range)) = (ctx.GetActiveView(), range) {
        let mut rc = RECT::default();
        let mut clipped = BOOL::default();
        if view.GetTextExt(ec, range, &mut rc, &mut clipped).is_ok() {
            let r = ScreenRect {
                left: rc.left,
                top: rc.top,
                right: rc.right,
                bottom: rc.bottom,
            };
            if !is_degenerate(r) {
                let hwnd = view.GetWnd().unwrap_or_default();
                let rect = if hwnd.is_invalid() {
                    r
                } else {
                    to_physical(hwnd, r)
                };
                return Anchor { rect, exact: true };
            }
        }
    }
    Anchor {
        rect: caret_fallback_anchor().unwrap_or_default(),
        exact: false,
    }
}
