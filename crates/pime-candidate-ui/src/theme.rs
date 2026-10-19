//! Palettes and the pure parts of theme selection. Registry / system-colour reads
//! live in the Windows module.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: Color,
    pub border: Color,
    pub text: Color,
    pub number: Color,
    pub label: Color,
    pub highlight_bg: Color,
    /// Outline of the highlighted row (`None`: fill only).
    pub highlight_border: Option<Color>,
    pub highlight_text: Color,
    /// Row under the mouse pointer (not the selection).
    pub hover_bg: Color,
    pub hover_border: Option<Color>,
    pub footer_bg: Color,
    pub footer_text: Color,
    pub separator: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
    HighContrast,
}

impl Palette {
    /// Close to Google Japanese IME's list: white card, thin grey frame, pale-blue selection.
    pub const LIGHT: Palette = Palette {
        background: Color::rgb(0xFF, 0xFF, 0xFF),
        border: Color::rgb(0xA8, 0xA8, 0xA8),
        text: Color::rgb(0x1F, 0x1F, 0x1F),
        number: Color::rgb(0x80, 0x80, 0x80),
        label: Color::rgb(0x8A, 0x8A, 0x8A),
        highlight_bg: Color::rgb(0xD5, 0xE6, 0xFB),
        highlight_border: Some(Color::rgb(0x84, 0xAC, 0xE0)),
        highlight_text: Color::rgb(0x00, 0x00, 0x00),
        hover_bg: Color::rgb(0xEF, 0xF4, 0xFB),
        hover_border: None,
        footer_bg: Color::rgb(0xF4, 0xF4, 0xF4),
        footer_text: Color::rgb(0x78, 0x78, 0x78),
        separator: Color::rgb(0xE2, 0xE2, 0xE2),
    };
    pub const DARK: Palette = Palette {
        background: Color::rgb(0x2B, 0x2B, 0x2B),
        border: Color::rgb(0x5A, 0x5A, 0x5A),
        text: Color::rgb(0xEE, 0xEE, 0xEE),
        number: Color::rgb(0x9A, 0x9A, 0x9A),
        label: Color::rgb(0x9A, 0x9A, 0x9A),
        highlight_bg: Color::rgb(0x1F, 0x4E, 0x80),
        highlight_border: Some(Color::rgb(0x3D, 0x7E, 0xC6)),
        highlight_text: Color::rgb(0xFF, 0xFF, 0xFF),
        hover_bg: Color::rgb(0x37, 0x3B, 0x41),
        hover_border: None,
        footer_bg: Color::rgb(0x24, 0x24, 0x24),
        footer_text: Color::rgb(0x9A, 0x9A, 0x9A),
        separator: Color::rgb(0x3C, 0x3C, 0x3C),
    };

    /// High contrast, built from the system colours (window, window text, highlight,
    /// highlight text, grayed text). Hover is an outline only, so the selection stays the
    /// only filled row.
    pub fn high_contrast(
        window: Color,
        window_text: Color,
        highlight: Color,
        highlight_text: Color,
        gray_text: Color,
    ) -> Palette {
        Palette {
            background: window,
            border: window_text,
            text: window_text,
            number: window_text,
            label: gray_text,
            highlight_bg: highlight,
            highlight_border: None,
            highlight_text,
            hover_bg: window,
            hover_border: Some(highlight),
            footer_bg: window,
            footer_text: window_text,
            separator: window_text,
        }
    }
}

/// `AppsUseLightTheme` registry value to theme. A missing value means light (pre-1809 Windows).
pub fn theme_from_apps_use_light(value: Option<u32>) -> Theme {
    match value {
        Some(0) => Theme::Dark,
        _ => Theme::Light,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_value_mapping() {
        assert_eq!(theme_from_apps_use_light(Some(0)), Theme::Dark);
        assert_eq!(theme_from_apps_use_light(Some(1)), Theme::Light);
        assert_eq!(theme_from_apps_use_light(None), Theme::Light);
    }

    #[test]
    fn high_contrast_uses_system_colours() {
        let c = |v| Color::rgb(v, v, v);
        let p = Palette::high_contrast(c(1), c(2), c(3), c(4), c(5));
        assert_eq!(p.background, c(1));
        assert_eq!(p.text, c(2));
        assert_eq!(p.highlight_bg, c(3));
        assert_eq!(p.highlight_text, c(4));
        assert_eq!(p.label, c(5));
        assert_eq!(p.hover_bg, c(1));
        assert_eq!(p.hover_border, Some(c(3)));
    }
}
