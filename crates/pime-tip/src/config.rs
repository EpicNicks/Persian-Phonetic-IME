//! Stream C. Config handle used by the text service. PINNED interface:
//!
//! ```ignore
//! pub struct ConfigHandle { .. }
//! impl ConfigHandle {
//!     pub fn load() -> Self;                                   // reads %APPDATA% config (PIME_CONFIG_DIR override), never fails
//!     pub fn current(&self) -> std::sync::Arc<pime_core::ResolvedConfig>;
//!     pub fn issues(&self) -> Vec<pime_core::ConfigIssue>;     // for the language-bar badge
//!     /// Cheap check (mtime/size) for focus changes; reloads off the key path when changed.
//!     pub fn reload_if_changed(&self) -> bool;
//! }
//! ```
//!
//! Additive: [`ConfigHandle::badge_text`] (text for `LangBar::set_error_badge`),
//! [`ConfigHandle::with_paths`] and [`ConfigHandle::paths`].
//!
//! File I/O happens only in `load` and `reload_if_changed` (activation and focus change),
//! never in `current`/`issues`, which are safe on the key path (one short lock + `Arc` clone).
//! The contract (paths, decoding, error file) is `crates/pime-settings/README.md`.

use pime_core::{
    fallback_of, format_issues_for, load_config_bytes, ConfigIssue, Fallback, ResolvedConfig,
};
use std::ffi::OsString;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

/// Environment variable that puts every file directly in one folder (tests, portable use).
pub const CONFIG_DIR_ENV: &str = "PIME_CONFIG_DIR";
/// Folder name under `%APPDATA%` and `%LOCALAPPDATA%`.
pub const APP_DIR: &str = "PersianPhoneticIme";
/// User config file name.
pub const CONFIG_FILE: &str = "config.toml";
/// Error report file name.
pub const ERRORS_FILE: &str = "config-errors.txt";

/// Where the TIP reads its config and writes the error report. A minimal copy of
/// `pime_settings::paths::Paths` (the settings crate is too heavy to link into the DLL); a
/// test checks the two agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    /// `%APPDATA%\PersianPhoneticIme\config.toml`.
    pub config_file: PathBuf,
    /// `%LOCALAPPDATA%\PersianPhoneticIme\config-errors.txt`.
    pub errors_file: PathBuf,
}

impl ConfigPaths {
    /// From the process environment. `None` when neither `PIME_CONFIG_DIR` nor `APPDATA` is set.
    pub fn from_env() -> Option<Self> {
        Self::resolve(|k| std::env::var_os(k))
    }

    /// Same rules as `pime_settings::paths::Paths::resolve`.
    pub fn resolve(env: impl Fn(&str) -> Option<OsString>) -> Option<Self> {
        let non_empty = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(dir) = non_empty(CONFIG_DIR_ENV) {
            return Some(Self::in_dir(&dir));
        }
        let roaming = non_empty("APPDATA")?.join(APP_DIR);
        let local = non_empty("LOCALAPPDATA")
            .map(|p| p.join(APP_DIR))
            .unwrap_or_else(|| roaming.clone());
        Some(Self {
            config_file: roaming.join(CONFIG_FILE),
            errors_file: local.join(ERRORS_FILE),
        })
    }

    /// Both files directly inside `dir`.
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            config_file: dir.join(CONFIG_FILE),
            errors_file: dir.join(ERRORS_FILE),
        }
    }
}

/// What a cheap `metadata` call saw. Compared on focus change to decide whether to re-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stamp {
    Missing,
    Present {
        modified: Option<SystemTime>,
        len: u64,
    },
    /// Could not stat (e.g. access denied in an AppContainer host).
    Unknown,
}

