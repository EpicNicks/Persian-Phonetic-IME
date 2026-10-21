//! Process runtime: background load, key-path completer, debounced cross-process save
//! (docs/autocomplete.md 3.5).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pime_core::{CompleteCtx, Completer, Completion, LearnEvent};

use crate::dict::{read_capped, Dictionary, RankCtx};
use crate::format::DictError;
use crate::history::{History, MAX_FILE as MAX_HISTORY_FILE};
use crate::score::{Day, ScoreParams};

/// Where the runtime reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    /// `dict.bin`.
    pub dict: PathBuf,
    /// `history.bin` (siblings `.tmp`, `.bad` and `history.lock` live next to it).
    pub history: PathBuf,
}

/// Runtime behaviour.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeOptions {
    /// Write `history.bin`. Off: learning lives only in memory for this process.
    pub persist: bool,
    /// Save this long after the last change. Default 10 s.
    pub debounce: Duration,
    /// Save at most this long after the first unsaved change. Default 60 s.
    pub max_delay: Duration,
    /// Scoring constants (clamped).
    pub params: ScoreParams,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        RuntimeOptions {
            persist: true,
            debounce: Duration::from_secs(10),
            max_delay: Duration::from_secs(60),
            params: ScoreParams::default(),
        }
    }
}

/// Dictionary availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The loader thread is still running.
    Loading,
    /// Completions are served.
    Ready {
        /// Dictionary size.
        words: usize,
    },
    /// The dictionary could not be loaded; completion stays off until restart.
    Unavailable(DictError),
}

#[derive(Default)]
struct SaveState {
    first_unsaved: Option<Instant>,
    last_change: Option<Instant>,
    requested: bool,
    clear: bool,
    shutdown: bool,
}

struct Shared {
    paths: RuntimePaths,
    opts: RuntimeOptions,
    clock: fn() -> Day,
    dict: OnceLock<Dictionary>,
    status: Mutex<Status>,
    loaded: Condvar,
    history: Mutex<History>,
    save: Mutex<SaveState>,
    wake: Condvar,
    /// Persistence still allowed (cleared by a panic in a runtime thread).
    persist: AtomicBool,
    /// Bumped by every `record`; a save is clean only if it saw the latest value.
    generation: AtomicU64,
    threads: AtomicUsize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The autocomplete runtime of one process. Dropping the last handle shuts it down.
pub struct Runtime {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("status", &self.status())
            .finish()
    }
}

/// [`Completer`] over the runtime's dictionary and in-memory history.
pub struct LiveCompleter {
    rt: Arc<Runtime>,
}

impl Completer for LiveCompleter {
    fn complete(&self, prefix: &str, limit: usize, ctx: &CompleteCtx) -> Vec<Completion> {
        let s = &self.rt.shared;
        let Some(dict) = s.dict.get() else {
            return Vec::new();
        };
        catch_unwind(AssertUnwindSafe(|| {
            let today = (s.clock)();
            if ctx.use_history {
                let h = lock(&s.history);
                let rank = RankCtx {
                    history: Some(&h),
                    today,
                    params: &s.opts.params,
                };
                dict.complete(prefix, limit, &rank)
            } else {
                let rank = RankCtx {
                    history: None,
                    today,
                    params: &s.opts.params,
                };
                dict.complete(prefix, limit, &rank)
            }
        }))
        .unwrap_or_default()
    }
}

struct ThreadCount(Arc<Shared>);

impl Drop for ThreadCount {
    fn drop(&mut self) {
        self.0.threads.fetch_sub(1, Ordering::SeqCst);
    }
}

fn spawn(shared: &Arc<Shared>, name: &str, body: fn(&Arc<Shared>)) -> bool {
    shared.threads.fetch_add(1, Ordering::SeqCst);
    let guard = ThreadCount(Arc::clone(shared));
    let res = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let s = Arc::clone(&guard.0);
            if catch_unwind(AssertUnwindSafe(|| body(&s))).is_err() {
                s.persist.store(false, Ordering::SeqCst);
            }
            drop(guard);
        });
    // On failure the closure (and with it the guard) has been dropped already.
    res.is_ok()
}

