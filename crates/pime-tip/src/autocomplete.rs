//! AC-E: word completion wiring (docs/autocomplete.md section 5).
//!
//! One `pime_dict::Runtime` per host process, shared by every text-service instance. It is
//! started by the first activation (or config reload) that finds `[autocomplete] enabled`,
//! never from `DllMain`, and reference counted like `reload::start/stop`: [`acquire`] /
//! [`release`], once each per text service that holds it. The last release asks for a final
//! save and shuts the runtime down; its threads finish on their own, and `DllCanUnloadNow`
//! answers `S_FALSE` while [`is_running`].
//!
//! Key-path rules: `Runtime::record` and the completer only take short in-memory locks. All
//! file work (dictionary load, history save) runs on the runtime's own threads. Nothing here
//! logs or stores words, prefixes or paths; the counters ([`counters`]) carry numbers only.

use std::ffi::OsString;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use pime_core::{ContextPolicy, LearnEvent};
use pime_dict::{DictError, Runtime, RuntimeOptions, RuntimePaths, Status};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::{VARIANT, VT_UNKNOWN};
use windows::Win32::UI::TextServices::{
    ITfContext, ITfInputScope, InputScope, GUID_PROP_INPUTSCOPE, IS_NUMERIC_PASSWORD,
    IS_NUMERIC_PIN, IS_PASSWORD, IS_PRIVATE,
};
use windows_core::{IUnknown, Interface};

use crate::config::ConfigPaths;

/// Development override for the dictionary location (a full file path).
pub const DICT_PATH_ENV: &str = "PIME_DICT_PATH";
/// Dictionary file name, next to the TIP DLLs.
pub const DICT_FILE: &str = "dict.bin";
/// Learned-word history file name (same as `pime_settings::learned::HISTORY_FILE`).
pub const HISTORY_FILE: &str = "history.bin";

/// Input scopes in which nothing is suggested or learned (docs/autocomplete.md 5, Q6).
pub const PRIVATE_SCOPES: [i32; 4] = [
    IS_PASSWORD.0,
    IS_NUMERIC_PASSWORD.0,
    IS_NUMERIC_PIN.0,
    IS_PRIVATE.0,
];

/// No completions, no learning. Also used for a freshly focused context until its input
/// scope has been read, so a password field never sees a completion or records a word.
pub const CLOSED: ContextPolicy = ContextPolicy {
    completions: false,
    learning: false,
};

/// Policy for a context with these input scopes.
pub fn policy_for_scopes(scopes: &[i32]) -> ContextPolicy {
    if scopes.iter().any(|s| PRIVATE_SCOPES.contains(s)) {
        CLOSED
    } else {
        ContextPolicy::default()
    }
}

