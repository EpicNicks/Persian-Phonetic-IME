//! Software painter for the settings window's frosted-glass look.
//!
//! A [`Canvas`] is an opaque 0x00RRGGBB pixel buffer. Shapes are rounded rectangles
//! evaluated as signed distance fields, which gives anti-aliased edges, soft shadows
//! and rim light without any graphics API. The window code (`ui.rs`) paints text on
//! top with GDI and blits the result. No Windows types here, so it is unit-tested.

/// 0xRRGGBB.
pub type Rgb = u32;

/// Colours of the window. Saffron is the only accent; the turquoise and apricot are
/// the light behind the glass, not UI colours.
pub mod palette {
    use super::Rgb;
    pub const SAFFRON: Rgb = 0xF4A300;
    pub const SAFFRON_LIGHT: Rgb = 0xFFC85A;
    pub const SAFFRON_DEEP: Rgb = 0x9A5B00;
    pub const SAFFRON_TEXT_DARK: Rgb = 0xFFC85A;
    /// Text on a saffron fill (white would fail contrast).
    pub const ON_SAFFRON: Rgb = 0x3A2500;
    pub const OK: Rgb = 0x2E9C74;
    pub const PROBLEM: Rgb = 0xD9482B;
    pub const DANGER: Rgb = 0xA33A1E;
    pub const WARM_RIM: Rgb = 0xE8B060;
}

/// Colours and glass strength for one look of the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Glass solidity, 0.2-1.0.
    pub opacity: f32,
    pub base: Rgb,
    pub ink: Rgb,
    pub ink_soft: Rgb,
    pub field: Rgb,
    pub field_lens: Rgb,
    /// Saffron used for text (deep on light glass, bright on dark).
    pub accent_text: Rgb,
    /// Colour of hairlines, field outlines and grooves.
    pub line: Rgb,
    pub shadow: Rgb,
    /// How strongly the backdrop's coloured lights show.
    pub light: f32,
}

impl Theme {
    /// `opacity_percent` is clamped to 20-100.
    pub fn new(dark: bool, opacity_percent: i64) -> Theme {
        let opacity = opacity_percent.clamp(20, 100) as f32 / 100.0;
        if dark {
            Theme {
                dark,
                opacity,
                base: 0x15110C,
                ink: 0xF6EDDF,
                ink_soft: 0xBFAE95,
                field: 0x2A2319,
                field_lens: 0x3D2E12,
                accent_text: palette::SAFFRON_TEXT_DARK,
                line: 0xFFFFFF,
                shadow: 0x000000,
                light: 0.42,
            }
        } else {
            Theme {
                dark,
                opacity,
                base: 0xFBF6EA,
                ink: 0x2B2118,
                ink_soft: 0x6B5B49,
                field: 0xFFFDF8,
                field_lens: 0xFFF3D6,
                accent_text: palette::SAFFRON_DEEP,
                line: 0x2B2118,
                shadow: 0x6B4A1A,
                light: 1.0,
            }
        }
    }

    /// Strength of hairlines relative to the light theme (white lines need less).
    pub fn line_alpha(&self, a: f32) -> f32 {
        if self.dark {
            a * 1.3
        } else {
            a
        }
    }

