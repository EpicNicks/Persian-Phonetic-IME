//! Where the IME keeps its files.
//!
//! | File | Default location |
//! |---|---|
//! | user config | `%APPDATA%\PersianPhoneticIme\config.toml` (roams) |
//! | error report | `%LOCALAPPDATA%\PersianPhoneticIme\config-errors.txt` |
//! | history (phase 2) | `%LOCALAPPDATA%\PersianPhoneticIme\` |
//!
//! When the environment variable `PIME_CONFIG_DIR` is set (tests, portable use),
//! all three live directly in that directory instead.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Environment variable that overrides every location (tests use it).
pub const CONFIG_DIR_ENV: &str = "PIME_CONFIG_DIR";
/// Folder name under `%APPDATA%` and `%LOCALAPPDATA%`.
pub const APP_DIR: &str = "PersianPhoneticIme";
/// User config file name.
pub const CONFIG_FILE: &str = "config.toml";
/// Error report file name.
pub const ERRORS_FILE: &str = "config-errors.txt";

/// Resolved file locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The user config file.
    pub config_file: PathBuf,
    /// The plain-text error report, rewritten on every load.
    pub errors_file: PathBuf,
    /// Directory for typing history and other local (non-roaming) data.
    pub history_dir: PathBuf,
}

impl Paths {
    /// Locations from the real process environment.
    pub fn from_env() -> Option<Paths> {
        Paths::resolve(|k| std::env::var_os(k))
    }

    /// Locations from an environment lookup (pure, for tests). `None` only when
    /// neither `PIME_CONFIG_DIR` nor `APPDATA`/`LOCALAPPDATA` is available.
    pub fn resolve(env: impl Fn(&str) -> Option<OsString>) -> Option<Paths> {
        let non_empty = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(dir) = non_empty(CONFIG_DIR_ENV) {
            return Some(Paths::in_dir(&dir));
        }
        let roaming = non_empty("APPDATA")?.join(APP_DIR);
        // LOCALAPPDATA is always set on supported Windows; fall back to roaming.
        let local = non_empty("LOCALAPPDATA")
            .map(|p| p.join(APP_DIR))
            .unwrap_or_else(|| roaming.clone());
        Some(Paths {
            config_file: roaming.join(CONFIG_FILE),
            errors_file: local.join(ERRORS_FILE),
            history_dir: local,
        })
    }

    /// Every file directly inside `dir`.
    pub fn in_dir(dir: &Path) -> Paths {
        Paths {
            config_file: dir.join(CONFIG_FILE),
            errors_file: dir.join(ERRORS_FILE),
            history_dir: dir.to_path_buf(),
        }
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
    fn default_locations() {
        let p = Paths::resolve(env_of(&[
            ("APPDATA", r"C:\U\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\U\AppData\Local"),
        ]))
        .unwrap();
        assert_eq!(
            p.config_file,
            Path::new(r"C:\U\AppData\Roaming")
                .join("PersianPhoneticIme")
                .join("config.toml")
        );
        assert_eq!(
            p.errors_file,
            Path::new(r"C:\U\AppData\Local")
                .join("PersianPhoneticIme")
                .join("config-errors.txt")
        );
        assert_eq!(
            p.history_dir,
            Path::new(r"C:\U\AppData\Local").join("PersianPhoneticIme")
        );
    }

    #[test]
    fn override_wins_and_empty_override_is_ignored() {
        let vars = [("PIME_CONFIG_DIR", r"D:\t"), ("APPDATA", r"C:\R")];
        let p = Paths::resolve(env_of(&vars)).unwrap();
        assert_eq!(p, Paths::in_dir(Path::new(r"D:\t")));

        let vars = [("PIME_CONFIG_DIR", ""), ("APPDATA", r"C:\R")];
        let p = Paths::resolve(env_of(&vars)).unwrap();
        assert_eq!(
            p.config_file,
            Path::new(r"C:\R").join(APP_DIR).join(CONFIG_FILE)
        );
        assert_eq!(p.history_dir, Path::new(r"C:\R").join(APP_DIR));
    }

    #[test]
    fn nothing_known() {
        assert_eq!(Paths::resolve(|_| None), None);
    }
}
