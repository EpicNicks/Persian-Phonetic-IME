//! Checking a config file and reporting the result: the `--check` text, exit codes
//! and `config-errors.txt`.

use pime_core::{
    decode_config_bytes, fallback_of, format_issues_for, load_config, ConfigIssue, Fallback,
    LoadedConfig, TextEncoding,
};
use std::io;
use std::path::{Path, PathBuf};

/// Exit codes of `pime-settings` (stable; scripts and CI rely on them).
pub mod exit {
    /// No problems, or only warnings (everything in the file was applied).
    pub const OK: i32 = 0;
    /// Some entries were skipped; the rest of the file is in use.
    pub const SKIPPED_ENTRIES: i32 = 1;
    /// The whole file was skipped (unreadable); the built-in defaults are in use.
    pub const WHOLE_FILE: i32 = 2;
    /// A file could not be read or written (missing explicit path, permissions, ...).
    pub const IO_ERROR: i32 = 3;
    /// Bad command line.
    pub const USAGE: i32 = 4;
}

/// Exit code for a list of issues.
pub fn exit_code(issues: &[ConfigIssue]) -> i32 {
    match fallback_of(issues) {
        Fallback::None => exit::OK,
        Fallback::Entry => exit::SKIPPED_ENTRIES,
        Fallback::WholeFile => exit::WHOLE_FILE,
    }
}

/// What was found on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// No file: the defaults are in use, which is not a problem.
    Missing,
    /// A text file in this encoding.
    Present(TextEncoding),
    /// The file exists but is not text.
    NotText,
}

/// The result of loading one config file.
#[derive(Debug, Clone)]
pub struct Check {
    /// The file that was checked.
    pub path: PathBuf,
    /// Whether it existed and could be decoded.
    pub state: FileState,
    /// The effective configuration and the problems found.
    pub loaded: LoadedConfig,
}

impl Check {
    /// Loads `path`. A missing file is not an error (the defaults apply); other
    /// I/O failures are returned.
    pub fn load(path: &Path) -> io::Result<Check> {
        let (state, loaded) = match std::fs::read(path) {
            Ok(bytes) => match decode_config_bytes(&bytes) {
                Ok(d) => (FileState::Present(d.encoding), load_config(&d.text)),
                Err(issue) => {
                    let mut loaded = load_config("");
                    loaded.issues = vec![issue];
                    (FileState::NotText, loaded)
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (FileState::Missing, load_config("")),
            Err(e) => return Err(e),
        };
        Ok(Check {
            path: path.to_path_buf(),
            state,
            loaded,
        })
    }

    /// The problems found (empty for a missing or clean file).
    pub fn issues(&self) -> &[ConfigIssue] {
        &self.loaded.issues
    }

    /// The process exit code for this result.
    pub fn exit_code(&self) -> i32 {
        exit_code(self.issues())
    }

    /// The text `--check` prints.
    pub fn report(&self) -> String {
        let path = self.path.display();
        if self.state == FileState::Missing {
            return format!(
                "There is no settings file at {path} yet, so the default settings are in use.\n"
            );
        }
        if self.issues().is_empty() {
            return format!("{path}: no problems found.\n");
        }
        format!("Checked {path}\n\n{}", self.issues_text())
    }

    /// The friendly problem text, naming the checked file by its real name
    /// (`config.toml` for the user's own file, `other.toml` for `--check other.toml`).
    pub fn issues_text(&self) -> String {
        format_issues_for(&file_label(&self.path), self.issues())
    }
}

/// The file name used in messages.
pub fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::paths::CONFIG_FILE.to_string())
}

/// Content of `config-errors.txt`, or `None` when there is nothing to report (the
/// file should then be removed). Warnings are reported too.
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

