//! Pure popup placement relative to the caret / composition rectangle.

use crate::geometry::{Point, Rect, Size};

#[derive(Clone, Copy, Debug)]
pub struct PlacementInput {
    /// Caret or composition rectangle, screen coordinates.
    pub anchor: Rect,
    pub size: Size,
    /// Work area of the monitor containing the anchor.
    pub work: Rect,
    /// Right-align to the anchor (RTL) instead of left-align.
    pub rtl: bool,
    /// Vertical gap between anchor and popup.
    pub gap: i32,
    /// Try above the anchor first (still flips below when there is no room above).
    pub prefer_above: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub origin: Point,
    pub above: bool,
}

pub fn place(i: &PlacementInput) -> Placement {
    let (a, w, h, work) = (i.anchor, i.size.w, i.size.h, i.work);

    let mut x = if i.rtl { a.right - w } else { a.left };
    x = x.min(work.right - w).max(work.left);

    let below_y = a.bottom + i.gap;
    let above_y = a.top - i.gap - h;
    let fits_below = below_y + h <= work.bottom;
    let fits_above = above_y >= work.top;
    let (fits_first, fits_second) = if i.prefer_above {
        (fits_above, fits_below)
    } else {
        (fits_below, fits_above)
    };
    let above = if fits_first {
        i.prefer_above
    } else if fits_second {
        !i.prefer_above
    } else {
        // Neither side fits: take the roomier one, then clamp below.
        a.top - work.top > work.bottom - a.bottom
    };
    let y = if above { above_y } else { below_y };
    let y = y.min(work.bottom - h).max(work.top);

    Placement {
        origin: Point { x, y },
        above,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1920, 1040);

    fn input(anchor: Rect, rtl: bool) -> PlacementInput {
        PlacementInput {
            anchor,
            size: Size { w: 120, h: 200 },
            work: WORK,
            rtl,
            gap: 2,
            prefer_above: false,
        }
    }

    #[test]
    fn below_right_aligned() {
        let p = place(&input(Rect::new(500, 100, 560, 120), true));
        assert_eq!(p.origin, Point { x: 440, y: 122 });
        assert!(!p.above);
    }

    #[test]
    fn left_aligned_when_ltr() {
        assert_eq!(
            place(&input(Rect::new(500, 100, 560, 120), false)).origin.x,
            500
        );
    }

    #[test]
    fn flips_above() {
        let p = place(&input(Rect::new(500, 900, 560, 920), true));
        assert!(p.above);
        assert_eq!(p.origin.y, 900 - 2 - 200);
    }

    #[test]
    fn prefers_above_when_asked() {
        let mut i = input(Rect::new(500, 400, 560, 420), true);
        i.prefer_above = true;
        let p = place(&i);
        assert!(p.above);
        assert_eq!(
            p.origin,
            Point {
                x: 440,
                y: 400 - 2 - 200
            }
        );
    }

    #[test]
    fn prefer_above_flips_below_near_top() {
        let mut i = input(Rect::new(500, 50, 560, 70), true);
        i.prefer_above = true;
        let p = place(&i);
        assert!(!p.above);
        assert_eq!(p.origin.y, 72);
    }

    #[test]
    fn prefer_above_no_room_either_side_takes_roomier() {
        let mut i = input(Rect::new(500, 300, 560, 320), true);
        i.prefer_above = true;
        i.size.h = 800;
        let p = place(&i);
        assert!(!p.above, "more room below");
        assert_eq!(p.origin.y, 1040 - 800);
    }

    #[test]
    fn clamps_to_edges() {
        let p = place(&input(Rect::new(1900, 100, 1910, 120), false));
        assert_eq!(p.origin.x, 1800);
        let p = place(&input(Rect::new(10, 100, 30, 120), true));
        assert_eq!(p.origin.x, 0);
    }

    #[test]
    fn no_room_either_side_clamps() {
        // Taller than half the screen: more room below, so below, clamped up to fit.
        let mut i = input(Rect::new(500, 400, 560, 420), true);
        i.size.h = 700;
        let p = place(&i);
        assert!(!p.above);
        assert_eq!(p.origin.y, 1040 - 700);
        // Anchor near the bottom, popup too tall for above: pinned to the work-area top.
        let mut i = input(Rect::new(500, 900, 560, 920), true);
        i.size.h = 950;
        let p = place(&i);
        assert!(p.above);
        assert_eq!(p.origin.y, 0);
    }

    #[test]
    fn second_monitor_with_negative_origin() {
        let work = Rect::new(-1920, 0, 0, 1040);
        let i = PlacementInput {
            work,
            ..input(Rect::new(-30, 100, -10, 120), false)
        };
        assert_eq!(place(&i).origin.x, -120);
    }

    #[test]
    fn wider_than_work_area_pins_left() {
        let mut i = input(Rect::new(500, 100, 560, 120), true);
        i.size.w = 3000;
        assert_eq!(place(&i).origin.x, 0);
    }
}
