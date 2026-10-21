use crate::layout::MAX_ROWS;

/// What the popup shows. Mirrors `pime-core`'s candidate list (and completion list) without
/// depending on it.
///
/// Rows form one index space: `0..n` are the alternates (`items`), `n..n+k` the completions,
/// drawn below a divider. Both sections together are truncated to `MAX_ROWS` (the 1-9 hot
/// keys), alternates first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CandidateView {
    pub items: Vec<String>,
    pub highlighted: usize,
    pub show_numbers: bool,
    /// Optional small learner hint per alternate row ("sin", "sad"), drawn at the far end of
    /// the row; may be shorter than `items`. Never applies to completion rows.
    pub labels: Option<Vec<String>>,
    /// Draw a footer with the 1-based position of the highlight ("2/3").
    pub show_footer: bool,
    /// Window size in percent (0 means 100).
    pub size_percent: u32,
    /// Word completions: rows after `items`, below a 1 px divider.
    pub completions: Vec<String>,
    /// Highlighted completion (index into `completions`); `None` leaves the selection on the
    /// alternates (or nowhere when there are none).
    pub completion_highlight: Option<usize>,
    /// Leading chars of each completion that were typed (the preedit); drawn in the secondary
    /// text colour, the rest in primary. 0 = no dimming.
    pub completion_prefix_chars: usize,
}

/// Row label for 0-based row `i`: the hot key 1-9 shown with Persian digits (the popup only
/// exists while typing Persian).
pub fn row_number(i: usize) -> String {
    char::from_u32(0x06F1 + i as u32 % 9)
        .map(String::from)
        .unwrap_or_default()
}

impl CandidateView {
    /// Alternate rows actually shown.
    pub fn alt_len(&self) -> usize {
        self.items.len().min(MAX_ROWS)
    }

    /// Completion rows actually shown (whatever room the alternates leave).
    pub fn completion_len(&self) -> usize {
        self.completions.len().min(MAX_ROWS - self.alt_len())
    }

    /// Rows actually shown, both sections (at most `layout::MAX_ROWS`).
    pub fn visible_len(&self) -> usize {
        self.alt_len() + self.completion_len()
    }

    /// Rows above the divider, when both sections are visible (`LayoutInput::divider_after`).
    pub fn divider_after(&self) -> Option<usize> {
        let n = self.alt_len();
        (n > 0 && self.completion_len() > 0).then_some(n)
    }

    /// True if the visible row `i` is a completion row.
    pub fn is_completion(&self, i: usize) -> bool {
        (self.alt_len()..self.visible_len()).contains(&i)
    }

    /// Text of visible row `i` ("" when out of range).
    pub fn row_text(&self, i: usize) -> &str {
        let n = self.alt_len();
        if i < n {
            &self.items[i]
        } else if i < self.visible_len() {
            &self.completions[i - n]
        } else {
            ""
        }
    }

    /// Visible row texts in display order.
    pub fn rows(&self) -> impl Iterator<Item = &str> {
        (0..self.visible_len()).map(move |i| self.row_text(i))
    }

    /// Number of leading UTF-16 units of row `i` to draw dimmed (the typed prefix of a
    /// completion row; 0 for alternates).
    pub fn dim_prefix_utf16(&self, i: usize) -> usize {
        if !self.is_completion(i) || self.completion_prefix_chars == 0 {
            return 0;
        }
        self.row_text(i)
            .chars()
            .take(self.completion_prefix_chars)
            .map(char::len_utf16)
            .sum()
    }

    /// The selected (highlighted) row in the combined index space: the highlighted
    /// completion if any, else the alternates' highlight, else `None` (nothing drawn).
    pub fn selected_row(&self) -> Option<usize> {
        let (n, k) = (self.alt_len(), self.completion_len());
        match self.completion_highlight {
            Some(j) if k > 0 => Some(n + j.min(k - 1)),
            _ if n > 0 => Some(self.highlighted.min(n - 1)),
            _ => None,
        }
    }

    /// Move the selection to combined row `row` (clamped to the visible rows). `None` clears
    /// the completion highlight, leaving the alternates' highlight.
    pub fn select_row(&mut self, row: Option<usize>) {
        let n = self.alt_len();
        match row {
            Some(r) if r >= n && self.completion_len() > 0 => {
                self.completion_highlight = Some((r - n).min(self.completion_len() - 1));
            }
            Some(r) => {
                self.highlighted = r.min(n.saturating_sub(1));
                self.completion_highlight = None;
            }
            None => self.completion_highlight = None,
        }
    }

    /// Selected row, or 0 when nothing is selected.
    pub fn highlight(&self) -> usize {
        self.selected_row().unwrap_or(0)
    }

    /// Multiplier applied on top of the DPI scale.
    pub fn size_factor(&self) -> f32 {
        if self.size_percent == 0 {
            1.0
        } else {
            self.size_percent as f32 / 100.0
        }
    }

    /// Learner label of row `i` (alternate rows only).
    pub fn label(&self, i: usize) -> &str {
        if i >= self.alt_len() {
            return "";
        }
        self.labels
            .as_ref()
            .and_then(|l| l.get(i))
            .map_or("", String::as_str)
    }

    pub fn has_labels(&self) -> bool {
        (0..self.alt_len()).any(|i| !self.label(i).is_empty())
    }