/// Rewrites (or removes, when clean) `config-errors.txt`. Returns whether a file
/// was written.
pub fn write_errors_file(
    errors_path: &Path,
    config_path: &Path,
    issues: &[ConfigIssue],
) -> io::Result<bool> {
    match errors_file_text(config_path, issues) {
        Some(text) => {
            if let Some(dir) = errors_path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // BOM so Notepad on older Windows 10 builds shows the Persian quotes correctly.
            let mut bytes = b"\xEF\xBB\xBF".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            crate::edit::write_atomically(errors_path, &bytes)?;
            Ok(true)
        }
        None => match std::fs::remove_file(errors_path) {
            Ok(()) => Ok(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn exit_codes_follow_fallback() {
        assert_eq!(exit_code(&load_config("").issues), exit::OK);
        let bad_value = load_config("[options]\ngh = \"kaf\"\n");
        assert_eq!(exit_code(&bad_value.issues), exit::SKIPPED_ENTRIES);
        let broken = load_config("[options]\ngh = \"ghain\n");
        assert_eq!(exit_code(&broken.issues), exit::WHOLE_FILE);
        let warning = load_config("[map.singles]\ny = [\"\u{064A}\"]\n");
        assert!(!warning.issues.is_empty());
        assert_eq!(exit_code(&warning.issues), exit::OK);
    }

    #[test]
    fn missing_file_is_fine() {
        let dir = TempDir::new("missing");
        let c = Check::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(c.state, FileState::Missing);
        assert_eq!(c.exit_code(), exit::OK);
        assert!(c.report().contains("default settings are in use"));
    }

    #[test]
    fn friendly_report_for_bad_value() {
        let dir = TempDir::new("badvalue");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[options]\ndigits = \"ascii\"\ngh = \"kaf\"\n").unwrap();
        let c = Check::load(&path).unwrap();
        assert_eq!(c.exit_code(), exit::SKIPPED_ENTRIES);
        let r = c.report();
        assert!(r.contains("line 3"), "{r}");
        assert!(r.contains("\"kaf\" isn't one of the choices for gh"), "{r}");
        assert!(r.contains("\"ghain\" or \"qaf\""), "{r}");
        assert!(
            r.contains("everything else in your file is still in use"),
            "{r}"
        );
    }

    #[test]
    fn binary_file_is_whole_file_fallback() {
        let dir = TempDir::new("binary");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, [0x80u8, 0x00, 0xC3]).unwrap();
        let c = Check::load(&path).unwrap();
        assert_eq!(c.state, FileState::NotText);
        assert_eq!(c.exit_code(), exit::WHOLE_FILE);
    }

    #[test]
    fn report_names_the_checked_file() {
        let dir = TempDir::new("named");
        let path = dir.path().join("other.toml");
        std::fs::write(&path, "[options]\ngh = \"ghain\n").unwrap();
        let r = Check::load(&path).unwrap().report();
        assert!(r.contains("other.toml, line 2"), "{r}");
        assert!(!r.contains("config.toml"), "{r}");
    }

    #[test]
    fn bom_file_is_read() {
        let dir = TempDir::new("bom");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, b"\xEF\xBB\xBF[options]\ndigits = \"ascii\"\n").unwrap();
        let c = Check::load(&path).unwrap();
        assert!(c.issues().is_empty(), "{:?}", c.issues());
        assert_eq!(c.state, FileState::Present(TextEncoding::Utf8Bom));
    }

    #[test]
    fn errors_file_written_then_removed() {
        let dir = TempDir::new("errfile");
        let errors = dir.path().join("sub").join("config-errors.txt");
        let cfg = dir.path().join("config.toml");
        let issues = load_config("[options]\ndiigts = \"ascii\"\n").issues;
        assert!(write_errors_file(&errors, &cfg, &issues).unwrap());
        let text = std::fs::read_to_string(&errors).unwrap();
        assert!(text.contains("Did you mean \"digits\""), "{text}");
        assert!(text.contains(&cfg.display().to_string()));
        assert!(!write_errors_file(&errors, &cfg, &[]).unwrap());
        assert!(!errors.exists());
        // Removing an absent file is fine too.
        assert!(!write_errors_file(&errors, &cfg, &[]).unwrap());
    }
}