    /// A glass preset scaled for this theme and opacity.
    pub fn glass(&self, g: Glass) -> Glass {
        let o = self.opacity;
        if self.dark {
            Glass {
                top: g.top * 0.2 * o,
                bottom: g.bottom * 0.2 * o,
                edge: g.edge * 0.9 * o,
                rim_top: g.rim_top * 0.5 * (0.5 + 0.5 * o),
                rim_bottom: g.rim_bottom * 0.45 * (0.5 + 0.5 * o),
                shadow: if g.shadow > 0.0 { 0.45 } else { 0.0 },
                shadow_color: self.shadow,
            }
        } else {
            Glass {
                top: g.top * o,
                bottom: g.bottom * o,
                edge: g.edge * o,
                rim_top: g.rim_top * (0.5 + 0.5 * o),
                rim_bottom: g.rim_bottom * (0.5 + 0.5 * o),
                shadow: g.shadow * o,
                shadow_color: self.shadow,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn inset(self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
}

/// A soft circle of light in the backdrop.
#[derive(Debug, Clone, Copy)]
pub struct Blob {
    pub cx: f32,
    pub cy: f32,
    pub radius: f32,
    pub color: Rgb,
    pub alpha: f32,
}

/// How a glass surface is lit. Alphas are for white light.
#[derive(Debug, Clone, Copy)]
pub struct Glass {
    pub top: f32,
    pub bottom: f32,
    /// Extra brightness in the 7px band along the inside of the edge (glass thickness).
    pub edge: f32,
    pub rim_top: f32,
    pub rim_bottom: f32,
    pub shadow: f32,
    pub shadow_color: Rgb,
}

impl Glass {
    pub const PANEL: Glass = Glass {
        top: 0.34,
        bottom: 0.27,
        edge: 0.04,
        rim_top: 0.7,
        rim_bottom: 0.35,
        shadow: 0.07,
        shadow_color: 0x6B4A1A,
    };
    pub const RAISED: Glass = Glass {
        top: 0.9,
        bottom: 0.82,
        edge: 0.03,
        rim_top: 0.9,
        rim_bottom: 0.5,
        shadow: 0.0,
        shadow_color: 0x6B4A1A,
    };
    pub const CONTROL: Glass = Glass {
        top: 0.5,
        bottom: 0.42,
        edge: 0.03,
        rim_top: 0.75,
        rim_bottom: 0.4,
        shadow: 0.0,
        shadow_color: 0x6B4A1A,
    };
    pub const HOVER: Glass = Glass {
        top: 0.7,
        bottom: 0.6,
        edge: 0.03,
        rim_top: 0.9,
        rim_bottom: 0.5,
        shadow: 0.0,
        shadow_color: 0x6B4A1A,
    };
    pub const FAINT: Glass = Glass {
        top: 0.26,
        bottom: 0.2,
        edge: 0.0,
        rim_top: 0.5,
        rim_bottom: 0.25,
        shadow: 0.0,
        shadow_color: 0x6B4A1A,
    };
}

#[derive(Debug, Clone)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

fn channels(c: Rgb) -> [f32; 3] {
    [
        ((c >> 16) & 0xFF) as f32,
        ((c >> 8) & 0xFF) as f32,
        (c & 0xFF) as f32,
    ]
}

fn mix(dst: u32, src: Rgb, a: f32) -> u32 {
    let a = a.clamp(0.0, 1.0);
    if a <= 0.0 {
        return dst;
    }
    let (d, s) = (channels(dst), channels(src));
    let ch = |i: usize| (d[i] + (s[i] - d[i]) * a).round().clamp(0.0, 255.0) as u32;
    (ch(0) << 16) | (ch(1) << 8) | ch(2)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    mix(a, b, t)
}

/// Signed distance from (px, py) to the rounded rectangle: negative inside.
fn sdf(px: f32, py: f32, r: Rect, radius: f32) -> f32 {
    let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let qx = (px - (r.x + r.w / 2.0)).abs() - (r.w / 2.0 - radius);
    let qy = (py - (r.y + r.h / 2.0)).abs() - (r.h / 2.0 - radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

/// Pixel coverage of a shape at signed distance `d`.
fn cover(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

impl Canvas {
    pub fn new(w: usize, h: usize, fill: Rgb) -> Canvas {
        Canvas {
            w,
            h,
            px: vec![fill; w * h],
        }
    }

    /// A copy of the given region (clamped to the canvas).
    pub fn crop(&self, x: i32, y: i32, w: usize, h: usize) -> Canvas {
        let mut out = Canvas::new(w, h, 0);
        for row in 0..h {
            let sy = y + row as i32;
            if sy < 0 || sy as usize >= self.h {
                continue;
            }
            for col in 0..w {
                let sx = x + col as i32;
                if sx >= 0 && (sx as usize) < self.w {
                    out.px[row * w + col] = self.px[sy as usize * self.w + sx as usize];
                }
            }
        }
        out
    }

    /// The top-left triangle of `light` joined to the bottom-right of `dark` along the
    /// diagonal, for a picture that shows both looks at once.
    pub fn split_diagonal(light: &Canvas, dark: &Canvas) -> Canvas {
        let mut out = light.clone();
        for y in 0..out.h.min(dark.h) {
            for x in 0..out.w.min(dark.w) {
                // Above the line from the top-right to the bottom-left corner: light.
                if x * out.h + y * out.w >= out.w * out.h {
                    out.px[y * out.w + x] = dark.px[y * dark.w + x];
                }
            }
        }
        out
    }

    /// The canvas as a 24-bit BMP file (what Windows Installer bitmap controls load).
    pub fn to_bmp(&self) -> Vec<u8> {
        let row = (self.w * 3 + 3) & !3;
        let size = row * self.h;
        let mut out = Vec::with_capacity(54 + size);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + size) as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(self.w as i32).to_le_bytes());
        out.extend_from_slice(&(self.h as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0; 16]);
        for y in (0..self.h).rev() {
            for x in 0..self.w {
                let c = self.px[y * self.w + x];
                out.extend_from_slice(&[c as u8, (c >> 8) as u8, (c >> 16) as u8]);
            }
            out.resize(out.len() + row - self.w * 3, 0);
        }
        out
    }

    /// Draws straight-alpha RGBA pixels (`sw` x `sh`) scaled into `dest`, averaging
    /// the source over each destination pixel so shrinking stays smooth.
    pub fn blit_rgba(&mut self, src: &[u8], sw: usize, sh: usize, dest: Rect) {
        if src.len() < sw * sh * 4 || dest.w <= 0.0 || dest.h <= 0.0 {
            return;
        }
        let n = ((sw as f32 / dest.w).ceil() as usize).clamp(1, 8);
        self.each(dest, 0.0, |p, x, y| {
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for j in 0..n {
                for i in 0..n {
                    let fx = (x - 0.5 + (i as f32 + 0.5) / n as f32 - dest.x) / dest.w;
                    let fy = (y - 0.5 + (j as f32 + 0.5) / n as f32 - dest.y) / dest.h;
                    if !(0.0..1.0).contains(&fx) || !(0.0..1.0).contains(&fy) {
                        continue;
                    }
                    let k = ((fy * sh as f32) as usize * sw + (fx * sw as f32) as usize) * 4;
                    let al = src[k + 3] as f32 / 255.0;
                    r += src[k] as f32 * al;
                    g += src[k + 1] as f32 * al;
                    b += src[k + 2] as f32 * al;
                    a += al;
                }
            }
            if a > 0.0 {
                let c = (((r / a).round() as u32) << 16)
                    | (((g / a).round() as u32) << 8)
                    | ((b / a).round() as u32);
                *p = mix(*p, c, a / (n * n) as f32);
            }
        });
    }

    /// Replaces the canvas with `base` plus soft circles of light.
    pub fn backdrop(&mut self, base: Rgb, blobs: &[Blob]) {
        for y in 0..self.h {
            for x in 0..self.w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut c = base;
                for b in blobs {
                    let t = ((fx - b.cx).hypot(fy - b.cy) / b.radius).min(1.0);
                    let falloff = 1.0 - t * t * (3.0 - 2.0 * t);
                    c = mix(c, b.color, b.alpha * falloff * falloff);
                }
                self.px[y * self.w + x] = c;
            }
        }
    }

    fn each(&mut self, r: Rect, margin: f32, mut f: impl FnMut(&mut u32, f32, f32)) {
        let x0 = ((r.x - margin).floor().max(0.0)) as usize;
        let y0 = ((r.y - margin).floor().max(0.0)) as usize;
        let x1 = (((r.x + r.w + margin).ceil()).max(0.0) as usize).min(self.w);
        let y1 = (((r.y + r.h + margin).ceil()).max(0.0) as usize).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                f(&mut self.px[y * self.w + x], x as f32 + 0.5, y as f32 + 0.5);
            }
        }
    }

    pub fn fill(&mut self, r: Rect, radius: f32, color: Rgb, alpha: f32) {
        self.each(r, 1.0, |p, x, y| {
            *p = mix(*p, color, alpha * cover(sdf(x, y, r, radius)));
        });
    }

    /// Vertical gradient from `top` to `bottom`.
    pub fn fill_v(&mut self, r: Rect, radius: f32, top: Rgb, bottom: Rgb, alpha: f32) {
        self.each(r, 1.0, |p, x, y| {
            let v = ((y - r.y) / r.h).clamp(0.0, 1.0);
            *p = mix(
                *p,
                lerp_rgb(top, bottom, v),
                alpha * cover(sdf(x, y, r, radius)),
            );
        });
    }

    /// A line of `width` along the inside edge.
    pub fn stroke(&mut self, r: Rect, radius: f32, width: f32, color: Rgb, alpha: f32) {
        self.each(r, 1.0, |p, x, y| {
            let d = sdf(x, y, r, radius);
            let band = cover(d) * (d + width + 0.5).clamp(0.0, 1.0);
            *p = mix(*p, color, alpha * band);
        });
    }

    /// A line of `width` along the outside edge (focus rings).
    pub fn ring(&mut self, r: Rect, radius: f32, width: f32, color: Rgb, alpha: f32) {
        self.each(r, width + 1.0, |p, x, y| {
            let d = sdf(x, y, r, radius);
            let band = (width + 0.5 - d).clamp(0.0, 1.0) * (d + 0.5).clamp(0.0, 1.0);
            *p = mix(*p, color, alpha * band);
        });
    }

    /// A soft shadow under `r`, hidden where the shape itself will be.
    pub fn shadow(&mut self, r: Rect, radius: f32, blur: f32, dy: f32, color: Rgb, alpha: f32) {
        let moved = Rect::new(r.x, r.y + dy, r.w, r.h);
        self.each(moved, blur + 2.0, |p, x, y| {
            let fall = 1.0 - (sdf(x, y, moved, radius) / blur).clamp(0.0, 1.0);
            let outside = (sdf(x, y, r, radius) + 0.5).clamp(0.0, 1.0);
            *p = mix(*p, color, alpha * fall * fall * outside);
        });
    }

    /// An anti-aliased line segment.
    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: Rgb, alpha: f32) {
        let bounds = Rect::new(
            a.0.min(b.0),
            a.1.min(b.1),
            (a.0 - b.0).abs(),
            (a.1 - b.1).abs(),
        );
        self.each(bounds, width + 1.0, |p, x, y| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len2 = (dx * dx + dy * dy).max(1e-6);
            let t = (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0);
            let d = (x - (a.0 + t * dx)).hypot(y - (a.1 + t * dy)) - width / 2.0;
            *p = mix(*p, color, alpha * cover(d));
        });
    }

