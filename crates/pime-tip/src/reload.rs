//! Stream C. "Config changed" signal from the settings app (contract:
//! `crates/pime-settings/README.md`, "Reload signal").
//!
//! A thread-pool wait on the named manual-reset event `Local\PersianPhoneticIme.ConfigChanged`.
//! On wake it only bumps an atomic counter ([`signal_counter`]); it never touches the UI thread
//! and never reads the file. `ConfigHandle::reload_if_changed` (called by the shell on focus
//! change) sees the bumped counter and re-reads the file even if its stamp looks unchanged.
//! The wait is re-armed by a thread-pool timer [`MIN_REARM_MS`] after each wake, because the
//! sender keeps the event signaled for 250 ms.
//!
//! Lifetime (stream A): call [`start`] from `ActivateEx` and [`stop`] from `Deactivate`
//! (reference counted, so one call each per text-service instance). Nothing here runs in
//! `DllMain`. `DllCanUnloadNow` must answer `S_FALSE` while [`is_running`] is true: a pending
//! thread-pool callback into an unloaded DLL would crash the host.
//!
//! AppContainer hosts cannot open the desktop session's `Local\` objects; then [`start`] does
//! nothing and the focus-change check is the only reload path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// Name of the event (same as `pime_settings::reload::CONFIG_CHANGED_EVENT`).
pub const CONFIG_CHANGED_EVENT: &str = r"Local\PersianPhoneticIme.ConfigChanged";
/// Minimum delay before re-arming the wait after a wake (same as the settings crate).
pub const MIN_REARM_MS: u32 = 1000;

static SIGNALS: AtomicU64 = AtomicU64::new(0);

/// Bumped on every wake of the process-wide watcher (and by [`request_recheck`]).
pub fn signal_counter() -> &'static AtomicU64 {
    &SIGNALS
}

/// Makes the next `ConfigHandle::reload_if_changed` re-read the file (menu "Check settings
/// again").
pub fn request_recheck() {
    SIGNALS.fetch_add(1, Ordering::AcqRel);
}

struct Global {
    refs: usize,
    watcher: Option<Watcher>,
}

static GLOBAL: Mutex<Global> = Mutex::new(Global {
    refs: 0,
    watcher: None,
});
static RUNNING: AtomicBool = AtomicBool::new(false);

fn global() -> std::sync::MutexGuard<'static, Global> {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the process-wide watcher on the first call; later calls only count. Never fails.
pub fn start() {
    let _ = std::panic::catch_unwind(|| {
        let mut g = global();
        g.refs += 1;
        if g.refs == 1 {
            g.watcher = Watcher::start(CONFIG_CHANGED_EVENT, &SIGNALS, MIN_REARM_MS);
            RUNNING.store(g.watcher.is_some(), Ordering::SeqCst);
        }
    });
}