fn stamp_of(path: &Path) -> Stamp {
    match std::fs::metadata(path) {
        Ok(m) => Stamp::Present {
            modified: m.modified().ok(),
            len: m.len(),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Stamp::Missing,
        Err(_) => Stamp::Unknown,
    }
}

/// Result of reading the file once.
enum Read {
    /// File absent: built-in defaults, no issues.
    Missing,
    Loaded(ResolvedConfig, Vec<ConfigIssue>),
    /// Could not read it (permissions, sharing violation): keep what we have.
    Failed,
}

fn read_config(path: &Path) -> Read {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Read::Missing,
        Err(_) => return Read::Failed,
    };
    // pime-core is not expected to panic, but a bad file must never take the host down.
    match catch_unwind(AssertUnwindSafe(|| load_config_bytes(&bytes))) {
        Ok(l) => Read::Loaded(l.config, l.issues),
        Err(_) => Read::Failed,
    }
}

/// The file name used in messages (`config.toml` unless overridden).
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| CONFIG_FILE.to_string())
}

/// Content of `config-errors.txt` (without the BOM), or `None` when the file should be removed.
/// Byte-for-byte the same as `pime_settings::report::errors_file_text` (tested).
pub fn errors_file_text(config_path: &Path, issues: &[ConfigIssue]) -> Option<String> {
    if issues.is_empty() {
        return None;
    }
    Some(format!(
        "Persian Phonetic IME: problems in your settings file\r\nFile: {}\r\n\r\n{}",
        config_path.display(),
        format_issues_for(&file_label(config_path), issues).replace('\n', "\r\n")
    ))
}

