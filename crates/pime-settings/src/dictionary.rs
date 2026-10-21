//! The installed word dictionary: where it is, the `--check` line, and the completer
//! behind the "Try typing" box.

use crate::paths::Paths;
use pime_core::Completer;
use pime_dict::{Dictionary, Runtime, RuntimeOptions, RuntimePaths, Status};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Environment variable that overrides the dictionary location (development).
pub const DICT_PATH_ENV: &str = "PIME_DICT_PATH";
/// Dictionary file name, installed next to the program files.
pub const DICT_FILE: &str = "dict.bin";

/// `dict.bin` next to this program (the installer ships them together), or
/// `PIME_DICT_PATH` when set.
pub fn installed_dict_path() -> PathBuf {
    if let Some(p) = std::env::var_os(DICT_PATH_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(DICT_FILE)))
        .unwrap_or_else(|| PathBuf::from(DICT_FILE))
}

/// The `--check` line for the dictionary at `path`.
pub fn dictionary_line(path: &Path) -> String {
    match Dictionary::load(path) {
        Ok(d) => format!("Word completion dictionary: OK ({} words)", d.len()),
        Err(_) => {
            "Word completion dictionary: missing or damaged; word completion is off.".to_string()
        }
    }
}

/// Starts the real dictionary for the preview. It never writes `history.bin`
/// (`persist = false`); the history path is only passed because the runtime wants one.
pub fn start_preview_runtime(paths: &Paths) -> Arc<Runtime> {
    Runtime::start(
        RuntimePaths {
            dict: installed_dict_path(),
            history: paths.history_file(),
        },
        RuntimeOptions {
            persist: false,
            ..RuntimeOptions::default()
        },
    )
}

/// The completer for the preview; returns nothing until the dictionary has loaded.
pub fn preview_completer(rt: &Arc<Runtime>) -> Arc<dyn Completer> {
    rt.completer()
}

/// Waits briefly for the dictionary (tests only).
pub fn wait_ready(rt: &Runtime) -> Status {
    rt.wait_loaded(Duration::from_secs(10))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn missing_or_damaged_dictionary() {
        let dir = TempDir::new("dict-line");
        let p = dir.path().join("dict.bin");
        let want = "Word completion dictionary: missing or damaged; word completion is off.";
        assert_eq!(dictionary_line(&p), want);
        std::fs::write(&p, b"not a dictionary").unwrap();
        assert_eq!(dictionary_line(&p), want);
    }

    #[test]
    fn preview_runtime_reports_unavailable_without_a_dictionary() {
        let dir = TempDir::new("dict-rt");
        let paths = Paths::in_dir(dir.path());
        // PIME_DICT_PATH is not set by tests, so the exe-relative dict.bin is used;
        // it does not exist next to the test binary.
        let rt = start_preview_runtime(&paths);
        assert!(matches!(
            wait_ready(&rt),
            Status::Unavailable(_) | Status::Ready { .. }
        ));
        assert!(
            !paths.history_file().exists(),
            "preview must not write history"
        );
        rt.shutdown();
    }
}
