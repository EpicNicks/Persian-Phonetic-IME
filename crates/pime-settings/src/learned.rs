//! Learned-word history: where it lives, how it is cleared, and the form rows of the
//! "Word completion" section. The history file is only ever deleted or counted
//! here; the words in it are never read, printed or logged.

use crate::edit::Section;
use crate::paths::Paths;
use pime_dict::History;
use std::io;
use std::path::PathBuf;

/// The history file written by the text service.
pub const HISTORY_FILE: &str = "history.bin";
/// Files removed by "clear": the history, its half-written copy and the quarantined bad copy.
pub const HISTORY_FILES: [&str; 3] = ["history.bin", "history.bin.tmp", "history.bin.bad"];
/// Printed by `--clear-history`.
pub const CLEARED_MESSAGE: &str = "Learned words cleared.";
/// The privacy line shown in the form.
pub const PRIVACY_LINE: &str = "Learned words stay on this computer. Nothing is sent anywhere.";

impl Paths {
    /// The history file (same rules as the text service: `PIME_CONFIG_DIR`, else
    /// `%LOCALAPPDATA%\PersianPhoneticIme`).
    pub fn history_file(&self) -> PathBuf {
        self.history_dir.join(HISTORY_FILE)
    }
}

/// Deletes the history files. Missing files are fine. Returns how many were removed.
pub fn clear_history(paths: &Paths) -> io::Result<usize> {
    let mut removed = 0;
    let mut first_err = None;
    for name in HISTORY_FILES {
        match std::fs::remove_file(paths.history_dir.join(name)) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(removed),
    }
}

/// "0 learned words", "1 learned word", "N learned words".
pub fn count_text(n: usize) -> String {
    format!("{n} learned word{}", if n == 1 { "" } else { "s" })
}

/// A yes/no row of the "Word completion" section.
#[derive(Debug, Clone, Copy)]
pub struct CheckRow {
    /// Key in `[autocomplete]`.
    pub key: &'static str,
    /// Row label.
    pub label: &'static str,
}

/// A whole-number row of the "Word completion" section.
#[derive(Debug, Clone, Copy)]
pub struct NumberRow {
    /// Key in `[autocomplete]`.
    pub key: &'static str,
    /// Row label.
    pub label: &'static str,
    /// Smallest value the form offers.
    pub min: i64,
    /// Largest value the form offers.
    pub max: i64,
}

/// The checkboxes, in form order.
pub const CHECK_ROWS: &[CheckRow] = &[
    CheckRow {
        key: "enabled",
        label: "Suggest word completions",
    },
    CheckRow {
        key: "learning",
        label: "Learn from my choices",
    },
    CheckRow {
        key: "fuzzy",
        label: "Suggest corrections for typos",
    },
    CheckRow {
        key: "filter_profanity",
        label: "Hide vulgar words",
    },
];

/// The number boxes, in form order.
pub const NUMBER_ROWS: &[NumberRow] = &[
    NumberRow {
        key: "predict_min_chars",
        label: "Start suggesting after N letters",
        min: 1,
        max: 10,
    },
    NumberRow {
        key: "max_completions",
        label: "Number of suggestions",
        min: 1,
        max: 9,
    },
    NumberRow {
        key: "fuzzy_min_chars",
        label: "Typo tolerance starts after N letters",
        min: 2,
        max: 10,
    },
];

/// The section the form rows' keys live in.
pub const SECTION: Section = Section::Autocomplete;

impl NumberRow {
    /// Parses what the user typed; digits only, clamped to the row's range.
    /// `None` for empty or non-numeric text (the box is left alone).
    pub fn parse(&self, text: &str) -> Option<i64> {
        let t = text.trim();
        if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n = t.parse::<i64>().unwrap_or(self.max);
        Some(n.clamp(self.min, self.max))
    }
}