impl Runtime {
    /// Start loading in the background and return at once.
    pub fn start(paths: RuntimePaths, opts: RuntimeOptions) -> Arc<Runtime> {
        Runtime::start_with_clock(paths, opts, Day::today)
    }

    /// [`Runtime::start`] with an injected clock (tests, evaluation).
    pub fn start_with_clock(
        paths: RuntimePaths,
        opts: RuntimeOptions,
        clock: fn() -> Day,
    ) -> Arc<Runtime> {
        let opts = RuntimeOptions {
            params: opts.params.clamped(),
            ..opts
        };
        let shared = Arc::new(Shared {
            paths,
            persist: AtomicBool::new(opts.persist),
            opts,
            clock,
            dict: OnceLock::new(),
            status: Mutex::new(Status::Loading),
            loaded: Condvar::new(),
            history: Mutex::new(History::new()),
            save: Mutex::new(SaveState::default()),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            threads: AtomicUsize::new(0),
        });
        if !spawn(&shared, "pime-dict-load", load_body) {
            set_status(
                &shared,
                Status::Unavailable(DictError::Io(io::ErrorKind::Other)),
            );
        }
        if !spawn(&shared, "pime-dict-save", save_body) {
            shared.persist.store(false, Ordering::SeqCst);
        }
        Arc::new(Runtime { shared })
    }

    /// The key-path completer.
    pub fn completer(self: &Arc<Self>) -> Arc<dyn Completer> {
        Arc::new(LiveCompleter {
            rt: Arc::clone(self),
        })
    }

    /// Apply learn events in memory (key path), then notify the saver.
    pub fn record(&self, events: &[LearnEvent]) {
        if events.is_empty() {
            return;
        }
        let s = &self.shared;
        let today = (s.clock)();
        {
            let mut h = lock(&s.history);
            for ev in events {
                h.record(ev, today, &s.opts.params);
            }
        }
        s.generation.fetch_add(1, Ordering::SeqCst);
        if s.persist.load(Ordering::SeqCst) {
            let now = Instant::now();
            let mut st = lock(&s.save);
            st.first_unsaved.get_or_insert(now);
            st.last_change = Some(now);
            s.wake.notify_one();
        }
    }

    /// Current dictionary status.
    pub fn status(&self) -> Status {
        lock(&self.shared.status).clone()
    }