    /// A frosted-glass surface: translucent white body, brighter along the inside of
    /// the edge, a bright rim that cools toward warm light at the bottom, and a soft
    /// shadow outside.
    pub fn glass(&mut self, r: Rect, radius: f32, g: Glass) {
        if g.shadow > 0.0 {
            self.shadow(r, radius, 14.0, 5.0, g.shadow_color, g.shadow);
        }
        self.each(r, 1.0, |p, x, y| {
            let d = sdf(x, y, r, radius);
            let c = cover(d);
            if c <= 0.0 {
                return;
            }
            let v = ((y - r.y) / r.h).clamp(0.0, 1.0);
            let near = 1.0 - (-d / 7.0).clamp(0.0, 1.0);
            let body = (lerp(g.top, g.bottom, v) + near * near * g.edge).min(1.0);
            *p = mix(*p, 0xFFFFFF, body * c);
            let rim = c * (d + 1.75).clamp(0.0, 1.0);
            let rim_color = lerp_rgb(
                0xFFFFFF,
                palette::WARM_RIM,
                ((v - 0.55) / 0.45).clamp(0.0, 1.0) * 0.25,
            );
            *p = mix(*p, rim_color, lerp(g.rim_top, g.rim_bottom, v) * rim);
        });
    }

    /// A recessed groove (the tab track, an off switch): darker, with a light lower lip.
    pub fn inset(&mut self, r: Rect, radius: f32, depth: f32, tint: Rgb, lip: f32) {
        self.fill(r, radius, tint, depth);
        self.each(r, 1.0, |p, x, y| {
            let d = sdf(x, y, r, radius);
            let c = cover(d);
            let v = ((y - r.y) / r.h).clamp(0.0, 1.0);
            let shade = (1.0 - (y - r.y) / 5.0).clamp(0.0, 1.0);
            *p = mix(*p, tint, depth * 0.8 * shade * c);
            let lip_band = c * (d + 1.5).clamp(0.0, 1.0) * ((v - 0.5) * 2.0).clamp(0.0, 1.0);
            *p = mix(*p, 0xFFFFFF, lip * lip_band);
        });
    }

