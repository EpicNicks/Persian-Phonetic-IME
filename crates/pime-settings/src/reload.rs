//! "Config changed" signal from the settings app to every running IME instance.
//!
//! # Contract (the TIP side implements the receiving half)
//!
//! - **Object**: a named, manual-reset Win32 event [`CONFIG_CHANGED_EVENT`]
//!   (`Local\PersianPhoneticIme.ConfigChanged`, the per-session namespace), default
//!   security. Whichever side comes first creates it (`CreateEventW(NULL, TRUE,
//!   FALSE, name)`, which opens it if it already exists); nobody owns it.
//! - **Sender** (this crate, after every save or reset): `SetEvent`, keep it signaled
//!   for [`PULSE_MS`], then `ResetEvent`. Holding it for a while (instead of
//!   `PulseEvent`, which is documented as unreliable) lets every waiting process see
//!   it. Hand edits in a text editor do not signal; see the last point.
//! - **Receiver** (TIP, once per process, never on the key path): wait on the
//!   event with a thread-pool wait (`RegisterWaitForSingleObject` with
//!   `WT_EXECUTEONLYONCE`, or `CreateThreadpoolWait`). On wake, re-read
//!   `config.toml` off the UI thread, swap the `Arc<ResolvedConfig>` and rewrite
//!   `config-errors.txt`; treat the wake as a hint ("check whether the file
//!   changed"), so a spurious or repeated wake is harmless. Re-arm the wait no
//!   sooner than [`MIN_REARM_MS`] after the wake, because the event may still be
//!   signaled; a second save inside that window is caught by the focus-change check.
//! - **Not delivered everywhere**: AppContainer (immersive/UWP) processes have their
//!   own object namespace and never see this event. The TIP must also re-check the
//!   file on focus change (`ITfThreadMgrEventSink::OnSetFocus`, comparing the file's
//!   last-write time and size), which also catches hand edits.

use std::io;

/// Name of the event (pass to `CreateEventW`).
pub const CONFIG_CHANGED_EVENT: &str = r"Local\PersianPhoneticIme.ConfigChanged";
/// How long the sender keeps the event signaled, in milliseconds.
pub const PULSE_MS: u32 = 250;
/// Minimum delay before a receiver re-arms its wait after a wake, in milliseconds.
pub const MIN_REARM_MS: u32 = 1000;

/// Tells running IME instances that `config.toml` changed. Blocks for [`PULSE_MS`].
pub fn signal_config_changed() -> io::Result<()> {
    signal_named(CONFIG_CHANGED_EVENT, PULSE_MS)
}

#[cfg(windows)]
pub use win::{signal_named, NamedEvent};

#[cfg(windows)]
mod win {
    use std::io;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateEventW, ResetEvent, SetEvent, WaitForSingleObject,
    };

    fn io_err(e: windows::core::Error) -> io::Error {
        io::Error::other(e.message())
    }

    /// An open handle to a named manual-reset event (the protocol in the module docs).
    #[derive(Debug)]
    pub struct NamedEvent(HANDLE);

    // SAFETY: event handles may be used from any thread.
    unsafe impl Send for NamedEvent {}
    unsafe impl Sync for NamedEvent {}

    impl NamedEvent {
        /// Creates the event, or opens it if another process already did.
        pub fn open_or_create(name: &str) -> io::Result<NamedEvent> {
            // SAFETY: plain FFI call; the name outlives it.
            let h =
                unsafe { CreateEventW(None, true, false, &HSTRING::from(name)) }.map_err(io_err)?;
            Ok(NamedEvent(h))
        }

        /// Sets the event.
        pub fn set(&self) -> io::Result<()> {
            // SAFETY: self.0 is a valid event handle until drop.
            unsafe { SetEvent(self.0) }.map_err(io_err)
        }

        /// Resets the event.
        pub fn reset(&self) -> io::Result<()> {
            // SAFETY: as above.
            unsafe { ResetEvent(self.0) }.map_err(io_err)
        }

        /// Waits up to `timeout_ms`; true if the event was signaled.
        pub fn wait(&self, timeout_ms: u32) -> bool {
            // SAFETY: as above.
            unsafe { WaitForSingleObject(self.0, timeout_ms) == WAIT_OBJECT_0 }
        }

        /// Set, hold for `hold_ms`, reset.
        pub fn pulse(&self, hold_ms: u32) -> io::Result<()> {
            self.set()?;
            std::thread::sleep(std::time::Duration::from_millis(hold_ms.into()));
            self.reset()
        }
    }

    impl Drop for NamedEvent {
        fn drop(&mut self) {
            // SAFETY: we own the handle.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Opens or creates event `name` and pulses it for `hold_ms`.
    pub fn signal_named(name: &str, hold_ms: u32) -> io::Result<()> {
        NamedEvent::open_or_create(name)?.pulse(hold_ms)
    }
}

/// No IME runs on other platforms; signalling is a no-op.
#[cfg(not(windows))]
pub fn signal_named(_name: &str, _hold_ms: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn test_name(tag: &str) -> String {
        // Never the real event name, so tests cannot wake an installed IME.
        format!(
            r"Local\PersianPhoneticIme.Test.{tag}.{}",
            std::process::id()
        )
    }

    #[test]
    fn waiter_in_other_handle_sees_pulse() {
        let name = test_name("pulse");
        let receiver = NamedEvent::open_or_create(&name).unwrap();
        assert!(!receiver.wait(0), "starts non-signaled");
        let waiter = std::thread::spawn(move || receiver.wait(5000));
        std::thread::sleep(std::time::Duration::from_millis(50));
        signal_named(&name, 100).unwrap();
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn event_is_reset_after_pulse() {
        let name = test_name("reset");
        let receiver = NamedEvent::open_or_create(&name).unwrap();
        signal_named(&name, 10).unwrap();
        assert!(!receiver.wait(0));
    }

    #[test]
    fn real_name_is_session_local() {
        assert!(CONFIG_CHANGED_EVENT.starts_with(r"Local\"));
    }
}
