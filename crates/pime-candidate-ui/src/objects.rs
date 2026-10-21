//! Live-object count for the COM objects this crate hands out (the UI Automation providers).
//!
//! UIA clients (Narrator, Inspect) may hold a provider long after the popup is gone; a host DLL
//! must keep answering `S_FALSE` from `DllCanUnloadNow` until [`live_com_objects`] is 0, or a
//! late `Release` would jump into unmapped code.

use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);

/// COM objects created by this crate that are still alive (any thread).
pub fn live_com_objects() -> usize {
    LIVE.load(Ordering::SeqCst)
}

/// Embed one in every `#[implement]` struct of this crate.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct ComToken(());

#[cfg_attr(not(windows), allow(dead_code))]
impl ComToken {
    pub(crate) fn new() -> Self {
        LIVE.fetch_add(1, Ordering::SeqCst);
        ComToken(())
    }
}

impl Drop for ComToken {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_count_up_and_down() {
        // Other tests may hold tokens concurrently; only check our own contribution.
        let a = ComToken::new();
        let b = ComToken::new();
        assert!(live_com_objects() >= 2);
        drop(a);
        drop(b);
    }
}