    /// A saffron surface (primary button, switch that is on, the brand drop).
    pub fn saffron(&mut self, r: Rect, radius: f32, shadow: bool) {
        if shadow {
            self.shadow(r, radius, 10.0, 4.0, palette::SAFFRON_DEEP, 0.4);
        }
        self.fill_v(r, radius, 0xF9B52C, palette::SAFFRON, 1.0);
        let gloss = Rect::new(r.x + 1.5, r.y + 1.5, r.w - 3.0, r.h * 0.5);
        self.each(gloss, 1.0, |p, x, y| {
            let t = ((y - gloss.y) / gloss.h).clamp(0.0, 1.0);
            let inside = cover(sdf(x, y, gloss, radius)) * cover(sdf(x, y, r, radius) + 1.5);
            *p = mix(*p, 0xFFFFFF, 0.14 * (1.0 - t) * inside);
        });
        self.stroke(r, radius, 1.0, 0xFFFFFF, 0.3);
        let low = Rect::new(r.x, r.y + r.h * 0.6, r.w, r.h * 0.4);
        self.each(low, 1.0, |p, x, y| {
            let d = sdf(x, y, r, radius);
            let band = cover(d) * (d + 1.4).clamp(0.0, 1.0);
            *p = mix(*p, palette::SAFFRON_DEEP, 0.18 * band);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_blends_and_clamps() {
        assert_eq!(mix(0x000000, 0xFFFFFF, 1.0), 0xFFFFFF);
        assert_eq!(mix(0x102030, 0xFFFFFF, 0.0), 0x102030);
        assert_eq!(mix(0x000000, 0xFF0000, 0.5), 0x800000);
        assert_eq!(mix(0x000000, 0xFFFFFF, 7.0), 0xFFFFFF);
    }

    #[test]
    fn sdf_is_negative_inside_positive_outside() {
        let r = Rect::new(10.0, 10.0, 40.0, 20.0);
        assert!(sdf(30.0, 20.0, r, 8.0) < -5.0);
        assert!(sdf(80.0, 20.0, r, 8.0) > 20.0);
        // The corner is rounded away: the square's corner is outside the shape.
        assert!(sdf(10.5, 10.5, r, 10.0) > 0.0);
        assert!(sdf(30.0, 10.0, r, 8.0).abs() < 0.01);
    }

    #[test]
    fn fill_covers_inside_only() {
        let mut c = Canvas::new(60, 40, 0x000000);
        c.fill(Rect::new(10.0, 10.0, 40.0, 20.0), 6.0, 0xFFFFFF, 1.0);
        assert_eq!(c.px[20 * 60 + 30], 0xFFFFFF);
        assert_eq!(c.px[2 * 60 + 2], 0x000000);
        assert_eq!(c.px[10 * 60 + 10], 0x000000, "rounded corner stays clear");
    }

    #[test]
    fn glass_keeps_the_shadow_out_of_the_panel_and_lightens_it() {
        let mut c = Canvas::new(120, 100, 0x806040);
        let r = Rect::new(20.0, 20.0, 80.0, 50.0);
        c.glass(r, 14.0, Glass::PANEL);
        let inside = c.px[45 * 120 + 60];
        assert!(
            channels(inside)[0] > channels(0x806040)[0],
            "lighter than the backdrop"
        );
        let below = c.px[78 * 120 + 60];
        assert!(
            channels(below)[0] < channels(0x806040)[0],
            "shadow under the panel"
        );
        assert_eq!(c.px[2 * 120 + 2], 0x806040, "far corner untouched");
    }

    #[test]
    fn backdrop_is_brightest_at_the_blob_centre() {
        let mut c = Canvas::new(80, 80, 0);
        c.backdrop(
            0x000000,
            &[Blob {
                cx: 40.0,
                cy: 40.0,
                radius: 30.0,
                color: 0xFFFFFF,
                alpha: 1.0,
            }],
        );
        assert!(channels(c.px[40 * 80 + 40])[0] > 250.0);
        assert_eq!(c.px[0], 0);
    }

    #[test]
    fn crop_clamps_to_the_canvas() {
        let mut c = Canvas::new(10, 10, 0x111111);
        c.px[0] = 0x222222;
        let part = c.crop(-2, -2, 5, 5);
        assert_eq!(part.px[2 * 5 + 2], 0x222222);
        assert_eq!(part.w * part.h, part.px.len());
    }

    #[test]
    fn saffron_and_inset_stay_inside_their_bounds() {
        let mut c = Canvas::new(100, 60, 0xFFFFFF);
        c.saffron(Rect::new(20.0, 15.0, 60.0, 28.0), 14.0, false);
        let centre = channels(c.px[29 * 100 + 50]);
        assert!(
            centre[0] > 230.0 && centre[2] < 150.0,
            "orange-yellow centre"
        );
        c.inset(Rect::new(0.0, 0.0, 100.0, 10.0), 5.0, 0.1, 0x000000, 0.7);
    }

    #[test]
    fn dark_theme_has_dim_glass_and_light_text() {
        let (l, d) = (Theme::new(false, 100), Theme::new(true, 100));
        assert!(d.glass(Glass::PANEL).top < l.glass(Glass::PANEL).top);
        assert!(channels(d.ink)[0] > channels(d.base)[0] + 100.0);
        assert!(channels(l.ink)[0] < channels(l.base)[0] - 100.0);
        assert_eq!(Theme::new(false, 5).opacity, 0.2, "clamped up");
        assert_eq!(Theme::new(false, 500).opacity, 1.0, "clamped down");
        let half = Theme::new(false, 50).glass(Glass::PANEL);
        assert!(half.top < l.glass(Glass::PANEL).top);
    }

    #[test]
    fn bmp_has_a_valid_header_and_size() {
        let c = Canvas::new(5, 3, 0x102030);
        let b = c.to_bmp();
        assert_eq!(&b[..2], b"BM");
        let row = (5 * 3 + 3) & !3;
        assert_eq!(b.len(), 54 + row * 3);
        assert_eq!(
            u32::from_le_bytes(b[2..6].try_into().unwrap()) as usize,
            b.len()
        );
        assert_eq!(&b[54..57], &[0x30, 0x20, 0x10], "BGR order");
    }

    #[test]
    fn diagonal_split_takes_each_corner_from_its_side() {
        let (a, b) = (Canvas::new(40, 20, 1), Canvas::new(40, 20, 2));
        let c = Canvas::split_diagonal(&a, &b);
        assert_eq!(c.px[0], 1, "top-left is the first");
        assert_eq!(c.px[19 * 40 + 39], 2, "bottom-right is the second");
    }

    #[test]
    fn blit_rgba_blends_by_alpha_and_ignores_clear_pixels() {
        let mut c = Canvas::new(8, 8, 0x000000);
        let mut src = vec![0u8; 2 * 2 * 4];
        for px in src.chunks_mut(4).take(2) {
            px.copy_from_slice(&[255, 255, 255, 255]);
        }
        c.blit_rgba(&src, 2, 2, Rect::new(0.0, 0.0, 8.0, 8.0));
        assert_eq!(c.px[0], 0xFFFFFF, "opaque top half");
        assert_eq!(c.px[7 * 8], 0x000000, "clear bottom half");
    }
}
