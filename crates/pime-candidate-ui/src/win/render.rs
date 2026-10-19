//! Direct2D / DirectWrite drawing. Owns the factories and the (lazily created) HWND render
//! target. The render target runs at 96 dpi so all coordinates are physical pixels.

use windows::core::BOOL;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
};
const D2DERR_RECREATE_TARGET: windows::core::HRESULT = windows::core::HRESULT(0x8899000Cu32 as i32);
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_LIGHT, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_READING_DIRECTION,
    DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_READING_DIRECTION_RIGHT_TO_LEFT,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::geometry::Rect;
use crate::layout::{dip, Layout};
use crate::theme::{Color, Palette};
use crate::view::CandidateView;

#[derive(Clone)]
struct Formats {
    glyph: IDWriteTextFormat,
    label: IDWriteTextFormat,
    number: IDWriteTextFormat,
    footer: IDWriteTextFormat,
}

pub struct Renderer {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    family: &'static [u16],
    scale: f32,
    formats: Option<Formats>,
    target: Option<ID2D1HwndRenderTarget>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn d2d_color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: 1.0,
    }
}

fn rect_f(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left as f32,
        top: r.top as f32,
        right: r.right as f32,
        bottom: r.bottom as f32,
    }
}

/// Rounded rect inset by half a pixel so a 1px outline lands on whole pixels.
fn rounded(r: Rect, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: r.left as f32 + 0.5,
            top: r.top as f32 + 0.5,
            right: r.right as f32 - 0.5,
            bottom: r.bottom as f32 - 0.5,
        },
        radiusX: radius,
        radiusY: radius,
    }
}

// NUL-terminated so they can be handed to PCWSTR.
const SEGOE: &[u16] = &[
    b'S' as u16,
    b'e' as u16,
    b'g' as u16,
    b'o' as u16,
    b'e' as u16,
    b' ' as u16,
    b'U' as u16,
    b'I' as u16,
    0,
];
const TAHOMA: &[u16] = &[
    b'T' as u16,
    b'a' as u16,
    b'h' as u16,
    b'o' as u16,
    b'm' as u16,
    b'a' as u16,
    0,
];