/// `dict.bin`: `PIME_DICT_PATH` if set (non-empty), else next to `dll` (our module's file).
pub fn dict_path(env: impl Fn(&str) -> Option<OsString>, dll: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = env(DICT_PATH_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    Some(dll?.parent()?.join(DICT_FILE))
}

/// `history.bin`: `PIME_CONFIG_DIR\history.bin`, else `%LOCALAPPDATA%\PersianPhoneticIme\`
/// (same rules as `pime_settings::paths`: it sits next to `config-errors.txt`).
pub fn history_path(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    ConfigPaths::resolve(env).map(|p| p.errors_file.with_file_name(HISTORY_FILE))
}

// -- process-global runtime --------------------------------------------------------------

struct Global {
    refs: usize,
    current: Option<Arc<Runtime>>,
    /// Shut down, threads possibly still finishing their final save.
    retired: Vec<Arc<Runtime>>,
    started: Option<Instant>,
    status_noted: bool,
}

static GLOBAL: Mutex<Global> = Mutex::new(Global {
    refs: 0,
    current: None,
    retired: Vec::new(),
    started: None,
    status_noted: false,
});

fn global() -> MutexGuard<'static, Global> {
    GLOBAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Takes one reference on the process runtime, starting it if needed. `None` only after a
/// panic (then autocomplete simply stays off for this caller; no reference is held).
pub fn acquire() -> Option<Arc<Runtime>> {
    acquire_with(start_runtime)
}

fn acquire_with(start: impl FnOnce() -> Arc<Runtime>) -> Option<Arc<Runtime>> {
    catch_unwind(AssertUnwindSafe(|| {
        let mut g = global();
        let rt = match &g.current {
            Some(rt) => rt.clone(),
            None => {
                let rt = start();
                g.current = Some(rt.clone());
                g.started = Some(Instant::now());
                g.status_noted = false;
                STARTS.fetch_add(1, Ordering::Relaxed);
                rt
            }
        };
        g.refs += 1;
        rt
    }))
    .ok()
}

/// Undoes one successful [`acquire`]. The last one requests a final save and shuts the
/// runtime down (non-blocking).
pub fn release() {
    let _ = catch_unwind(|| {
        let mut g = global();
        g.refs = g.refs.saturating_sub(1);
        if g.refs > 0 {
            return;
        }
        if let Some(rt) = g.current.take() {
            rt.request_save();
            rt.shutdown();
            g.retired.push(rt);
        }
        g.retired.retain(|r| r.is_running());
    });
}

/// True while a runtime exists or a retired one still has a thread alive.
pub fn is_running() -> bool {
    catch_unwind(|| {
        let mut g = global();
        g.retired.retain(|r| r.is_running());
        g.current.is_some() || !g.retired.is_empty()
    })
    // Unknown state: keep the DLL loaded.
    .unwrap_or(true)
}

/// The settings app may have cleared the history (reload watcher thread, off the key path).
pub fn on_clear_signal() {
    let _ = catch_unwind(|| {
        if let Some(rt) = global().current.clone() {
            rt.on_clear_signal();
        }
    });
}

/// Hands one episode's learn events to the runtime (key path: in-memory only).
pub fn record(rt: &Runtime, events: &[LearnEvent]) {
    if events.is_empty() {
        return;
    }
    rt.record(events);
    RECORDED.fetch_add(events.len() as u64, Ordering::Relaxed);
}

fn start_runtime() -> Arc<Runtime> {
    let env = |k: &str| std::env::var_os(k);
    let dll = crate::registry::dll_path().ok().map(PathBuf::from);
    let dict = dict_path(env, dll.as_deref()).unwrap_or_default();
    let history = history_path(env);
    // AppContainer hosts cannot write the user's LocalAppData; learning then lasts for the
    // process only. Creating the folder is a cheap check that we can write there at all.
    let persist = !crate::reload::is_app_container()
        && history
            .as_deref()
            .and_then(Path::parent)
            .is_some_and(|d| std::fs::create_dir_all(d).is_ok());
    Runtime::start(
        RuntimePaths {
            dict,
            history: history.unwrap_or_default(),
        },
        RuntimeOptions {
            persist,
            ..Default::default()
        },
    )
}

// -- counters (numbers only, for ETW) -----------------------------------------------------

static STARTS: AtomicU64 = AtomicU64::new(0);
static RECORDED: AtomicU64 = AtomicU64::new(0);
static LOAD_MS: AtomicU64 = AtomicU64::new(u64::MAX);
static WORDS: AtomicU64 = AtomicU64::new(0);
static UNAVAILABLE: AtomicU8 = AtomicU8::new(0);

/// Process-wide autocomplete counters. Never contains words, prefixes or paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    /// Runtimes started in this process.
    pub starts: u64,
    /// Milliseconds from start to the end of loading (first noticed on a focus change).
    pub load_ms: Option<u64>,
    /// Dictionary size once ready.
    pub words: u64,
    /// Why the dictionary is unavailable (`DictError` kind), if it is.
    pub unavailable: Option<&'static str>,
    /// Learn events handed to the runtime.
    pub events_recorded: u64,
}

pub fn counters() -> Counters {
    Counters {
        starts: STARTS.load(Ordering::Relaxed),
        load_ms: Some(LOAD_MS.load(Ordering::Relaxed)).filter(|v| *v != u64::MAX),
        words: WORDS.load(Ordering::Relaxed),
        unavailable: unavailable_name(UNAVAILABLE.load(Ordering::Relaxed)),
        events_recorded: RECORDED.load(Ordering::Relaxed),
    }
}

/// The error kind as a small code (0 = none).
fn unavailable_code(e: &DictError) -> u8 {
    match e {
        DictError::Missing => 1,
        DictError::Io(_) => 2,
        DictError::TooLarge => 3,
        DictError::BadMagic => 4,
        DictError::UnsupportedVersion(_) => 5,
        DictError::Corrupt(_) => 6,
    }
}