/// Number of learned words, or `None` if the file is missing or damaged. Only the
/// count leaves this function; the words are never exposed.
pub fn learned_count(paths: &Paths) -> Option<usize> {
    let bytes = std::fs::read(paths.history_file()).ok()?;
    History::from_bytes(&bytes).ok().map(|h| h.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::run_clear_history;
    use crate::edit::set_setting;
    use crate::testutil::TempDir;
    use pime_core::{load_config, OptionValue};

    fn file(dir: &TempDir, name: &str) -> PathBuf {
        dir.path().join(name)
    }

    #[test]
    fn clear_removes_all_three_and_tolerates_missing() {
        let dir = TempDir::new("clear");
        let paths = Paths::in_dir(dir.path());
        assert_eq!(clear_history(&paths).unwrap(), 0);
        for n in HISTORY_FILES {
            std::fs::write(file(&dir, n), b"x").unwrap();
        }
        std::fs::write(file(&dir, "config.toml"), b"preset = \"minimal\"\n").unwrap();
        assert_eq!(clear_history(&paths).unwrap(), 3);
        for n in HISTORY_FILES {
            assert!(!file(&dir, n).exists());
        }
        assert!(file(&dir, "config.toml").exists());
    }

    #[test]
    fn cli_message_and_code() {
        let dir = TempDir::new("clear-cli");
        let paths = Paths::in_dir(dir.path());
        std::fs::write(file(&dir, "history.bin"), b"x").unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run_clear_history(&paths, &mut out, &mut err), 0);
        assert_eq!(String::from_utf8(out).unwrap(), "Learned words cleared.\n");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run_clear_history(&paths, &mut out, &mut err), 0);
        assert_eq!(String::from_utf8(out).unwrap(), "Learned words cleared.\n");
        assert!(err.is_empty());
    }

    #[test]
    fn history_path_rules() {
        let p = Paths::resolve(|k| (k == "PIME_CONFIG_DIR").then(|| r"D:\t".into())).unwrap();
        assert_eq!(p.history_file(), PathBuf::from(r"D:\t").join("history.bin"));
        let p = Paths::resolve(|k| match k {
            "APPDATA" => Some(r"C:\R".into()),
            "LOCALAPPDATA" => Some(r"C:\L".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            p.history_file(),
            PathBuf::from(r"C:\L")
                .join("PersianPhoneticIme")
                .join("history.bin")
        );
    }

    #[test]
    fn counts() {
        assert_eq!(count_text(0), "0 learned words");
        assert_eq!(count_text(1), "1 learned word");
        assert_eq!(count_text(12), "12 learned words");
        let dir = TempDir::new("count");
        let paths = Paths::in_dir(dir.path());
        assert_eq!(learned_count(&paths), None);
        std::fs::write(paths.history_file(), b"garbage").unwrap();
        assert_eq!(learned_count(&paths), None);
        std::fs::write(paths.history_file(), History::new().to_bytes()).unwrap();
        assert_eq!(learned_count(&paths), Some(0));
    }

    #[test]
    fn fuzzy_and_filter_rows_exist_with_their_ranges() {
        for k in ["fuzzy", "filter_profanity"] {
            assert!(CHECK_ROWS.iter().any(|r| r.key == k), "{k}");
        }
        let r = NUMBER_ROWS
            .iter()
            .find(|r| r.key == "fuzzy_min_chars")
            .unwrap();
        assert_eq!(
            (r.parse("1"), r.parse("99"), r.parse("4")),
            (Some(2), Some(10), Some(4))
        );
    }

    #[test]
    fn number_parsing_clamps() {
        let r = NUMBER_ROWS[0];
        assert_eq!(r.parse("3"), Some(3));
        assert_eq!(r.parse(" 0 "), Some(1));
        assert_eq!(r.parse("99"), Some(10));
        assert_eq!(r.parse("99999999999999999999"), Some(10));
        assert_eq!(r.parse(""), None);
        assert_eq!(r.parse("-2"), None);
        assert_eq!(r.parse("x"), None);
    }

    fn effective(text: &str, key: &str) -> Option<OptionValue> {
        load_config(text)
            .config
            .option_value(&format!("autocomplete.{key}"))
    }

    #[test]
    fn check_rows_round_trip_through_the_config() {
        for row in CHECK_ROWS {
            for want in [true, false] {
                let v = OptionValue::Bool(want);
                let text = set_setting("", SECTION, row.key, &v).unwrap();
                assert_eq!(effective(&text, row.key), Some(v), "{}", row.key);
            }
        }
    }

    #[test]
    fn number_rows_round_trip_through_the_config() {
        for row in NUMBER_ROWS {
            for n in [row.min, row.max] {
                let v = OptionValue::Integer(n);
                match set_setting("", SECTION, row.key, &v) {
                    Ok(text) => assert_eq!(effective(&text, row.key), Some(v), "{}", row.key),
                    Err(e) => panic!("{}: {e}", row.key),
                }
            }
        }
    }
}
