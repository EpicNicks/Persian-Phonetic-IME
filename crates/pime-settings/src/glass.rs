//! Software painter for the settings window: flat colour, translucent panels.
//!
//! A [`Canvas`] is an opaque 0x00RRGGBB pixel buffer. Shapes are rounded rectangles
//! evaluated as signed distance fields, which gives anti-aliased edges without any
//! graphics API. There are no gradients and no shadows: a surface is a solid colour at
//! some alpha, with a hairline edge. No Windows types here, so it is unit-tested.

/// 0xRRGGBB.
pub type Rgb = u32;

/// Colours shared by both looks. Saffron marks action (the logo, the main button, focus);
/// Persian turquoise marks state (switches that are on, the selected tab, the result).
pub mod palette {
    use super::Rgb;
    pub const SAFFRON: Rgb = 0xF4A300;
    pub const SAFFRON_DEEP: Rgb = 0x9A5B00;
    /// Text on a saffron fill (white would fail contrast).
    pub const ON_SAFFRON: Rgb = 0x3A2500;
    pub const TURQUOISE: Rgb = 0x19AFA0;
    pub const PROBLEM: Rgb = 0xD9482B;
    pub const DANGER: Rgb = 0xA33A1E;
}

/// Colours and panel strength for one look of the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Panel solidity, 0.2-1.0.
    pub opacity: f32,
    pub base: Rgb,
    pub ink: Rgb,
    pub ink_soft: Rgb,
    pub field: Rgb,
    /// Fill of the output field.
    pub field_lens: Rgb,
    /// Persian turquoise as a fill.
    pub accent2: Rgb,
    /// Persian turquoise as text on this look's surfaces.
    pub accent_text: Rgb,
    /// Colour of hairlines and grooves.
    pub line: Rgb,
}

impl Theme {
    /// `opacity_percent` is clamped to 20-100.
    pub fn new(dark: bool, opacity_percent: i64) -> Theme {
        let opacity = opacity_percent.clamp(20, 100) as f32 / 100.0;
        if dark {
            Theme {
                dark,
                opacity,
                base: 0x14110D,
                ink: 0xF6EDDF,
                ink_soft: 0xBFAE95,
                field: 0x2A2319,
                field_lens: 0x12332F,
                accent2: 0x3CC9BB,
                accent_text: 0x5FD8CB,
                line: 0xFFFFFF,
            }
        } else {
            Theme {
                dark,
                opacity,
                base: 0xF7F2E7,
                ink: 0x2B2118,
                ink_soft: 0x6B5B49,
                field: 0xFFFDF8,
                field_lens: 0xE4F5F2,
                accent2: 0x19AFA0,
                accent_text: 0x0B6F67,
                line: 0x2B2118,
            }
        }
    }

    /// Strength of hairlines relative to the light look (white lines need more).
    pub fn line_alpha(&self, a: f32) -> f32 {
        if self.dark {
            a * 1.4
        } else {
            a
        }
    }

    /// A surface preset for this look and opacity.
    pub fn glass(&self, g: Glass) -> Glass {
        let o = self.opacity;
        if self.dark {
            Glass {
                fill: g.fill * 0.16 * o,
                border: g.border * 0.45,
                tint: 0xFFFFFF,
            }
        } else {
            Glass {
                fill: g.fill * o,
                border: g.border * (0.5 + 0.5 * o),
                tint: 0xFFFFFF,
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

/// A flat translucent surface: `fill` alpha of `tint`, with a hairline of `border` alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glass {
    pub fill: f32,
    pub border: f32,
    pub tint: Rgb,
}

impl Glass {
    pub const PANEL: Glass = Glass {
        fill: 0.6,
        border: 0.85,
        tint: 0xFFFFFF,
    };
    pub const RAISED: Glass = Glass {
        fill: 0.94,
        border: 0.9,
        tint: 0xFFFFFF,
    };
    pub const CONTROL: Glass = Glass {
        fill: 0.62,
        border: 0.8,
        tint: 0xFFFFFF,
    };
    pub const HOVER: Glass = Glass {
        fill: 0.86,
        border: 0.9,
        tint: 0xFFFFFF,
    };
    pub const FAINT: Glass = Glass {
        fill: 0.3,
        border: 0.5,
        tint: 0xFFFFFF,
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

    /// A flat translucent surface with a hairline edge. `border_color` is the colour of
    /// the hairline (the look's line colour).
    pub fn glass(&mut self, r: Rect, radius: f32, g: Glass, border_color: Rgb) {
        self.fill(r, radius, g.tint, g.fill);
        self.stroke(r, radius, 1.0, border_color, g.border * 0.2);
    }

    /// A recessed groove (the tab track, a switch that is off): one flat tint.
    pub fn inset(&mut self, r: Rect, radius: f32, depth: f32, tint: Rgb) {
        self.fill(r, radius, tint, depth);
    }

    /// The saffron surface of the main button: one flat colour.
    pub fn saffron(&mut self, r: Rect, radius: f32) {
        self.fill(r, radius, palette::SAFFRON, 1.0);
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
    fn glass_is_one_flat_translucent_colour() {
        let t = Theme::new(false, 100);
        let mut c = Canvas::new(120, 100, 0x806040);
        let r = Rect::new(20.0, 20.0, 80.0, 50.0);
        c.glass(r, 14.0, t.glass(Glass::PANEL), t.line);
        let top = c.px[35 * 120 + 60];
        let bottom = c.px[55 * 120 + 60];
        assert_eq!(top, bottom, "no gradient inside the panel");
        assert!(
            channels(top)[0] > channels(0x806040)[0],
            "lighter than the backdrop"
        );
        assert!(channels(top)[0] < 255.0, "the backdrop still shows through");
        assert_eq!(c.px[75 * 120 + 60], 0x806040, "no shadow outside");
    }

    #[test]
    fn lower_opacity_lets_more_through() {
        let fill = |pct| Theme::new(false, pct).glass(Glass::PANEL).fill;
        assert!(fill(40) < fill(100));
    }

    #[test]
    fn dark_theme_is_dim_with_light_text() {
        let (l, d) = (Theme::new(false, 100), Theme::new(true, 100));
        assert!(d.glass(Glass::PANEL).fill < l.glass(Glass::PANEL).fill);
        assert!(channels(d.ink)[0] > channels(d.base)[0] + 100.0);
        assert!(channels(l.ink)[0] < channels(l.base)[0] - 100.0);
        assert_eq!(Theme::new(false, 5).opacity, 0.2, "clamped up");
        assert_eq!(Theme::new(false, 500).opacity, 1.0, "clamped down");
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
    fn saffron_is_a_single_colour() {
        let mut c = Canvas::new(100, 60, 0xFFFFFF);
        c.saffron(Rect::new(20.0, 15.0, 60.0, 28.0), 14.0);
        assert_eq!(c.px[20 * 100 + 50], palette::SAFFRON);
        assert_eq!(c.px[38 * 100 + 50], palette::SAFFRON);
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