fn unavailable_name(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "missing",
        2 => "io",
        3 => "too-large",
        4 => "bad-magic",
        5 => "unsupported-version",
        6 => "corrupt",
        _ => return None,
    })
}

/// Records load time / size / failure kind once the current runtime finished loading.
/// Call off the key path (focus change).
pub fn note_status() {
    let _ = catch_unwind(|| {
        let mut g = global();
        if g.status_noted {
            return;
        }
        let Some(rt) = g.current.clone() else {
            return;
        };
        let status = rt.status();
        match &status {
            Status::Loading => return,
            Status::Ready { words } => {
                WORDS.store(*words as u64, Ordering::Relaxed);
                UNAVAILABLE.store(0, Ordering::Relaxed);
            }
            Status::Unavailable(e) => UNAVAILABLE.store(unavailable_code(e), Ordering::Relaxed),
        }
        if let Some(t) = g.started {
            LOAD_MS.store(t.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
        g.status_noted = true;
    });
}

// -- input scope --------------------------------------------------------------------------

/// The policy for `ctx` from its input scope at the selection (needs a read edit cookie).
/// No input scope, or one that cannot be read, means an ordinary field.
///
/// # Safety
/// `ec` must be a valid read cookie for `ctx` (call from inside an edit session).
pub(crate) unsafe fn read_policy(ctx: &ITfContext, ec: u32) -> ContextPolicy {
    // SAFETY: forwarded from the caller.
    let scopes = unsafe { read_scopes(ctx, ec) }.unwrap_or_default();
    policy_for_scopes(&scopes)
}

unsafe fn read_scopes(ctx: &ITfContext, ec: u32) -> Option<Vec<i32>> {
    const MAX_SCOPES: usize = 256;
    // SAFETY: plain TSF calls with a valid cookie; the array from GetInputScopes is read
    // within its reported length and freed with CoTaskMemFree as documented.
    unsafe {
        let range = crate::edit::selection(ctx, ec).or_else(|| ctx.GetStart(ec).ok())?;
        let prop = ctx.GetProperty(&GUID_PROP_INPUTSCOPE).ok()?;
        let v = prop.GetValue(ec, &range).ok()?;
        let scope = variant_unknown(&v)?.cast::<ITfInputScope>().ok()?;
        let mut p: *mut InputScope = std::ptr::null_mut();
        let mut n = 0u32;
        let ok = scope.GetInputScopes(&mut p, &mut n).is_ok();
        if p.is_null() {
            return ok.then(Vec::new);
        }
        let len = (n as usize).min(MAX_SCOPES);
        let out = ok.then(|| {
            std::slice::from_raw_parts(p, len)
                .iter()
                .map(|s| s.0)
                .collect()
        });
        CoTaskMemFree(Some(p as *const std::ffi::c_void));
        out
    }
}

fn variant_unknown(v: &VARIANT) -> Option<IUnknown> {
    // SAFETY: the union is read as `punkVal` only when the tag says VT_UNKNOWN.
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        if inner.vt != VT_UNKNOWN {
            return None;
        }
        (*inner.Anonymous.punkVal).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn history_path_agrees_with_pime_settings() {
        let cases: &[&[(&str, &str)]] = &[
            &[
                ("APPDATA", r"C:\U\Roaming"),
                ("LOCALAPPDATA", r"C:\U\Local"),
            ],
            &[("APPDATA", r"C:\U\Roaming")],
            &[
                ("PIME_CONFIG_DIR", r"D:\t"),
                ("APPDATA", r"C:\R"),
                ("LOCALAPPDATA", r"C:\L"),
            ],
            &[("PIME_CONFIG_DIR", ""), ("APPDATA", r"C:\R")],
            &[("LOCALAPPDATA", r"C:\L")],
            &[],
        ];
        for vars in cases {
            let ours = history_path(env_of(vars));
            let theirs =
                pime_settings::paths::Paths::resolve(env_of(vars)).map(|p| p.history_file());
            assert_eq!(ours, theirs, "{vars:?}");
        }
        assert_eq!(HISTORY_FILE, pime_settings::learned::HISTORY_FILE);
        assert_eq!(
            history_path(env_of(&[("LOCALAPPDATA", r"C:\L"), ("APPDATA", r"C:\R")])),
            Some(PathBuf::from(r"C:\L\PersianPhoneticIme\history.bin"))
        );
    }

    #[test]
    fn dict_path_rules() {
        let dll = Path::new(r"C:\Program Files\PIME\x64\pime_tip.dll");
        assert_eq!(
            dict_path(env_of(&[]), Some(dll)),
            Some(PathBuf::from(r"C:\Program Files\PIME\x64\dict.bin"))
        );
        assert_eq!(
            dict_path(env_of(&[(DICT_PATH_ENV, r"D:\dev\d.bin")]), Some(dll)),
            Some(PathBuf::from(r"D:\dev\d.bin"))
        );
        assert_eq!(
            dict_path(env_of(&[(DICT_PATH_ENV, "")]), Some(dll)),
            Some(PathBuf::from(r"C:\Program Files\PIME\x64\dict.bin"))
        );
        assert_eq!(dict_path(env_of(&[]), None), None);
    }

    #[test]
    fn our_dll_path_points_at_a_real_module() {
        // In tests "our module" is the test executable; the rule is the same.
        let dll = crate::registry::dll_path().map(PathBuf::from).unwrap();
        let dict = dict_path(env_of(&[]), Some(&dll)).unwrap();
        assert_eq!(dict.parent(), dll.parent());
    }

    #[test]
    fn private_scopes_close_everything() {
        for s in PRIVATE_SCOPES {
            assert_eq!(policy_for_scopes(&[1, s]), CLOSED, "{s}");
        }
        assert_eq!(policy_for_scopes(&[]), ContextPolicy::default());
        // IS_DEFAULT, IS_URL, IS_EMAIL_SMTPEMAILADDRESS: completions stay on (Q6 open).
        assert_eq!(policy_for_scopes(&[0, 1, 5]), ContextPolicy::default());
        assert_eq!(IS_PASSWORD.0, 31);
    }

    #[test]
    fn variant_without_unknown_is_none() {
        assert!(variant_unknown(&VARIANT::default()).is_none());
        assert!(variant_unknown(&VARIANT::from(5i32)).is_none());
    }

    #[test]
    fn unavailable_codes_round_trip() {
        let all = [
            DictError::Missing,
            DictError::Io(std::io::ErrorKind::Other),
            DictError::TooLarge,
            DictError::BadMagic,
            DictError::UnsupportedVersion(2),
            DictError::Corrupt("x"),
        ];
        let names: Vec<_> = all
            .iter()
            .map(|e| unavailable_name(unavailable_code(e)).unwrap())
            .collect();
        assert_eq!(names.len(), 6);
        assert!(names.iter().all(|n| !n.contains('\\')));
        assert_eq!(unavailable_name(0), None);
    }

    /// The process-global lifecycle, in one test so nothing else interleaves with it. A
    /// temp-folder runtime with no dictionary and no persistence (never the user's AppData).
    #[test]
    fn global_runtime_is_reference_counted() {
        let dir = crate::config::tests::TempDir::new("ac-global");
        let paths = RuntimePaths {
            dict: dir.path().join(DICT_FILE),
            history: dir.path().join(HISTORY_FILE),
        };
        let start = || {
            Runtime::start(
                paths.clone(),
                RuntimeOptions {
                    persist: false,
                    ..Default::default()
                },
            )
        };
        let before = counters().starts;
        let a = acquire_with(start).expect("runtime");
        let b =
            acquire_with(|| -> Arc<Runtime> { unreachable!("already running") }).expect("runtime");
        assert!(Arc::ptr_eq(&a, &b), "one runtime per process");
        assert_eq!(counters().starts, before + 1);
        assert!(is_running());
        on_clear_signal();
        record(&a, &[]);
        release();
        assert!(is_running(), "one reference left");
        release();
        drop((a, b));
        let end = Instant::now() + std::time::Duration::from_secs(10);
        while is_running() && Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!is_running(), "threads exit after the last release");
        release(); // extra release is harmless
        assert!(!is_running());
        // A new acquire starts a fresh runtime.
        let c = acquire_with(start).expect("runtime");
        assert_eq!(counters().starts, before + 2);
        note_status();
        release();
        drop(c);
    }
}