struct FormatSpec {
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    direction: DWRITE_READING_DIRECTION,
    align: DWRITE_TEXT_ALIGNMENT,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        // SAFETY: plain factory creation.
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let family = if Self::has_family(&dwrite, SEGOE) {
            SEGOE
        } else {
            TAHOMA
        };
        Ok(Self {
            d2d,
            dwrite,
            family,
            scale: 0.0,
            formats: None,
            target: None,
        })
    }

    fn has_family(dwrite: &IDWriteFactory, name: &[u16]) -> bool {
        // SAFETY: `name` is NUL-terminated; out-params are valid locals.
        unsafe {
            let mut coll = None;
            if dwrite.GetSystemFontCollection(&mut coll, false).is_err() {
                return false;
            }
            let Some(coll) = coll else { return false };
            let (mut index, mut exists) = (0u32, BOOL(0));
            coll.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
        }
    }

    fn make_format(&self, spec: FormatSpec) -> Result<IDWriteTextFormat> {
        // SAFETY: family is NUL-terminated.
        unsafe {
            let f = self.dwrite.CreateTextFormat(
                PCWSTR(self.family.as_ptr()),
                None,
                spec.weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                spec.size,
                w!("fa-IR"),
            )?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            // LEADING is the right edge for RTL and the left edge for LTR.
            f.SetReadingDirection(spec.direction)?;
            f.SetTextAlignment(spec.align)?;
            Ok(f)
        }
    }

    pub fn set_scale(&mut self, scale: f32) -> Result<()> {
        if self.formats.is_some() && (self.scale - scale).abs() < f32::EPSILON {
            return Ok(());
        }
        let ltr = DWRITE_READING_DIRECTION_LEFT_TO_RIGHT;
        self.formats = Some(Formats {
            glyph: self.make_format(FormatSpec {
                size: dip::GLYPH_FONT * scale,
                weight: DWRITE_FONT_WEIGHT_NORMAL,
                direction: DWRITE_READING_DIRECTION_RIGHT_TO_LEFT,
                align: DWRITE_TEXT_ALIGNMENT_LEADING,
            })?,
            label: self.make_format(FormatSpec {
                size: dip::LABEL_FONT * scale,
                weight: DWRITE_FONT_WEIGHT_NORMAL,
                direction: ltr,
                align: DWRITE_TEXT_ALIGNMENT_LEADING,
            })?,
            number: self.make_format(FormatSpec {
                size: dip::NUMBER_FONT * scale,
                weight: DWRITE_FONT_WEIGHT_SEMI_LIGHT,
                direction: ltr,
                align: DWRITE_TEXT_ALIGNMENT_CENTER,
            })?,
            footer: self.make_format(FormatSpec {
                size: dip::FOOTER_FONT * scale,
                weight: DWRITE_FONT_WEIGHT_NORMAL,
                direction: ltr,
                align: DWRITE_TEXT_ALIGNMENT_LEADING,
            })?,
        });
        self.scale = scale;
        Ok(())
    }

    /// Widest rendered glyph run and label run in pixels.
    pub fn measure(&self, view: &CandidateView) -> (f32, f32) {
        let Some(f) = &self.formats else {
            return (0.0, 0.0);
        };
        let width = |text: &str, fmt: &IDWriteTextFormat| -> f32 {
            if text.is_empty() {
                return 0.0;
            }
            let w = wide(text);
            // SAFETY: valid slice and format; metrics is a local out-param.
            unsafe {
                let Ok(layout) = self.dwrite.CreateTextLayout(&w, fmt, 4096.0, 256.0) else {
                    return 0.0;
                };
                let mut m = DWRITE_TEXT_METRICS::default();
                if layout.GetMetrics(&mut m).is_ok() {
                    m.widthIncludingTrailingWhitespace
                } else {
                    0.0
                }
            }
        };
        let n = view.visible_len();
        let g = (0..n)
            .map(|i| width(&view.items[i], &f.glyph))
            .fold(0.0, f32::max);
        let l = (0..n)
            .map(|i| width(view.label(i), &f.label))
            .fold(0.0, f32::max);
        (g, l)
    }

    fn ensure_target(&mut self, hwnd: HWND) -> Result<&ID2D1HwndRenderTarget> {
        let mut rc = RECT::default();
        // SAFETY: valid hwnd and out-param.
        unsafe { GetClientRect(hwnd, &mut rc)? };
        let size = D2D_SIZE_U {
            width: (rc.right - rc.left).max(1) as u32,
            height: (rc.bottom - rc.top).max(1) as u32,
        };
        if self.target.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_UNKNOWN,
                    alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            // SAFETY: props are fully initialised.
            self.target = Some(unsafe { self.d2d.CreateHwndRenderTarget(&props, &hprops)? });
        }
        let t = self.target.as_ref().expect("just created");
        // Resize lazily at paint time so no message is sent from inside SetWindowPos.
        // SAFETY: live render target.
        unsafe {
            let cur = t.GetPixelSize();
            if cur.width != size.width || cur.height != size.height {
                t.Resize(&size)?;
            }
        }
        Ok(t)
    }

    pub fn paint(
        &mut self,
        hwnd: HWND,
        layout: &Layout,
        view: &CandidateView,
        highlighted: usize,
        hot: Option<usize>,
        pal: &Palette,
    ) -> Result<()> {
        let Some(formats) = self.formats.clone() else {
            return Ok(());
        };
        let radius = dip::RADIUS * self.scale;
        let rt = self.ensure_target(hwnd)?.clone();
        // SAFETY: draw calls between BeginDraw/EndDraw on a live target.
        let res = unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&d2d_color(pal.background)));
            let brush = |c: Color| -> Result<ID2D1SolidColorBrush> {
                rt.CreateSolidColorBrush(&d2d_color(c), None)
            };
            let b_text = brush(pal.text)?;
            let b_hi_text = brush(pal.highlight_text)?;
            let b_num = brush(pal.number)?;
            let b_label = brush(pal.label)?;
            let b_hi = brush(pal.highlight_bg)?;
            let b_hover = brush(pal.hover_bg)?;
            let b_border = brush(pal.border)?;
            let draw = |text: &str, fmt: &IDWriteTextFormat, r: Rect, b: &ID2D1SolidColorBrush| {
                rt.DrawText(
                    &wide(text),
                    fmt,
                    &rect_f(r),
                    b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            };

            for (i, row) in layout.rows.iter().enumerate() {
                let hi = i == highlighted;
                let shape = rounded(row.row, radius);
                if hi {
                    rt.FillRoundedRectangle(&shape, &b_hi);
                    if let Some(c) = pal.highlight_border {
                        rt.DrawRoundedRectangle(&shape, &brush(c)?, 1.0, None);
                    }
                } else if hot == Some(i) {
                    rt.FillRoundedRectangle(&shape, &b_hover);
                    if let Some(c) = pal.hover_border {
                        rt.DrawRoundedRectangle(&shape, &brush(c)?, 1.0, None);
                    }
                }
                let (tb, nb, lb) = if hi {
                    (&b_hi_text, &b_hi_text, &b_hi_text)
                } else {
                    (&b_text, &b_num, &b_label)
                };
                if row.number.width() > 0 {
                    draw(&crate::view::row_number(i), &formats.number, row.number, nb);
                }
                draw(&view.items[i], &formats.glyph, row.glyph, tb);
                if let Some(lr) = row.label {
                    let l = view.label(i);
                    if !l.is_empty() {
                        draw(l, &formats.label, lr, lb);
                    }
                }
            }

            if let (Some(fr), Some(text)) = (layout.footer, view.footer_text()) {
                let band = D2D_RECT_F {
                    left: 1.0,
                    top: fr.top as f32,
                    right: layout.size.w as f32 - 1.0,
                    bottom: layout.size.h as f32 - 1.0,
                };
                rt.FillRectangle(&band, &brush(pal.footer_bg)?);
                let line = D2D_RECT_F {
                    bottom: band.top + 1.0,
                    ..band
                };
                rt.FillRectangle(&line, &brush(pal.separator)?);
                draw(&text, &formats.footer, fr, &brush(pal.footer_text)?);
            }

            // 1px frame just inside the window edge; CS_DROPSHADOW adds the soft shadow.
            let full = border_rect(layout.size.w, layout.size.h);
            rt.DrawRectangle(&full, &b_border, 1.0, None);
            rt.EndDraw(None, None)
        };
        if let Err(e) = res {
            if e.code() == D2DERR_RECREATE_TARGET {
                self.target = None;
                return Ok(());
            }
            return Err(e);
        }
        Ok(())
    }

    pub fn drop_target(&mut self) {
        self.target = None;
    }
}

fn border_rect(w: i32, h: i32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: 0.5,
        top: 0.5,
        right: w as f32 - 0.5,
        bottom: h as f32 - 0.5,
    }
}
