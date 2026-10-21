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
//! Security: a new event gets the explicit descriptor from the settings contract (Everyone and
//! both AppContainer package groups may `SYNCHRONIZE | EVENT_MODIFY_STATE`, low mandatory
//! label), so an elevated host that creates it first does not lock out the medium-integrity
//! settings app. The watcher asks only for `SYNCHRONIZE`. Every failure means "no watcher".
//!
//! AppContainer hosts: `Local\` there names a private per-package object that nobody signals,
//! so [`start`] never creates one. It only tries to open the desktop session's event by its
//! `Session\<id>\` name (works only if the namespace lets the package through; unverified);
//! otherwise the focus-change check is the only reload path.

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
            g.watcher = start_process_watcher();
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

/// True when this process runs in an AppContainer (also false when the token cannot be read).
#[cfg(windows)]
pub fn is_app_container() -> bool {
    win::app_container_session().is_some()
}

#[cfg(not(windows))]
pub fn is_app_container() -> bool {
    false
}

#[cfg(windows)]
fn start_process_watcher() -> Option<Watcher> {
    match win::app_container_session() {
        None => Watcher::start(CONFIG_CHANGED_EVENT, &SIGNALS, MIN_REARM_MS),
        Some(session) => {
            let leaf = CONFIG_CHANGED_EVENT.trim_start_matches(r"Local\");
            let name = format!(r"Session\{session}\{leaf}");
            Watcher::open_existing(&name, &SIGNALS, MIN_REARM_MS)
        }
    }
}

#[cfg(not(windows))]
fn start_process_watcher() -> Option<Watcher> {
    None
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

/// Binary DACL and SACL of the event's descriptor (tests compare them with the settings app's).
#[cfg(windows)]
#[doc(hidden)]
pub fn event_security_acls() -> Option<(Vec<u8>, Vec<u8>)> {
    win::security::EventSecurity::new()
        .ok()
        .map(|s| s.acl_bytes())
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::Arc;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenIsAppContainer, TokenSessionId, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        CloseThreadpoolTimer, CloseThreadpoolWait, CreateEventExW, CreateThreadpoolTimer,
        CreateThreadpoolWait, GetCurrentProcess, OpenEventW, OpenProcessToken, SetThreadpoolTimer,
        SetThreadpoolWait, WaitForThreadpoolTimerCallbacks, WaitForThreadpoolWaitCallbacks,
        CREATE_EVENT_MANUAL_RESET, PTP_CALLBACK_INSTANCE, PTP_TIMER, PTP_WAIT,
        SYNCHRONIZATION_ACCESS_RIGHTS,
    };

    /// The event's explicit security descriptor; must match `pime_settings::reload` (tested).
    pub(crate) mod security {
        use std::ffi::c_void;
        use windows::Win32::Security::{
            AddAccessAllowedAce, AddMandatoryAce, InitializeAcl, InitializeSecurityDescriptor,
            SetSecurityDescriptorDacl, SetSecurityDescriptorSacl, ACE_FLAGS, ACL, ACL_REVISION,
            PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        };

        pub const SYNCHRONIZE: u32 = 0x0010_0000;
        pub const EVENT_MODIFY_STATE: u32 = 0x0002;
        const SYSTEM_MANDATORY_LABEL_NO_WRITE_UP: u32 = 1;

        // Binary SIDs as u32 words: [revision 1 | count << 8, 6-byte big-endian authority
        // (last byte in the top bits of word 1), sub-authorities...].
        const EVERYONE: [u32; 3] = [0x0101, 0x0100_0000, 0];
        const ALL_APP_PACKAGES: [u32; 4] = [0x0201, 0x0F00_0000, 2, 1];
        const ALL_RESTRICTED_APP_PACKAGES: [u32; 4] = [0x0201, 0x0F00_0000, 2, 2];
        const LOW_LABEL: [u32; 3] = [0x0101, 0x1000_0000, 0x1000];
        const GRANTEES: [&[u32]; 3] = [&EVERYONE, &ALL_APP_PACKAGES, &ALL_RESTRICTED_APP_PACKAGES];

        fn sid(s: &[u32]) -> PSID {
            PSID(s.as_ptr() as *mut c_void)
        }

        /// ACL header plus one ACE (header + mask + SID) per SID.
        fn acl_len(sids: &[&[u32]]) -> u32 {
            8 + sids.iter().map(|s| 8 + 4 * s.len() as u32).sum::<u32>()
        }

        #[repr(C, align(8))]
        struct AclBuf([u8; 128]);

        /// An absolute security descriptor and the ACLs it points to (boxed so they never move).
        pub struct EventSecurity {
            sd: Box<SECURITY_DESCRIPTOR>,
            dacl: Box<AclBuf>,
            sacl: Box<AclBuf>,
        }

        impl EventSecurity {
            pub fn new() -> windows::core::Result<EventSecurity> {
                let mut me = EventSecurity {
                    sd: Box::new(SECURITY_DESCRIPTOR::default()),
                    dacl: Box::new(AclBuf([0; 128])),
                    sacl: Box::new(AclBuf([0; 128])),
                };
                let dacl = me.dacl.0.as_mut_ptr() as *mut ACL;
                let sacl = me.sacl.0.as_mut_ptr() as *mut ACL;
                let psd = PSECURITY_DESCRIPTOR(&mut *me.sd as *mut SECURITY_DESCRIPTOR as _);
                // SAFETY: the buffers are large enough (acl_len <= 128), aligned, and live in
                // `me`; the SIDs are valid constants, copied into the ACEs.
                unsafe {
                    InitializeAcl(dacl, acl_len(&GRANTEES), ACL_REVISION)?;
                    for s in GRANTEES {
                        AddAccessAllowedAce(
                            dacl,
                            ACL_REVISION,
                            SYNCHRONIZE | EVENT_MODIFY_STATE,
                            sid(s),
                        )?;
                    }
                    InitializeAcl(sacl, acl_len(&[&LOW_LABEL]), ACL_REVISION)?;
                    AddMandatoryAce(
                        sacl,
                        ACL_REVISION,
                        ACE_FLAGS(0),
                        SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
                        sid(&LOW_LABEL),
                    )?;
                    InitializeSecurityDescriptor(psd, 1)?;
                    SetSecurityDescriptorDacl(psd, true, Some(dacl), false)?;
                    SetSecurityDescriptorSacl(psd, true, Some(sacl), false)?;
                }
                Ok(me)
            }

            /// Attributes pointing at this descriptor; valid while `self` lives.
            pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
                SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: &*self.sd as *const SECURITY_DESCRIPTOR as *mut c_void,
                    bInheritHandle: false.into(),
                }
            }

            /// The DACL and SACL bytes (ACL header's `AclSize` long).
            pub fn acl_bytes(&self) -> (Vec<u8>, Vec<u8>) {
                fn bytes(b: &AclBuf) -> Vec<u8> {
                    let len = u16::from_le_bytes([b.0[2], b.0[3]]) as usize;
                    b.0[..len.min(b.0.len())].to_vec()
                }
                (bytes(&self.dacl), bytes(&self.sacl))
            }
        }
    }

    /// `Some(session id)` when this process runs in an AppContainer, else `None` (also when
    /// the token cannot be queried: then behave like a desktop process).
    pub fn app_container_session() -> Option<u32> {
        // SAFETY: plain FFI on locals; the token handle is closed before returning.
        unsafe {
            let mut tok = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).ok()?;
            let query = |class, out: &mut u32| {
                let mut len = 0u32;
                GetTokenInformation(
                    tok,
                    class,
                    Some(out as *mut u32 as *mut c_void),
                    4,
                    &mut len,
                )
                .is_ok()
            };
            let (mut is_ac, mut session) = (0u32, 0u32);
            let r = (query(TokenIsAppContainer, &mut is_ac) && is_ac != 0)
                && query(TokenSessionId, &mut session);
            let _ = CloseHandle(tok);
            r.then_some(session)
        }
    }

    /// Opens (or, with `create`, creates with the explicit descriptor) event `name` with
    /// `SYNCHRONIZE` only. Creation falls back to plain open, then default security.
    fn open_event(name: &str, create: bool) -> Option<HANDLE> {
        let name = HSTRING::from(name);
        let access = security::SYNCHRONIZE;
        if create {
            if let Ok(sec) = security::EventSecurity::new() {
                let sa = sec.attributes();
                // SAFETY: plain FFI; `sa` and its descriptor outlive the call.
                let r =
                    unsafe { CreateEventExW(Some(&sa), &name, CREATE_EVENT_MANUAL_RESET, access) };
                if let Ok(h) = r {
                    return Some(h);
                }
            }
        }
        // SAFETY: plain FFI; the name outlives the calls.
        let opened = unsafe { OpenEventW(SYNCHRONIZATION_ACCESS_RIGHTS(access), false, &name) };
        if opened.is_ok() || !create {
            return opened.ok();
        }
        // SAFETY: as above.
        unsafe { CreateEventExW(None, &name, CREATE_EVENT_MANUAL_RESET, access) }.ok()
    }

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
            if std::ptr::eq(s.counter, &super::SIGNALS) {
                // The settings app also signals after clearing learned words.
                crate::autocomplete::on_clear_signal();
            }
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
        /// event cannot be opened or the pool refuses: silently no watcher.
        /// Each wake bumps `counter`; the wait re-arms `rearm_ms` later.
        pub fn start(name: &str, counter: &'static AtomicU64, rearm_ms: u32) -> Option<Watcher> {
            Self::with_event(open_event(name, true)?, counter, rearm_ms)
        }

        /// Like [`Watcher::start`] but never creates the event (AppContainer hosts).
        pub fn open_existing(
            name: &str,
            counter: &'static AtomicU64,
            rearm_ms: u32,
        ) -> Option<Watcher> {
            Self::with_event(open_event(name, false)?, counter, rearm_ms)
        }

        fn with_event(
            event: HANDLE,
            counter: &'static AtomicU64,
            rearm_ms: u32,
        ) -> Option<Watcher> {
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
    fn descriptor_matches_settings_contract() {
        let ours = event_security_acls().expect("descriptor builds");
        assert_eq!(Some(ours), pime_settings::reload::event_security_acls());
    }

    #[test]
    fn opens_event_created_first_by_settings() {
        let name = test_name("settings-first");
        let sender = pime_settings::reload::NamedEvent::open_or_create(&name).unwrap();
        let c = counter();
        let w = Watcher::open_existing(&name, c, 300).expect("opens existing");
        sender.pulse(50).unwrap();
        assert!(wait_until(2000, || w.wakes() == 1));
    }

    #[test]
    fn open_existing_never_creates() {
        let c = counter();
        assert!(Watcher::open_existing(&test_name("absent"), c, 300).is_none());
    }

    #[test]
    fn this_test_process_is_not_an_app_container() {
        assert_eq!(win::app_container_session(), None);
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