    /// Footer text ("2/3") over the combined rows, `"-/3"` with no selection, or `None` when
    /// the footer is off or the popup is empty.
    pub fn footer_text(&self) -> Option<String> {
        let total = self.items.len() + self.completion_len();
        if !self.show_footer || total == 0 {
            return None;
        }
        Some(match self.selected_row() {
            Some(r) => format!("{}/{}", r + 1, total),
            None => format!("-/{total}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

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
            completions: strs(&["x"]),
            ..Default::default()
        };
        assert_eq!(v.size_factor(), 0.5);
        assert_eq!(CandidateView::default().size_factor(), 1.0);
        assert_eq!(v.visible_len(), 9);
        assert_eq!(v.completion_len(), 0, "no room left for completions");
        assert_eq!(v.highlight(), 8);
        assert_eq!(v.label(0), "a");
        assert_eq!(v.label(3), "");
        assert!(v.has_labels());
        assert_eq!(CandidateView::default().highlight(), 0);
        assert_eq!(CandidateView::default().selected_row(), None);
        assert!(!CandidateView::default().has_labels());
        assert_eq!(v.footer_text().as_deref(), Some("9/12"));
        assert_eq!(CandidateView::default().footer_text(), None);
    }

    fn two_sections() -> CandidateView {
        CandidateView {
            items: strs(&["س", "ص", "ث"]),
            highlighted: 1,
            show_numbers: true,
            labels: Some(strs(&["sin", "sad", "se", "extra"])),
            show_footer: true,
            completions: strs(&["سلام", "سلامت"]),
            completion_prefix_chars: 1,
            ..Default::default()
        }
    }

    #[test]
    fn combined_rows() {
        let v = two_sections();
        assert_eq!(
            (v.alt_len(), v.completion_len(), v.visible_len()),
            (3, 2, 5)
        );
        assert_eq!(v.rows().collect::<Vec<_>>(), ["س", "ص", "ث", "سلام", "سلامت"]);
        assert_eq!(v.row_text(4), "سلامت");
        assert_eq!(v.row_text(5), "");
        assert!(!v.is_completion(2) && v.is_completion(3) && v.is_completion(4));
        assert!(!v.is_completion(5));
        assert_eq!(v.divider_after(), Some(3));
        let only_comps = CandidateView {
            completions: v.completions.clone(),
            ..Default::default()
        };
        assert_eq!(only_comps.divider_after(), None);
        assert!(only_comps.is_completion(0));
        let only_alts = CandidateView {
            items: v.items.clone(),
            ..Default::default()
        };
        assert_eq!(only_alts.divider_after(), None);
        assert_eq!(v.label(2), "se");
        assert_eq!(v.label(3), "", "labels apply to alternates only");
        assert_eq!(row_number(3), "۴", "numbers continue across the divider");
    }

    #[test]
    fn truncation_keeps_alternates_first() {
        let v = CandidateView {
            items: strs(&["a", "b", "c", "d", "e", "f", "g"]),
            completions: strs(&["1", "2", "3", "4"]),
            completion_highlight: Some(3),
            ..Default::default()
        };
        assert_eq!(v.visible_len(), 9);
        assert_eq!(v.completion_len(), 2);
        assert_eq!(
            v.selected_row(),
            Some(8),
            "highlight clamped into the visible rows"
        );
    }

    #[test]
    fn selection_mapping() {
        let mut v = two_sections();
        assert_eq!(v.selected_row(), Some(1));
        assert_eq!(v.footer_text().as_deref(), Some("2/5"));
        v.completion_highlight = Some(1);
        assert_eq!(v.selected_row(), Some(4));
        assert_eq!(v.footer_text().as_deref(), Some("5/5"));
        v.select_row(Some(3));
        assert_eq!(
            (v.completion_highlight, v.selected_row()),
            (Some(0), Some(3))
        );
        v.select_row(Some(2));
        assert_eq!((v.highlighted, v.completion_highlight), (2, None));
        v.select_row(Some(40));
        assert_eq!(v.selected_row(), Some(4));
        v.select_row(None);
        assert_eq!(
            v.selected_row(),
            Some(2),
            "back to the alternates' highlight"
        );

        // Completions only: nothing selected until one is highlighted.
        let mut c = CandidateView {
            completions: strs(&["سلام", "سلامت"]),
            show_footer: true,
            ..Default::default()
        };
        assert_eq!(c.selected_row(), None);
        assert_eq!(c.footer_text().as_deref(), Some("-/2"));
        c.select_row(Some(1));
        assert_eq!(c.selected_row(), Some(1));
        c.select_row(None);
        assert_eq!(c.selected_row(), None);
        c.select_row(Some(0));
        assert_eq!(c.completion_highlight, Some(0));
    }

    #[test]
    fn dimmed_prefix_in_utf16_units() {
        let mut v = two_sections();
        assert_eq!(v.dim_prefix_utf16(0), 0, "alternates are never dimmed");
        assert_eq!(v.dim_prefix_utf16(3), 1);
        v.completion_prefix_chars = 2;
        v.completions = strs(&["می\u{200C}خواهم", "😀ab"]);
        assert_eq!(v.dim_prefix_utf16(3), 2);
        assert_eq!(v.dim_prefix_utf16(4), 3, "surrogate pair counts twice");
        v.completion_prefix_chars = 99;
        assert_eq!(v.dim_prefix_utf16(4), 4, "clamped to the row");
        v.completion_prefix_chars = 0;
        assert_eq!(v.dim_prefix_utf16(3), 0);
    }
}
