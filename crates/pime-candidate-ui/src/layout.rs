//! Pure layout and hit-testing. Everything is computed in physical pixels from DIP
//! constants and a scale factor (dpi / 96); text widths come from the renderer.

use crate::geometry::{Rect, Size};

pub const MAX_ROWS: usize = 9;

/// Design constants in DIPs (96-dpi pixels).
pub mod dip {
    /// Inset between the 1px frame and the rows.
    pub const OUTER_PAD: f32 = 3.0;
    pub const ROW_PAD_X: f32 = 10.0;
    pub const ROW_PAD_Y: f32 = 2.0;
    pub const NUMBER_W: f32 = 18.0;
    /// Between the number column and the glyph.
    pub const COL_GAP: f32 = 10.0;
    /// Between the glyph and the learner label at the other end of the row.
    pub const LABEL_GAP: f32 = 18.0;
    pub const GLYPH_LINE: f32 = 36.0;
    pub const FOOTER_H: f32 = 20.0;
    /// Space above and below the 1px divider between alternates and completions.
    pub const DIVIDER_PAD: f32 = 2.0;
    pub const MIN_WIDTH: f32 = 112.0;
    /// Row highlight corner radius.
    pub const RADIUS: f32 = 3.0;
    pub const GLYPH_FONT: f32 = 24.0;
    pub const LABEL_FONT: f32 = 12.0;
    pub const NUMBER_FONT: f32 = 13.0;
    pub const FOOTER_FONT: f32 = 11.0;
}

