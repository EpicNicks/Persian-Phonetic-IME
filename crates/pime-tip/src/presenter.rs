//! Interface between the composition layer (A) and the candidate popup (B). PINNED: change only
//! by editing docs/contracts.md and telling the other stream.
pub use pime_core::CandidateList;

/// Composition/caret rectangle in SCREEN PIXELS, already converted to physical coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Where the popup prefers to appear relative to the composition (it still flips if no room).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferredSide {
    Below,
    Above,
}

/// Implemented by B. All methods are called on the TSF thread that owns the context.
pub trait CandidatePresenter {
    /// Show or refresh the list next to `anchor`. `list.highlighted` is the selected row.
    fn show(&mut self, list: &CandidateList, anchor: ScreenRect, side: PreferredSide);
    /// Move the highlight only (no relayout).
    fn set_highlight(&mut self, index: usize);
    fn hide(&mut self);
    /// Called by the presenter when the user clicks row `index` (0-based); A wires this to
    /// `Session::select_candidate`.
    fn set_on_click(&mut self, f: Box<dyn FnMut(usize)>);
}