    /// Block until loading finished or `timeout` passed; returns the status then.
    /// Not for the key path (tests, settings, evaluation).
    pub fn wait_loaded(&self, timeout: Duration) -> Status {
        let s = &self.shared;
        let guard = lock(&s.status);
        let (guard, _) = s
            .loaded
            .wait_timeout_while(guard, timeout, |st| *st == Status::Loading)
            .unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    /// Ask the saver to save now (non-blocking).
    pub fn request_save(&self) {
        self.signal(|st| st.requested = true);
    }

    /// Someone may have cleared the history file; the saver re-checks it (non-blocking).
    pub fn on_clear_signal(&self) {
        self.signal(|st| st.clear = true);
    }

    /// Final save, then the threads exit (non-blocking).
    pub fn shutdown(&self) {
        self.signal(|st| st.shutdown = true);
    }

    /// True while any runtime thread is alive.
    pub fn is_running(&self) -> bool {
        self.shared.threads.load(Ordering::SeqCst) > 0
    }

    fn signal(&self, f: impl FnOnce(&mut SaveState)) {
        let mut st = lock(&self.shared.save);
        f(&mut st);
        self.shared.wake.notify_all();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn set_status(s: &Shared, status: Status) {
    *lock(&s.status) = status;
    s.loaded.notify_all();
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut p: OsString = path.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

fn new_epoch() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mixed = nanos
        ^ (u64::from(std::process::id()) << 40)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    if mixed == 0 {
        1
    } else {
        mixed
    }
}

enum FileState {
    Missing,
    Valid(History),
    Corrupt,
    /// Unreadable for another reason (sharing violation, permissions): leave it alone.
    Unknown,
}

fn read_history(path: &Path) -> FileState {
    match read_capped(path, MAX_HISTORY_FILE) {
        Ok(Some(bytes)) => match History::from_bytes(&bytes) {
            Ok(h) => FileState::Valid(h),
            Err(_) => FileState::Corrupt,
        },
        Ok(None) => FileState::Corrupt,
        Err(e) if e.kind() == io::ErrorKind::NotFound => FileState::Missing,
        Err(_) => FileState::Unknown,
    }
}

/// Keep a damaged file once as `history.bin.bad` (replacing an older one).
fn keep_bad(path: &Path) {
    let _ = fs::rename(path, sibling(path, ".bad"));
}

fn load_body(s: &Arc<Shared>) {
    let dict = Dictionary::load(&s.paths.dict);
    match read_history(&s.paths.history) {
        FileState::Valid(file) => {
            let mut h = lock(&s.history);
            h.merge(&file);
            if h.epoch() == 0 {
                h.set_epoch(file.epoch());
            }
        }
        FileState::Corrupt if s.persist.load(Ordering::SeqCst) => keep_bad(&s.paths.history),
        _ => {}
    }
    let status = match dict {
        Ok(d) => {
            let words = d.len();
            let _ = s.dict.set(d);
            Status::Ready { words }
        }
        Err(e) => Status::Unavailable(e),
    };
    set_status(s, status);
}

enum Work {
    Round,
    Final,
}

fn wait_for_work(s: &Shared) -> Work {
    let mut st = lock(&s.save);
    loop {
        if st.shutdown {
            return Work::Final;
        }
        if st.requested || st.clear {
            st.requested = false;
            st.clear = false;
            return Work::Round;
        }
        let deadline = match (st.first_unsaved, st.last_change) {
            (Some(first), Some(last)) => {
                Some((last + s.opts.debounce).min(first + s.opts.max_delay))
            }
            _ => None,
        };
        st = match deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    return Work::Round;
                }
                s.wake
                    .wait_timeout(st, d - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
            None => s.wake.wait(st).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

fn save_body(s: &Arc<Shared>) {
    loop {
        let work = wait_for_work(s);
        // Let the loader merge the file first, so the clear rule sees its epoch.
        if *lock(&s.status) == Status::Loading {
            let guard = lock(&s.status);
            let _ = s
                .loaded
                .wait_timeout_while(guard, Duration::from_secs(30), |st| *st == Status::Loading);
        }
        if s.persist.load(Ordering::SeqCst) {
            save_round(s);
        } else {
            check_cleared(s);
        }
        if matches!(work, Work::Final) {
            return;
        }
    }
}

/// Mark saved if nothing was recorded since `generation`; otherwise retry later.
fn finish_round(s: &Shared, generation: u64, ok: bool) {
    let mut st = lock(&s.save);
    if ok && s.generation.load(Ordering::SeqCst) == generation {
        st.first_unsaved = None;
        st.last_change = None;
    } else if st.first_unsaved.is_some() || !ok {
        let now = Instant::now();
        st.first_unsaved = Some(now);
        st.last_change = Some(now);
    }
}

/// Persistence off: only apply the clear rule (read-only).
fn check_cleared(s: &Shared) {
    let epoch = lock(&s.history).epoch();
    if epoch == 0 {
        return;
    }
    match read_history(&s.paths.history) {
        FileState::Missing => {
            let mut h = lock(&s.history);
            *h = History::new();
        }
        FileState::Valid(file) if file.epoch() != epoch => *lock(&s.history) = file,
        _ => {}
    }
}

/// Exclusive `history.lock`: 5 tries, 50 ms apart.
fn acquire_lock(path: &Path) -> Option<File> {
    let lock_path = path.with_extension("lock");
    for attempt in 0..5 {
        let mut o = fs::OpenOptions::new();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            o.share_mode(0);
        }
        if let Ok(f) = o.open(&lock_path) {
            return Some(f);
        }
        if attempt < 4 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    None
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = sibling(path, ".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

fn save_round(s: &Shared) {
    let path = &s.paths.history;
    let generation = s.generation.load(Ordering::SeqCst);
    let snapshot = lock(&s.history).clone();
    if snapshot.is_empty() && snapshot.epoch() == 0 && !path.exists() {
        finish_round(s, generation, true);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Some(_lock_file) = acquire_lock(path) else {
        finish_round(s, generation, false);
        return;
    };
    let file = read_history(path);
    let mine = snapshot.epoch();
    let mut result = snapshot;
    let mut file_missing = false;
    match file {
        FileState::Unknown => {
            finish_round(s, generation, false);
            return;
        }
        FileState::Valid(f) if mine == 0 || f.epoch() == mine => {
            result.merge(&f);
            result.set_epoch(if f.epoch() != 0 {
                f.epoch()
            } else {
                new_epoch()
            });
        }
        FileState::Valid(f) => {
            // Cleared, and another process has started a new file since: adopt it.
            *lock(&s.history) = f;
            finish_round(s, generation, true);
            return;
        }
        FileState::Missing if mine != 0 => {
            // Cleared: drop what we learned; the next change starts a new epoch.
            *lock(&s.history) = History::new();
            finish_round(s, generation, true);
            return;
        }
        FileState::Missing => {
            file_missing = true;
            result.set_epoch(new_epoch());
        }
        FileState::Corrupt => {
            keep_bad(path);
            if mine == 0 {
                result.set_epoch(new_epoch());
            }
        }
    }
    result.decay((s.clock)(), &s.opts.params);
    if result.is_empty() && file_missing {
        finish_round(s, generation, true);
        return;
    }
    let ok = write_atomic(path, &result.to_bytes()).is_ok();
    if ok {
        let mut h = lock(&s.history);
        h.merge(&result);
        h.set_epoch(result.epoch());
    }
    finish_round(s, generation, ok);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build_from_tsv, BuildOptions};
    use pime_core::LearnKind;
    use std::io::BufRead;
    use std::sync::atomic::AtomicU32;

    static TODAY: AtomicU32 = AtomicU32::new(20_000);
    fn clock() -> Day {
        Day(TODAY.load(Ordering::SeqCst))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pime-dict-rt-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn setup(name: &str) -> RuntimePaths {
        let dir = temp_dir(name);
        let tsv = "میز\t4.5\nمیخ\t3.5\nمی\u{200C}خواهم\t4.8\n";
        let mut r: &[u8] = tsv.as_bytes();
        let mut inputs: [&mut dyn BufRead; 1] = [&mut r];
        let (bytes, _) = build_from_tsv(&mut inputs, &BuildOptions::default()).unwrap();
        fs::write(dir.join("dict.bin"), bytes).unwrap();
        RuntimePaths {
            dict: dir.join("dict.bin"),
            history: dir.join("sub").join("history.bin"),
        }
    }

    fn opts(persist: bool) -> RuntimeOptions {
        RuntimeOptions {
            persist,
            debounce: Duration::from_millis(30),
            max_delay: Duration::from_millis(200),
            params: ScoreParams::default(),
        }
    }

    fn eventually(what: &str, f: impl Fn() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn top(rt: &Arc<Runtime>, prefix: &str) -> String {
        let c = rt.completer();
        let got = c.complete(prefix, 5, &CompleteCtx { use_history: true });
        got.first().map(|c| c.text.clone()).unwrap_or_default()
    }

    fn accept(word: &str) -> Vec<LearnEvent> {
        vec![LearnEvent {
            word: word.to_string(),
            kind: LearnKind::Accepted,
        }]
    }

    fn file_history(p: &RuntimePaths) -> Option<History> {
        History::from_bytes(&fs::read(&p.history).ok()?).ok()
    }

    #[test]
    fn missing_dictionary_is_silent() {
        let dir = temp_dir("missing");
        let rt = Runtime::start_with_clock(
            RuntimePaths {
                dict: dir.join("nope.bin"),
                history: dir.join("history.bin"),
            },
            opts(true),
            clock,
        );
        assert_eq!(
            rt.wait_loaded(Duration::from_secs(10)),
            Status::Unavailable(DictError::Missing)
        );
        assert!(rt
            .completer()
            .complete("می", 5, &CompleteCtx { use_history: true })
            .is_empty());
        rt.record(&accept("میز"));
        rt.shutdown();
        eventually("threads exit", || !rt.is_running());
    }

    #[test]
    fn learn_save_reload() {
        let p = setup("save");
        let rt = Runtime::start_with_clock(p.clone(), opts(true), clock);
        assert_eq!(
            rt.wait_loaded(Duration::from_secs(10)),
            Status::Ready { words: 3 }
        );
        assert_eq!(top(&rt, "می"), "می\u{200C}خواهم");
        rt.record(&accept("میخ"));
        rt.record(&accept("میخ"));
        assert_eq!(top(&rt, "می"), "میخ");
        let no_hist = rt
            .completer()
            .complete("می", 1, &CompleteCtx { use_history: false });
        assert_eq!(no_hist[0].text, "می\u{200C}خواهم");
        // Debounced save happens without request_save.
        eventually("debounced save", || {
            file_history(&p).is_some_and(|h| h.len() == 1)
        });
        let epoch = file_history(&p).unwrap().epoch();
        assert_ne!(epoch, 0);
        rt.shutdown();
        eventually("threads exit", || !rt.is_running());

        let rt2 = Runtime::start_with_clock(p.clone(), opts(true), clock);
        rt2.wait_loaded(Duration::from_secs(10));
        assert_eq!(top(&rt2, "می"), "میخ");
        rt2.record(&accept("میز"));
        rt2.request_save();
        eventually("second save", || {
            file_history(&p).is_some_and(|h| h.len() == 2)
        });
        assert_eq!(
            file_history(&p).unwrap().epoch(),
            epoch,
            "epoch kept across processes"
        );
        drop(rt2);
    }

    #[test]
    fn clear_rule() {
        let p = setup("clear");
        let rt = Runtime::start_with_clock(p.clone(), opts(true), clock);
        rt.wait_loaded(Duration::from_secs(10));
        rt.record(&accept("میخ"));
        rt.request_save();
        eventually("saved", || file_history(&p).is_some());
        let old = file_history(&p).unwrap().epoch();
        // Another process (settings) clears: delete the file and signal.
        fs::remove_file(&p.history).unwrap();
        rt.on_clear_signal();
        eventually("history dropped", || top(&rt, "می") == "می\u{200C}خواهم");
        assert!(!p.history.exists(), "clearing writes nothing");
        // Learning afterwards starts a new epoch.
        rt.record(&accept("میز"));
        rt.request_save();
        eventually("new file", || file_history(&p).is_some());
        let h = file_history(&p).unwrap();
        assert_ne!(h.epoch(), old);
        assert_eq!(h.len(), 1);
        rt.shutdown();
        eventually("threads exit", || !rt.is_running());
    }

    #[test]
    fn corrupt_history_kept_as_bad() {
        let p = setup("bad");
        fs::create_dir_all(p.history.parent().unwrap()).unwrap();
        fs::write(&p.history, b"PIMEHIST garbage").unwrap();
        let rt = Runtime::start_with_clock(p.clone(), opts(true), clock);
        rt.wait_loaded(Duration::from_secs(10));
        assert!(sibling(&p.history, ".bad").exists());
        assert!(!p.history.exists());
        rt.record(&accept("میخ"));
        rt.request_save();
        eventually("saved", || file_history(&p).is_some());
        rt.shutdown();
        eventually("threads exit", || !rt.is_running());
    }

    #[test]
    fn no_persist_never_writes() {
        let p = setup("nopersist");
        let rt = Runtime::start_with_clock(p.clone(), opts(false), clock);
        rt.wait_loaded(Duration::from_secs(10));
        rt.record(&accept("میخ"));
        assert_eq!(top(&rt, "می"), "میخ");
        rt.request_save();
        rt.shutdown();
        eventually("threads exit", || !rt.is_running());
        assert!(!p.history.exists());
        assert!(!p.history.parent().unwrap().exists());
    }

    #[test]
    fn empty_history_writes_nothing() {
        let p = setup("empty");
        let rt = Runtime::start_with_clock(p.clone(), opts(true), clock);
        rt.wait_loaded(Duration::from_secs(10));
        rt.request_save();
        rt.shutdown();
        eventually("threads exit", || !rt.is_running());
        assert!(!p.history.exists());
    }
}