pub fn px(dip: f32, scale: f32) -> i32 {
    (dip * scale).round() as i32
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutInput {
    pub rows: usize,
    pub show_numbers: bool,
    pub has_labels: bool,
    pub footer: bool,
    /// Draw a divider after this many rows (the alternates) when completions follow.
    pub divider_after: Option<usize>,
    /// Widest glyph run / label run in physical pixels, from DirectWrite.
    pub glyph_width: f32,
    pub label_width: f32,
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowLayout {
    pub row: Rect,
    /// Zero-width rect when numbers are hidden.
    pub number: Rect,
    pub glyph: Rect,
    /// Left end of the row (the far side from the number in RTL).
    pub label: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Layout {
    pub size: Size,
    pub rows: Vec<RowLayout>,
    /// Footer text rect (row padding applied) in the band along the bottom edge.
    pub footer: Option<Rect>,
    /// The 1px divider line between alternates and completions.
    pub divider: Option<Rect>,
}

impl Layout {
    pub fn compute(inp: &LayoutInput) -> Layout {
        let s = inp.scale;
        let outer = px(dip::OUTER_PAD, s);
        let pad_x = px(dip::ROW_PAD_X, s);
        let pad_y = px(dip::ROW_PAD_Y, s);
        let num_w = if inp.show_numbers {
            px(dip::NUMBER_W, s)
        } else {
            0
        };
        let gap = if inp.show_numbers {
            px(dip::COL_GAP, s)
        } else {
            0
        };
        let (label_w, label_gap) = if inp.has_labels {
            (inp.label_width.ceil() as i32, px(dip::LABEL_GAP, s))
        } else {
            (0, 0)
        };
        let row_h = px(dip::GLYPH_LINE, s) + 2 * pad_y;
        let footer_h = if inp.footer { px(dip::FOOTER_H, s) } else { 0 };
        let rows_n = inp.rows.min(MAX_ROWS);
        // Only between two non-empty sections.
        let divider_after = inp.divider_after.filter(|&d| d > 0 && d < rows_n);
        let div_pad = px(dip::DIVIDER_PAD, s);
        let divider_band = if divider_after.is_some() {
            2 * div_pad + 1
        } else {
            0
        };

        let content = inp.glyph_width.ceil() as i32 + label_w + label_gap;
        let width = (2 * outer + 2 * pad_x + num_w + gap + content).max(px(dip::MIN_WIDTH, s));
        let rows_bottom = outer + row_h * rows_n as i32 + divider_band;
        let height = rows_bottom + outer + footer_h;
        let row_top = |i: usize| {
            let below = divider_after.is_some_and(|d| i >= d);
            outer + row_h * i as i32 + if below { divider_band } else { 0 }
        };

        let rows = (0..rows_n)
            .map(|i| {
                let top = row_top(i);
                let row = Rect::new(outer, top, width - outer, top + row_h);
                // RTL: the number column sits on the right edge, text right-aligned beside it.
                let number = Rect::new(
                    row.right - pad_x - num_w,
                    top,
                    row.right - pad_x,
                    top + row_h,
                );
                let text_right = if inp.show_numbers {
                    number.left - gap
                } else {
                    row.right - pad_x
                };
                let label_left = row.left + pad_x;
                let glyph_left = label_left + label_w + label_gap;
                let glyph = Rect::new(glyph_left, top + pad_y, text_right, top + row_h - pad_y);
                let label = inp
                    .has_labels
                    .then(|| Rect::new(label_left, top, label_left + label_w, top + row_h));
                RowLayout {
                    row,
                    number,
                    glyph,
                    label,
                }
            })
            .collect();

        let footer = inp.footer.then(|| {
            Rect::new(
                outer + pad_x,
                rows_bottom + outer,
                width - outer - pad_x,
                height,
            )
        });

        let divider = divider_after.map(|d| {
            let y = row_top(d) - divider_band + div_pad;
            Rect::new(outer + pad_x, y, width - outer - pad_x, y + 1)
        });

        Layout {
            size: Size {
                w: width,
                h: height,
            },
            rows,
            footer,
            divider,
        }
    }

    /// Row index under a client-space point.
    pub fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
        self.rows.iter().position(|r| r.row.contains(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(rows: usize, labels: bool, scale: f32) -> LayoutInput {
        LayoutInput {
            rows,
            show_numbers: true,
            has_labels: labels,
            footer: false,
            divider_after: None,
            glyph_width: 20.0 * scale,
            label_width: if labels { 30.0 * scale } else { 0.0 },
            scale,
        }
    }

    // Row height at 1x: 36 + 2 * 2.
    const ROW: i32 = 40;

    #[test]
    fn heights_and_rows() {
        let l = Layout::compute(&input(3, false, 1.0));
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.size.h, 6 + 3 * ROW);
        assert_eq!(l.rows[1].row.top, 3 + ROW);
        assert_eq!(l.footer, None);
        // Labels sit beside the glyph, so they do not add height.
        let l2 = Layout::compute(&input(3, true, 1.0));
        assert_eq!(l2.size.h, l.size.h);
        assert!(l2.rows[0].label.is_some());
    }

    #[test]
    fn footer_band() {
        let mut i = input(2, false, 1.0);
        i.footer = true;
        let l = Layout::compute(&i);
        assert_eq!(l.size.h, 6 + 2 * ROW + 20);
        let f = l.footer.unwrap();
        assert_eq!(f.top, 3 + 2 * ROW + 3);
        assert_eq!(f.bottom, l.size.h);
        assert_eq!(f.left, 13);
        assert_eq!(l.hit_test(50, f.top + 5), None, "footer is not a row");
    }

    #[test]
    fn scales_with_dpi() {
        let a = Layout::compute(&input(2, false, 1.0));
        let b = Layout::compute(&input(2, false, 2.0));
        assert_eq!(b.size.h, a.size.h * 2);
        assert!(b.size.w > a.size.w);
    }

    #[test]
    fn caps_rows_and_min_width() {
        let l = Layout::compute(&input(20, false, 1.0));
        assert_eq!(l.rows.len(), MAX_ROWS);
        let mut i = input(1, false, 1.0);
        i.glyph_width = 1.0;
        assert_eq!(Layout::compute(&i).size.w, 112);
        i.glyph_width = 400.0;
        assert_eq!(Layout::compute(&i).size.w, 2 * 3 + 2 * 10 + 18 + 10 + 400);
    }

    #[test]
    fn rtl_columns() {
        let mut i = input(1, true, 1.0);
        i.glyph_width = 200.0;
        let l = Layout::compute(&i);
        let r = &l.rows[0];
        assert!(r.number.left > r.glyph.right, "number column right of text");
        assert_eq!(r.number.right, r.row.right - 10);
        let lab = r.label.unwrap();
        assert_eq!(lab.left, r.row.left + 10);
        assert_eq!(lab.width(), 30);
        assert_eq!(r.glyph.left, lab.right + 18, "label at the far (left) end");
        assert!(r.glyph.width() >= 200);
        let mut i = input(1, false, 1.0);
        i.show_numbers = false;
        let r = &Layout::compute(&i).rows[0];
        assert_eq!(r.number.width(), 0);
        assert_eq!(r.glyph.right, r.row.right - 10);
        assert_eq!(r.glyph.left, r.row.left + 10);
    }

    #[test]
    fn hit_testing() {
        let l = Layout::compute(&input(3, false, 1.0));
        assert_eq!(l.hit_test(50, 3), Some(0));
        assert_eq!(l.hit_test(50, 3 + ROW - 1), Some(0));
        assert_eq!(l.hit_test(50, 3 + ROW), Some(1));
        assert_eq!(l.hit_test(50, 2), None, "outer padding");
        assert_eq!(l.hit_test(2, 10), None, "left padding");
        assert_eq!(l.hit_test(50, l.size.h), None);
        assert_eq!(l.hit_test(-5, -5), None);
        assert_eq!(Layout::default().hit_test(0, 0), None);
    }

    #[test]
    fn divider_between_sections() {
        let mut i = input(5, false, 1.0);
        i.divider_after = Some(3);
        let l = Layout::compute(&i);
        // Band = 2 + 1 + 2 at 1x.
        assert_eq!(l.size.h, 6 + 5 * ROW + 5);
        let d = l.divider.unwrap();
        assert_eq!(d.height(), 1);
        assert_eq!(d.top, 3 + 3 * ROW + 2);
        assert_eq!((d.left, d.right), (13, l.size.w - 13));
        assert_eq!(l.rows[2].row.bottom, 3 + 3 * ROW);
        assert_eq!(
            l.rows[3].row.top,
            3 + 3 * ROW + 5,
            "completions start below the band"
        );
        assert_eq!(l.rows[4].row.bottom, l.size.h - 3);
        assert_eq!(l.hit_test(50, d.top), None, "the divider is not a row");
        assert_eq!(l.hit_test(50, l.rows[3].row.top), Some(3));
        assert_eq!(l.hit_test(50, l.rows[3].row.top - 1), None);
        assert_eq!(l.hit_test(50, l.rows[4].row.top), Some(4));

        // No divider without both sections.
        for after in [Some(0), Some(5), Some(9), None] {
            i.divider_after = after;
            let l = Layout::compute(&i);
            assert_eq!(l.divider, None, "{after:?}");
            assert_eq!(l.size.h, 6 + 5 * ROW);
        }
        let mut i = input(20, false, 1.0);
        i.divider_after = Some(7);
        let l = Layout::compute(&i);
        assert_eq!(l.rows.len(), MAX_ROWS);
        assert!(l.divider.is_some());
        assert_eq!(l.size.h, 6 + 9 * ROW + 5);
    }

    #[test]
    fn divider_scales_but_stays_one_pixel() {
        let mut i = input(2, false, 2.0);
        i.divider_after = Some(1);
        let l = Layout::compute(&i);
        assert_eq!(l.divider.unwrap().height(), 1);
        assert_eq!(l.rows[1].row.top - l.rows[0].row.bottom, 4 + 1 + 4);
    }
}
