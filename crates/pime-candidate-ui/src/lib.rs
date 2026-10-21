//! Reusable non-activating candidate popup for the Persian phonetic IME.
//!
//! Layout, hit-testing, placement and theme selection are pure and unit-tested; the
//! Win32 / DirectWrite / UI Automation code lives in the `windows`-only `win` module.

pub mod geometry;
pub mod layout;
mod objects;
pub mod placement;
pub mod theme;
mod view;

pub use geometry::{Point, Rect, Size};
pub use objects::live_com_objects;
pub use view::CandidateView;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::{unregister_class, CandidateEvent, CandidateWindow};
