//! Runs the real `pime-settings` binary against temporary config folders
//! (`PIME_CONFIG_DIR`), never the user's own `%APPDATA%` files. Window commands are
//! not run here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let d =
            std::env::temp_dir().join(format!("pime-settings-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(env!("CARGO_BIN_EXE_pime-settings"))
        .args(args)
        .env("PIME_CONFIG_DIR", dir)
        .output()
        .unwrap();
    (
        status.code().unwrap(),
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn check_missing_bad_value_and_broken_file() {
    let d = Dir::new("check");
    let (code, out, _) = run(&d.0, &["--check"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("default settings are in use"), "{out}");

    let cfg = d.0.join("config.toml");
    std::fs::write(
        &cfg,
        "[options]\ndigits = \"ascii\"\ngh = \"kaf\"\ndiigts = 1\n",
    )
    .unwrap();
    let (code, out, _) = run(&d.0, &["--check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("\"kaf\" isn't one of the choices for gh. Use \"ghain\" or \"qaf\"."),
        "{out}"
    );
    assert!(
        out.contains("unknown setting \"diigts\". Did you mean \"digits\"?"),
        "{out}"
    );

    std::fs::write(&cfg, "[options]\ngh = \"ghain\n").unwrap();
    let (code, out, _) = run(&d.0, &["--check"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("a closing quote is missing"), "{out}");
    assert!(
        out.contains("Your whole settings file was skipped"),
        "{out}"
    );

    let (code, _, err) = run(&d.0, &["--check", "does-not-exist.toml"]);
    assert_eq!(code, 3, "{err}");
}

#[test]
fn print_default_is_a_clean_config() {
    let d = Dir::new("default");
    let (code, out, _) = run(&d.0, &["--print-default"]);
    assert_eq!(code, 0);
    let path = d.0.join("printed.toml");
    std::fs::write(&path, &out).unwrap();
    let (code, report, _) = run(&d.0, &["--check", path.to_str().unwrap()]);
    assert_eq!(code, 0, "{report}");
    assert!(report.contains("no problems found"), "{report}");
}

#[test]
fn write_errors_then_reset() {
    let d = Dir::new("reset");
    let cfg = d.0.join("config.toml");
    let errors = d.0.join("config-errors.txt");
    std::fs::write(&cfg, "[options]\nenter = \"pass\"\n").unwrap();
    let (code, _, _) = run(&d.0, &["--write-errors"]);
    assert_eq!(code, 1);
    let text = std::fs::read_to_string(&errors).unwrap();
    assert!(text.contains("\"commit\" or \"commit-and-pass\""), "{text}");

    let (code, out, err) = run(&d.0, &["--reset"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(!errors.exists());
    let backups: Vec<_> = std::fs::read_dir(&d.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("config.toml.bak-"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read_to_string(d.0.join(&backups[0])).unwrap(),
        "[options]\nenter = \"pass\"\n"
    );
    let (code, _, _) = run(&d.0, &["--check"]);
    assert_eq!(code, 0);
}

#[test]
fn usage_errors() {
    let d = Dir::new("usage");
    assert_eq!(run(&d.0, &["--nope"]).0, 4);
    assert_eq!(run(&d.0, &["--reset", "extra"]).0, 4);
    let (code, out, _) = run(&d.0, &["--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("--check [FILE]"));
}
