//! Command line: parsing and the commands that need no window.

use crate::backup::{reset_config, Timestamp};
use crate::defaults::DEFAULT_CONFIG;
use crate::paths::Paths;
use crate::report::{exit, write_errors_file, Check};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

/// Usage text for `--help` and bad command lines.
pub const USAGE: &str = "\
Persian Phonetic IME settings

Usage:
  pime-settings                 open the settings window (same as --ui)
  pime-settings --ui            open the settings window
  pime-settings --show-errors   open the window on the settings-problems page
  pime-settings --check [FILE]  check a settings file (default: your config.toml)
  pime-settings --write-errors  check your config.toml and rewrite config-errors.txt
  pime-settings --reset         back up config.toml and replace it with the defaults
  pime-settings --print-default print a commented default config.toml
  pime-settings --open          open config.toml in your editor (created if missing)
  pime-settings --help          show this text

Exit codes: 0 no problems (notes allowed), 1 some settings were skipped,
2 the whole file was skipped, 3 a file couldn't be read or written, 4 bad usage.
Set PIME_CONFIG_DIR to use another folder for all files.
";

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The settings window.
    Ui,
    /// The window on the error page.
    ShowErrors,
    /// `--check [path]`.
    Check(Option<PathBuf>),
    /// `--write-errors`.
    WriteErrors,
    /// `--reset`.
    Reset,
    /// `--print-default`.
    PrintDefault,
    /// `--open`.
    Open,
    /// `--help`.
    Help,
}

/// Parses the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Ui);
    };
    let first_str = first.to_string_lossy();
    let cmd = match first_str.as_ref() {
        "--ui" => Command::Ui,
        "--show-errors" => Command::ShowErrors,
        "--check" => Command::Check(args.next().map(PathBuf::from)),
        "--write-errors" => Command::WriteErrors,
        "--reset" => Command::Reset,
        "--print-default" => Command::PrintDefault,
        "--open" => Command::Open,
        "--help" | "-h" | "/?" => Command::Help,
        other => return Err(format!("unknown option \"{other}\"")),
    };
    if let Some(extra) = args.next() {
        return Err(format!("unexpected \"{}\"", extra.to_string_lossy()));
    }
    Ok(cmd)
}

fn io_failure(err: &mut dyn Write, what: &str, path: &std::path::Path, e: std::io::Error) -> i32 {
    let _ = writeln!(err, "Couldn't {what} {}: {e}", path.display());
    exit::IO_ERROR
}

/// `--check [path]`: prints the friendly report; returns the exit code. An explicit
/// path that does not exist is an I/O error; the default path missing is fine.
pub fn run_check(
    paths: &Paths,
    path: Option<PathBuf>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let explicit = path.is_some();
    let path = path.unwrap_or_else(|| paths.config_file.clone());
    if explicit && !path.exists() {
        let _ = writeln!(err, "There is no file at {}.", path.display());
        return exit::IO_ERROR;
    }
    match Check::load(&path) {
        Ok(c) => {
            let _ = write!(out, "{}", c.report());
            c.exit_code()
        }
        Err(e) => io_failure(err, "read", &path, e),
    }
}

/// `--write-errors`: checks the user config and rewrites (or removes) the error file.
pub fn run_write_errors(paths: &Paths, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let c = match Check::load(&paths.config_file) {
        Ok(c) => c,
        Err(e) => return io_failure(err, "read", &paths.config_file, e),
    };
    match write_errors_file(&paths.errors_file, &paths.config_file, c.issues()) {
        Ok(true) => {
            let _ = writeln!(out, "Wrote {}", paths.errors_file.display());
            c.exit_code()
        }
        Ok(false) => {
            let _ = writeln!(
                out,
                "No problems; {} is cleared.",
                paths.errors_file.display()
            );
            c.exit_code()
        }
        Err(e) => io_failure(err, "write", &paths.errors_file, e),
    }
}

/// `--reset`: backs up and rewrites the config, clears the error file. The caller
/// signals the reload event afterwards.
pub fn run_reset(paths: &Paths, at: Timestamp, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match reset_config(&paths.config_file, at) {
        Ok(saved) => {
            if let Some(saved) = saved {
                let _ = writeln!(out, "Your old settings were saved as {}", saved.display());
            }
            let _ = writeln!(
                out,
                "{} now has the default settings.",
                paths.config_file.display()
            );
            if let Err(e) = write_errors_file(&paths.errors_file, &paths.config_file, &[]) {
                return io_failure(err, "clear", &paths.errors_file, e);
            }
            exit::OK
        }
        Err(e) => io_failure(err, "reset", &paths.config_file, e),
    }
}

/// `--print-default`.
pub fn run_print_default(out: &mut dyn Write) -> i32 {
    let _ = out.write_all(DEFAULT_CONFIG.as_bytes());
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn p(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn parsing() {
        assert_eq!(p(&[]), Ok(Command::Ui));
        assert_eq!(p(&["--check"]), Ok(Command::Check(None)));
        assert_eq!(
            p(&["--check", "a b.toml"]),
            Ok(Command::Check(Some("a b.toml".into())))
        );
        assert_eq!(p(&["--reset"]), Ok(Command::Reset));
        assert_eq!(p(&["--show-errors"]), Ok(Command::ShowErrors));
        assert!(p(&["--reset", "x"]).is_err());
        assert!(p(&["--bogus"]).unwrap_err().contains("--bogus"));
    }

    #[test]
    fn check_codes() {
        let dir = TempDir::new("cli-check");
        let paths = Paths::in_dir(dir.path());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run_check(&paths, None, &mut out, &mut err), exit::OK);
        let missing = Some(dir.path().join("nope.toml"));
        assert_eq!(
            run_check(&paths, missing, &mut out, &mut err),
            exit::IO_ERROR
        );
        std::fs::write(&paths.config_file, "[options\n").unwrap();
        assert_eq!(
            run_check(&paths, None, &mut out, &mut err),
            exit::WHOLE_FILE
        );
    }

    #[test]
    fn write_errors_and_reset() {
        let dir = TempDir::new("cli-reset");
        let paths = Paths::in_dir(dir.path());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        std::fs::write(&paths.config_file, "[options]\nq = 1\n").unwrap();
        assert_eq!(
            run_write_errors(&paths, &mut out, &mut err),
            exit::SKIPPED_ENTRIES
        );
        assert!(paths.errors_file.exists());
        let at = Timestamp::from_unix(0);
        assert_eq!(run_reset(&paths, at, &mut out, &mut err), exit::OK);
        assert!(!paths.errors_file.exists());
        assert!(dir.path().join("config.toml.bak-19700101-000000").exists());
        assert_eq!(run_check(&paths, None, &mut out, &mut err), exit::OK);
    }
}
