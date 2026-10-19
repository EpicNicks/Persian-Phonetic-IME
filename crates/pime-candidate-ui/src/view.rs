/// What the popup shows. Mirrors `pime-core`'s candidate list without depending on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CandidateView {
    pub items: Vec<String>,
    pub highlighted: usize,
    pub show_numbers: bool,
    /// Optional small learner hint per row ("sin", "sad"), drawn at the far end of the row;
    /// may be shorter than `items`.
    pub labels: Option<Vec<String>>,
    /// Draw a footer with the 1-based position of the highlight ("2/3").
    pub show_footer: bool,
    /// Window size in percent (0 means 100).
    pub size_percent: u32,
}

/// Row label for 0-based row `i`: the hot key 1-9 shown with Persian digits (the popup only
/// exists while typing Persian).
pub fn row_number(i: usize) -> String {
    char::from_u32(0x06F1 + i as u32 % 9)
        .map(String::from)
        .unwrap_or_default()
}

impl CandidateView {
    /// Items actually shown (at most `layout::MAX_ROWS`, the 1-9 hot keys).
    pub fn visible_len(&self) -> usize {
        self.items.len().min(crate::layout::MAX_ROWS)
    }

    /// Highlight clamped into the visible range (0 when empty).
    pub fn highlight(&self) -> usize {
        self.highlighted.min(self.visible_len().saturating_sub(1))
    }

    /// Multiplier applied on top of the DPI scale.
    pub fn size_factor(&self) -> f32 {
        if self.size_percent == 0 {
            1.0
        } else {
            self.size_percent as f32 / 100.0
        }
    }

    pub fn label(&self, i: usize) -> &str {
        self.labels
            .as_ref()
            .and_then(|l| l.get(i))
            .map_or("", String::as_str)
    }

    pub fn has_labels(&self) -> bool {
        (0..self.visible_len()).any(|i| !self.label(i).is_empty())
    }

    /// Footer text ("2/3"), or `None` when the footer is off or the list is empty.
    pub fn footer_text(&self) -> Option<String> {
        (self.show_footer && !self.items.is_empty())
            .then(|| format!("{}/{}", self.highlight() + 1, self.items.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_numbered_with_persian_digits() {
        assert_eq!(row_number(0), "۱");
        assert_eq!(row_number(8), "۹");
    }

    #[test]
    fn clamps_and_labels() {
        let v = CandidateView {
            items: (0..12).map(|i| i.to_string()).collect(),
            highlighted: 40,
            show_numbers: true,
            labels: Some(vec!["a".into()]),
            show_footer: true,
            size_percent: 50,
        };
        assert_eq!(v.size_factor(), 0.5);
        assert_eq!(CandidateView::default().size_factor(), 1.0);
        assert_eq!(v.visible_len(), 9);
        assert_eq!(v.highlight(), 8);
        assert_eq!(v.label(0), "a");
        assert_eq!(v.label(3), "");
        assert!(v.has_labels());
        assert_eq!(CandidateView::default().highlight(), 0);
        assert!(!CandidateView::default().has_labels());
        assert_eq!(v.footer_text().as_deref(), Some("9/12"));
        assert_eq!(CandidateView::default().footer_text(), None);
    }
}