/// Rewrites or removes the error report. Skips the write when the file already holds the same
/// bytes, so many host processes activating at once do not churn it. Errors are returned for
/// tests; callers ignore them (AppContainer hosts cannot write there).
fn write_errors_file(
    errors_path: &Path,
    config_path: &Path,
    issues: &[ConfigIssue],
) -> io::Result<()> {
    let Some(text) = errors_file_text(config_path, issues) else {
        return match std::fs::remove_file(errors_path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    };
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend_from_slice(text.as_bytes());
    if std::fs::read(errors_path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(dir) = errors_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Unique temp name per process and write: several hosts may write at the same moment.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = errors_path.with_extension(format!(
        "txt.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, errors_path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Longest message (in characters) shown in the language-bar tooltip.
const BADGE_DETAIL_CHARS: usize = 120;

/// Friendly language-bar text for a set of issues; `None` when there is nothing to report.
/// Never contains typed text (issues only quote the config file).
pub fn badge_text_for(file_name: &str, issues: &[ConfigIssue]) -> Option<String> {
    let first = issues.first()?;
    let lead = match fallback_of(issues) {
        Fallback::WholeFile => {
            "Your settings file couldn't be read, so the default settings are in use."
        }
        Fallback::Entry => "Your settings file has a problem; that setting was skipped.",
        Fallback::None => "Your settings file has a warning.",
    };
    let at = if first.line > 0 {
        format!("{file_name}, line {}", first.line)
    } else {
        file_name.to_string()
    };
    let more = match issues.len() {
        1 => String::new(),
        n => format!(" (+{} more)", n - 1),
    };
    let detail = crate::langbar::truncate_chars(&first.message, BADGE_DETAIL_CHARS);
    Some(format!(
        "{lead} {at}: {detail}{more} Right-click for details."
    ))
}

struct State {
    cfg: Arc<ResolvedConfig>,
    issues: Arc<Vec<ConfigIssue>>,
    stamp: Stamp,
    seen_signal: u64,
}

/// The text service's view of the user config. Cheap to read from any thread.
pub struct ConfigHandle {
    paths: Option<ConfigPaths>,
    signal: &'static AtomicU64,
    state: Mutex<State>,
    /// Serialises reloads so two focus changes never interleave file I/O.
    io: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ConfigHandle {
    /// Reads the user config (`PIME_CONFIG_DIR` override, else `%APPDATA%`). Never fails: a
    /// missing or broken file gives the built-in defaults. Also rewrites `config-errors.txt`.
    pub fn load() -> Self {
        Self::with_source(ConfigPaths::from_env(), crate::reload::signal_counter())
    }

    /// Like [`load`](Self::load) with explicit locations (`None` = defaults only, no files).
    pub fn with_paths(paths: Option<ConfigPaths>) -> Self {
        Self::with_source(paths, crate::reload::signal_counter())
    }

    pub(crate) fn with_source(paths: Option<ConfigPaths>, signal: &'static AtomicU64) -> Self {
        let h = Self {
            paths,
            signal,
            state: Mutex::new(State {
                cfg: Arc::new(ResolvedConfig::default()),
                issues: Arc::new(Vec::new()),
                stamp: Stamp::Unknown,
                seen_signal: signal.load(Ordering::Acquire),
            }),
            io: Mutex::new(()),
        };
        h.reload(true);
        h
    }

    /// The config in force. Key-path safe.
    pub fn current(&self) -> Arc<ResolvedConfig> {
        lock(&self.state).cfg.clone()
    }

    /// Problems found in the last load (warnings included), in file order.
    pub fn issues(&self) -> Vec<ConfigIssue> {
        lock(&self.state).issues.as_ref().clone()
    }

    /// Text for `LangBar::set_error_badge`, `None` when the file is clean.
    pub fn badge_text(&self) -> Option<String> {
        let label = self
            .paths
            .as_ref()
            .map_or_else(|| CONFIG_FILE.to_string(), |p| file_label(&p.config_file));
        badge_text_for(&label, &lock(&self.state).issues)
    }

    /// The resolved file locations (`None` when no AppData folder is known).
    pub fn paths(&self) -> Option<&ConfigPaths> {
        self.paths.as_ref()
    }

    /// Call on focus change (and after `reload::start`'s signal). Stats the file and re-reads it
    /// only when its last-write time or size changed, or when the settings app signalled a save
    /// since the last check. Returns true when `current()`/`issues()` now return something
    /// different, so the caller should push the new config to its session and refresh the badge.
    pub fn reload_if_changed(&self) -> bool {
        catch_unwind(AssertUnwindSafe(|| self.reload(false))).unwrap_or(false)
    }

    fn reload(&self, initial: bool) -> bool {
        let Some(paths) = self.paths.as_ref() else {
            return false;
        };
        let _io = lock(&self.io);
        // Read the signal before the stat so a save landing in between is seen next time.
        let signal = self.signal.load(Ordering::Acquire);
        let stamp = stamp_of(&paths.config_file);
        {
            let st = lock(&self.state);
            if !initial && signal == st.seen_signal && stamp == st.stamp {
                return false;
            }
        }
        let (cfg, issues) = match read_config(&paths.config_file) {
            Read::Missing => (ResolvedConfig::default(), Vec::new()),
            Read::Loaded(c, i) => (c, i),
            Read::Failed => {
                // Keep the config in force; try again when the stamp changes.
                let mut st = lock(&self.state);
                st.stamp = stamp;
                st.seen_signal = signal;
                return false;
            }
        };
        let changed = {
            let mut st = lock(&self.state);
            st.stamp = stamp;
            st.seen_signal = signal;
            let changed = *st.cfg != cfg || *st.issues != issues;
            if changed {
                st.cfg = Arc::new(cfg);
                st.issues = Arc::new(issues.clone());
            }
            changed
        };
        if changed || initial {
            let _ = write_errors_file(&paths.errors_file, &paths.config_file, &issues);
        }
        changed
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// A fresh temp folder, removed on drop. Never under %APPDATA%.
    pub(crate) struct TempDir(PathBuf);
    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "pime-tip-test-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn private_signal() -> &'static AtomicU64 {
        Box::leak(Box::new(AtomicU64::new(0)))
    }

    fn handle(dir: &TempDir) -> (ConfigHandle, &'static AtomicU64) {
        let sig = private_signal();
        (
            ConfigHandle::with_source(Some(ConfigPaths::in_dir(dir.path())), sig),
            sig,
        )
    }

    fn env_of<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn paths_agree_with_pime_settings() {
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
            let ours = ConfigPaths::resolve(env_of(vars));
            let theirs = pime_settings::paths::Paths::resolve(env_of(vars));
            assert_eq!(
                ours.as_ref().map(|p| (&p.config_file, &p.errors_file)),
                theirs.as_ref().map(|p| (&p.config_file, &p.errors_file)),
                "{vars:?}"
            );
        }
        assert_eq!(CONFIG_DIR_ENV, pime_settings::paths::CONFIG_DIR_ENV);
        assert_eq!(APP_DIR, pime_settings::paths::APP_DIR);
        assert_eq!(CONFIG_FILE, pime_settings::paths::CONFIG_FILE);
        assert_eq!(ERRORS_FILE, pime_settings::paths::ERRORS_FILE);
    }

    #[test]
    fn errors_text_agrees_with_pime_settings() {
        let cfg = Path::new(r"C:\x\config.toml");
        for toml in [
            "",
            "[options]\ndiigts = \"ascii\"\n",
            "[options]\ngh = \"ghain\n",
        ] {
            let issues = pime_core::load_config(toml).issues;
            assert_eq!(
                errors_file_text(cfg, &issues),
                pime_settings::report::errors_file_text(cfg, &issues)
            );
        }
        let other = Path::new(r"C:\x\other.toml");
        let issues = pime_core::load_config("[options]\ngh = \"kaf\"\n").issues;
        assert_eq!(
            errors_file_text(other, &issues),
            pime_settings::report::errors_file_text(other, &issues)
        );
    }

    #[test]
    fn missing_file_is_default_without_issues() {
        let dir = TempDir::new("missing");
        let (h, _) = handle(&dir);
        assert_eq!(*h.current(), ResolvedConfig::default());
        assert!(h.issues().is_empty());
        assert_eq!(h.badge_text(), None);
        assert!(!dir.path().join(ERRORS_FILE).exists());
        assert!(!h.reload_if_changed());
    }

    #[test]
    fn no_paths_is_default() {
        let h = ConfigHandle::with_source(None, private_signal());
        assert_eq!(*h.current(), ResolvedConfig::default());
        assert!(!h.reload_if_changed());
    }

    #[test]
    fn loads_and_writes_errors_file_with_bom_and_crlf() {
        let dir = TempDir::new("errors");
        let cfg = dir.path().join(CONFIG_FILE);
        std::fs::write(&cfg, "[options]\ndigits = \"ascii\"\ngh = \"kaf\"\n").unwrap();
        let (h, _) = handle(&dir);
        assert_ne!(*h.current(), ResolvedConfig::default(), "digits applied");
        assert_eq!(h.issues().len(), 1);
        let bytes = std::fs::read(dir.path().join(ERRORS_FILE)).unwrap();
        assert!(bytes.starts_with(b"\xEF\xBB\xBF"));
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        assert!(text.starts_with("Persian Phonetic IME: problems in your settings file\r\n"));
        assert!(text.contains(&format!("File: {}\r\n", cfg.display())));
        assert!(text.contains("config.toml, line 3"), "{text}");
        assert!(!text.replace("\r\n", "").contains('\n'), "CRLF only");
        let badge = h.badge_text().unwrap();
        assert!(badge.contains("config.toml, line 3"), "{badge}");
        assert!(badge.contains("skipped"), "{badge}");
    }

    #[test]
    fn reload_only_on_change_and_errors_file_removed_when_fixed() {
        let dir = TempDir::new("reload");
        let cfg = dir.path().join(CONFIG_FILE);
        let errors = dir.path().join(ERRORS_FILE);
        std::fs::write(&cfg, "[options]\ngh = \"ghain\n").unwrap();
        let (h, _) = handle(&dir);
        assert_eq!(fallback_of(&h.issues()), Fallback::WholeFile);
        assert!(h
            .badge_text()
            .unwrap()
            .contains("default settings are in use"));
        assert!(errors.exists());
        assert!(!h.reload_if_changed(), "nothing changed");

        std::fs::write(&cfg, "[options]\ndigits = \"ascii\"\n").unwrap();
        assert!(h.reload_if_changed());
        assert!(h.issues().is_empty());
        assert_eq!(h.badge_text(), None);
        assert!(!errors.exists());
        assert!(!h.reload_if_changed());

        std::fs::remove_file(&cfg).unwrap();
        assert!(h.reload_if_changed(), "deleted file -> defaults");
        assert_eq!(*h.current(), ResolvedConfig::default());
    }

    #[test]
    fn signal_forces_a_reread_even_with_the_same_stamp() {
        let dir = TempDir::new("signal");
        let cfg = dir.path().join(CONFIG_FILE);
        std::fs::write(&cfg, "[options]\ndigits = \"ascii\"\n").unwrap();
        let (h, sig) = handle(&dir);
        let before = h.current();
        // Same size, then restore the old mtime so only the signal can reveal the edit.
        let mtime = std::fs::metadata(&cfg).unwrap().modified().unwrap();
        std::fs::write(&cfg, "[options]\ndigits = \"persn\"\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&cfg)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert!(!h.reload_if_changed(), "stamp identical");
        sig.fetch_add(1, Ordering::Release);
        assert!(h.reload_if_changed());
        assert_ne!(h.current(), before);
        assert_eq!(h.issues().len(), 1);
        // A signal with no real change reports false.
        sig.fetch_add(1, Ordering::Release);
        assert!(!h.reload_if_changed());
    }

    #[test]
    fn non_text_file_is_whole_file_fallback() {
        let dir = TempDir::new("binary");
        std::fs::write(dir.path().join(CONFIG_FILE), [0x80u8, 0x00, 0xC3]).unwrap();
        let (h, _) = handle(&dir);
        assert_eq!(*h.current(), ResolvedConfig::default());
        assert_eq!(fallback_of(&h.issues()), Fallback::WholeFile);
    }

    #[test]
    fn utf16_file_is_read() {
        let dir = TempDir::new("utf16");
        let mut bytes = vec![0xFF, 0xFE];
        for u in "[options]\r\ndigits = \"ascii\"\r\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(dir.path().join(CONFIG_FILE), bytes).unwrap();
        let (h, _) = handle(&dir);
        assert!(h.issues().is_empty(), "{:?}", h.issues());
        assert_ne!(*h.current(), ResolvedConfig::default());
    }

    #[test]
    fn errors_file_unchanged_content_is_not_rewritten() {
        let dir = TempDir::new("nochurn");
        let cfg = dir.path().join(CONFIG_FILE);
        let errors = dir.path().join(ERRORS_FILE);
        let issues = pime_core::load_config("[options]\ndiigts = \"ascii\"\n").issues;
        write_errors_file(&errors, &cfg, &issues).unwrap();
        let t0 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&errors)
            .unwrap()
            .set_modified(t0)
            .unwrap();
        write_errors_file(&errors, &cfg, &issues).unwrap();
        assert_eq!(std::fs::metadata(&errors).unwrap().modified().unwrap(), t0);
        write_errors_file(&errors, &cfg, &[]).unwrap();
        assert!(!errors.exists());
        write_errors_file(&errors, &cfg, &[]).unwrap();
        // Only the report itself: no stray temp files.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn badge_text_variants() {
        assert_eq!(badge_text_for("config.toml", &[]), None);
        let warn = pime_core::load_config("[map.singles]\ny = [\"\u{064A}\"]\n").issues;
        assert_eq!(fallback_of(&warn), Fallback::None);
        assert!(badge_text_for("config.toml", &warn)
            .unwrap()
            .contains("warning"));
        let two = pime_core::load_config("[options]\ndiigts = \"ascii\"\ngh = \"kaf\"\n").issues;
        let t = badge_text_for("config.toml", &two).unwrap();
        assert!(t.contains("(+1 more)") && t.contains("line 2"), "{t}");
    }
}