/// Undoes one [`start`]; the last call unregisters the wait and waits for running callbacks.
/// Must not be called from inside a thread-pool callback.
pub fn stop() {
    let _ = std::panic::catch_unwind(|| {
        let w = {
            let mut g = global();
            g.refs = g.refs.saturating_sub(1);
            if g.refs > 0 {
                return;
            }
            g.watcher.take()
        };
        drop(w);
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// True while thread-pool callbacks may still run (gate `DllCanUnloadNow` on it).
pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

#[cfg(windows)]
pub use win::Watcher;

#[cfg(not(windows))]
pub struct Watcher;
#[cfg(not(windows))]
impl Watcher {
    pub fn start(_: &str, _: &'static AtomicU64, _: u32) -> Option<Watcher> {
        None
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::Arc;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::Threading::{
        CloseThreadpoolTimer, CloseThreadpoolWait, CreateEventW, CreateThreadpoolTimer,
        CreateThreadpoolWait, SetThreadpoolTimer, SetThreadpoolWait,
        WaitForThreadpoolTimerCallbacks, WaitForThreadpoolWaitCallbacks, PTP_CALLBACK_INSTANCE,
        PTP_TIMER, PTP_WAIT,
    };

    struct Shared {
        event: HANDLE,
        wait: AtomicIsize,
        timer: AtomicIsize,
        stopping: AtomicBool,
        counter: &'static AtomicU64,
        wakes: AtomicU64,
        rearm_ms: u32,
    }

    // SAFETY: an event handle and thread-pool object handles are usable from any thread.
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}

    /// One thread-pool wait on a named event. Dropping it unregisters the wait and blocks until
    /// no callback is running.
    pub struct Watcher {
        shared: Arc<Shared>,
        wait: PTP_WAIT,
        timer: PTP_TIMER,
    }

    // SAFETY: see Shared; the PTP handles are only used by stop(), which is &mut.
    unsafe impl Send for Watcher {}

    /// Relative due time in FILETIME form (negative = relative, 100 ns units).
    fn relative(ms: u32) -> FILETIME {
        let t = -(i64::from(ms) * 10_000);
        FILETIME {
            dwLowDateTime: t as u32,
            dwHighDateTime: (t >> 32) as u32,
        }
    }

    unsafe extern "system" fn on_wait(
        _: PTP_CALLBACK_INSTANCE,
        ctx: *mut c_void,
        _: PTP_WAIT,
        _result: u32,
    ) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: ctx is the Shared kept alive by the Watcher until all callbacks finished.
            let s = unsafe { &*(ctx as *const Shared) };
            if s.stopping.load(Ordering::SeqCst) {
                return;
            }
            s.counter.fetch_add(1, Ordering::AcqRel);
            s.wakes.fetch_add(1, Ordering::AcqRel);
            let due = relative(s.rearm_ms);
            // SAFETY: the timer is valid until stop() closes it after draining callbacks.
            unsafe {
                SetThreadpoolTimer(
                    PTP_TIMER(s.timer.load(Ordering::SeqCst)),
                    Some(&due),
                    0,
                    None,
                )
            };
        }));
    }

    unsafe extern "system" fn on_timer(_: PTP_CALLBACK_INSTANCE, ctx: *mut c_void, _: PTP_TIMER) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: as in on_wait.
            let s = unsafe { &*(ctx as *const Shared) };
            if s.stopping.load(Ordering::SeqCst) {
                return;
            }
            // SAFETY: as in on_wait.
            unsafe {
                SetThreadpoolWait(PTP_WAIT(s.wait.load(Ordering::SeqCst)), Some(s.event), None)
            };
        }));
    }

    impl Watcher {
        /// Opens or creates the manual-reset event `name` and starts waiting. `None` when the
        /// event cannot be opened (AppContainer) or the pool refuses: silently no watcher.
        /// Each wake bumps `counter`; the wait re-arms `rearm_ms` later.
        pub fn start(name: &str, counter: &'static AtomicU64, rearm_ms: u32) -> Option<Watcher> {
            // SAFETY: plain FFI; the name outlives the call. Default security, per the README.
            let event = unsafe { CreateEventW(None, true, false, &HSTRING::from(name)) }.ok()?;
            let shared = Arc::new(Shared {
                event,
                wait: AtomicIsize::new(0),
                timer: AtomicIsize::new(0),
                stopping: AtomicBool::new(false),
                counter,
                wakes: AtomicU64::new(0),
                rearm_ms,
            });
            let ctx = Arc::as_ptr(&shared) as *mut c_void;
            // SAFETY: callbacks get ctx, valid for the Watcher's lifetime (see Drop).
            let wait = match unsafe { CreateThreadpoolWait(Some(on_wait), Some(ctx), None) } {
                Ok(w) => w,
                Err(_) => {
                    unsafe { CloseHandle(event).ok() };
                    return None;
                }
            };
            let timer = match unsafe { CreateThreadpoolTimer(Some(on_timer), Some(ctx), None) } {
                Ok(t) => t,
                Err(_) => {
                    unsafe {
                        CloseThreadpoolWait(wait);
                        CloseHandle(event).ok();
                    }
                    return None;
                }
            };
            shared.wait.store(wait.0, Ordering::SeqCst);
            shared.timer.store(timer.0, Ordering::SeqCst);
            // SAFETY: valid handles created above.
            unsafe { SetThreadpoolWait(wait, Some(event), None) };
            Some(Watcher {
                shared,
                wait,
                timer,
            })
        }

        /// How many times this watcher woke (tests).
        pub fn wakes(&self) -> u64 {
            self.shared.wakes.load(Ordering::SeqCst)
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            self.shared.stopping.store(true, Ordering::SeqCst);
            // Each callback may arm the other object, so cancel and drain twice: after the
            // first round no callback is in flight, and `stopping` stops new arming.
            for _ in 0..2 {
                // SAFETY: handles are valid until closed below.
                unsafe {
                    SetThreadpoolWait(self.wait, None, None);
                    SetThreadpoolTimer(self.timer, None, 0, None);
                    WaitForThreadpoolTimerCallbacks(self.timer, true);
                    WaitForThreadpoolWaitCallbacks(self.wait, true);
                }
            }
            // SAFETY: no callback runs any more; close everything once.
            unsafe {
                CloseThreadpoolTimer(self.timer);
                CloseThreadpoolWait(self.wait);
                let _ = CloseHandle(self.shared.event);
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn test_name(tag: &str) -> String {
        // Never the real name, so tests cannot wake an installed IME or be woken by one.
        format!(
            r"Local\PersianPhoneticIme.TipTest.{tag}.{}",
            std::process::id()
        )
    }

    fn counter() -> &'static AtomicU64 {
        Box::leak(Box::new(AtomicU64::new(0)))
    }

    fn wait_until(limit_ms: u64, f: impl Fn() -> bool) -> bool {
        let end = Instant::now() + Duration::from_millis(limit_ms);
        while Instant::now() < end {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        f()
    }

    #[test]
    fn names_match_settings_contract() {
        assert_eq!(
            CONFIG_CHANGED_EVENT,
            pime_settings::reload::CONFIG_CHANGED_EVENT
        );
        assert_eq!(MIN_REARM_MS, pime_settings::reload::MIN_REARM_MS);
    }

    #[test]
    fn pulse_wakes_once_and_rearms_after_delay() {
        let name = test_name("pulse");
        let c = counter();
        let w = Watcher::start(&name, c, 300).expect("event");
        assert_eq!(w.wakes(), 0);
        // The settings app's own sender, held 100 ms (< re-arm delay): exactly one wake.
        pime_settings::reload::signal_named(&name, 100).unwrap();
        assert!(wait_until(2000, || w.wakes() == 1));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(w.wakes(), 1, "no repeated wake while signaled");
        assert_eq!(c.load(Ordering::SeqCst), 1);
        // Re-armed: a second pulse is seen.
        pime_settings::reload::signal_named(&name, 50).unwrap();
        assert!(wait_until(2000, || w.wakes() == 2));
    }

    #[test]
    fn drop_stops_cleanly_and_later_signals_are_ignored() {
        let name = test_name("drop");
        let c = counter();
        let w = Watcher::start(&name, c, 50).expect("event");
        drop(w);
        pime_settings::reload::signal_named(&name, 50).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(c.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn drop_while_rearm_timer_pending() {
        let name = test_name("pending");
        let c = counter();
        let w = Watcher::start(&name, c, 5000).expect("event");
        pime_settings::reload::signal_named(&name, 20).unwrap();
        assert!(wait_until(2000, || w.wakes() == 1));
        let t = Instant::now();
        drop(w); // cancels the 5 s timer instead of waiting for it
        assert!(t.elapsed() < Duration::from_millis(1000));
    }

    #[test]
    fn global_start_stop_is_reference_counted() {
        // Uses the real event name but only opens it; nothing signals it.
        start();
        start();
        let up = is_running();
        stop();
        assert_eq!(is_running(), up);
        stop();
        assert!(!is_running());
        stop(); // extra stop is harmless
        assert!(!is_running());
    }

    #[test]
    fn recheck_bumps_counter() {
        let before = signal_counter().load(Ordering::SeqCst);
        request_recheck();
        assert!(signal_counter().load(Ordering::SeqCst) > before);
    }
}
