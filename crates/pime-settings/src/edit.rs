//! Changing one setting in `config.toml` while keeping the user's comments,
//! formatting and every unrelated entry exactly as they were (`toml_edit`).
//!
//! Validation is delegated to `pime-core`: a change is accepted only if loading
//! the edited file gives the setting exactly the requested effective value, so
//! the allowed values are never duplicated here.

use crate::text::encode_config_text;
use pime_core::{
    decode_config_bytes, load_config, ConfigIssue, OptionValue, Severity, TextEncoding,
};
use std::fmt;
use std::io;
use std::path::Path;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

/// Where a setting lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Top level of the file (only `preset`).
    Root,
    /// `[options]`.
    Options,
    /// `[autocomplete]` (phase 2).
    Autocomplete,
}

impl Section {
    fn table_name(self) -> Option<&'static str> {
        match self {
            Section::Root => None,
            Section::Options => Some("options"),
            Section::Autocomplete => Some("autocomplete"),
        }
    }
}

/// Why a change was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The file itself cannot be read as settings, so it cannot be edited safely.
    FileUnreadable,
    /// The section exists but is not a table (e.g. `options = 3`).
    NotATable(&'static str),
    /// The value is not allowed; the friendly explanation from `pime-core`.
    Invalid(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::FileUnreadable => write!(
                f,
                "Your settings file has a problem that stops it from being read, so it can't be \
                 changed here. Fix the file or reset it to the defaults first."
            ),
            EditError::NotATable(name) => write!(
                f,
                "\"{name}\" in your settings file isn't a section, so it can't be changed here. \
                 Fix the file or reset it to the defaults first."
            ),
            EditError::Invalid(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for EditError {}

/// Converts a setting value to its TOML form.
pub fn to_toml_value(value: &OptionValue) -> Value {
    match value {
        OptionValue::Text(s) => Value::from(s.as_str()),
        OptionValue::Bool(b) => Value::from(*b),
        OptionValue::Integer(i) => Value::from(*i),
        OptionValue::List(items) => {
            let mut a = Array::new();
            for s in items {
                a.push(s.as_str());
            }
            Value::Array(a)
        }
    }
}

fn effective(text: &str, section: Section, key: &str) -> Option<OptionValue> {
    let cfg = load_config(text).config;
    match section {
        Section::Root if key == "preset" => Some(OptionValue::Text(cfg.preset_name().to_string())),
        Section::Root => None,
        Section::Options => cfg.option_value(key),
        Section::Autocomplete => cfg.option_value(&format!("autocomplete.{key}")),
    }
}

fn explain(issues: &[ConfigIssue]) -> String {
    let errors: Vec<String> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| {
            if i.hint.is_empty() {
                i.message.clone()
            } else {
                format!("{} {}", i.message, i.hint)
            }
        })
        .collect();
    if errors.is_empty() {
        "That value can't be used here.".into()
    } else {
        errors.join(" ")
    }
}

/// Explains why `value` is not allowed for `key`, or `None` if it is (checked in
/// isolation, on the default preset).
pub fn check_value(section: Section, key: &str, value: &OptionValue) -> Option<String> {
    let probe = set_in_text("", section, key, value).ok()?;
    if effective(&probe, section, key).as_ref() == Some(value) {
        None
    } else {
        Some(explain(&load_config(&probe).issues))
    }
}

fn set_in_text(
    text: &str,
    section: Section,
    key: &str,
    value: &OptionValue,
) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse().map_err(|_| EditError::FileUnreadable)?;
    let table = match section.table_name() {
        None => doc.as_table_mut() as &mut dyn toml_edit::TableLike,
        Some(name) => {
            let item = doc.entry(name).or_insert(Item::Table(Table::new()));
            item.as_table_like_mut().ok_or(EditError::NotATable(name))?
        }
    };
    let mut new = to_toml_value(value);
    match table.get_mut(key) {
        Some(Item::Value(old)) => {
            // Keep the spacing and any trailing comment on the line.
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        _ => {
            table.insert(key, Item::Value(new));
        }
    }
    Ok(doc.to_string())
}

/// Returns `text` with one setting changed (or added), everything else untouched.
/// Fails if the file cannot be parsed or `pime-core` rejects the value.
pub fn set_setting(
    text: &str,
    section: Section,
    key: &str,
    value: &OptionValue,
) -> Result<String, EditError> {
    let out = set_in_text(text, section, key, value)?;
    if effective(&out, section, key).as_ref() == Some(value) {
        Ok(out)
    } else {
        // Explain from the value alone when possible; the file context may add noise.
        Err(EditError::Invalid(
            check_value(section, key, value).unwrap_or_else(|| explain(&load_config(&out).issues)),
        ))
    }
}

/// Why saving a setting failed.
#[derive(Debug)]
pub enum SaveError {
    /// The change itself was refused.
    Edit(EditError),
    /// Reading or writing the file failed.
    Io(io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Edit(e) => e.fmt(f),
            SaveError::Io(e) => write!(f, "The settings file couldn't be saved ({e})."),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

/// Reads `path` (a missing file starts from the commented default), changes one
/// setting and writes the file back atomically. Does not signal the reload event.
pub fn save_setting(
    path: &Path,
    section: Section,
    key: &str,
    value: &OptionValue,
) -> Result<(), SaveError> {
    let (text, encoding) = match std::fs::read(path) {
        Ok(bytes) => {
            let d = decode_config_bytes(&bytes)
                .map_err(|_| SaveError::Edit(EditError::FileUnreadable))?;
            (d.text, d.encoding)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (
            crate::defaults::DEFAULT_CONFIG.to_string(),
            TextEncoding::Utf8,
        ),
        Err(e) => return Err(e.into()),
    };
    let out = set_setting(&text, section, key, value).map_err(SaveError::Edit)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomically(path, &encode_config_text(&out, encoding))?;
    Ok(())
}

/// Writes via a temporary file and a rename, so the IME never reads a half-written
/// file. Falls back to a direct write if the rename is refused (an editor holding
/// the file open without delete sharing).
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(path, bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn text(s: &str) -> OptionValue {
        OptionValue::Text(s.into())
    }

    const USER: &str = "# my settings\n\
        preset = \"qamooscheh\"  # base\n\
        \n\
        [options]\n\
        # I like qaf\n\
        gh   =   \"ghain\"   # was ghain\n\
        digits = \"ascii\"\n\
        \n\
        [map.singles]\n\
        v = [\"و\"]  # keep\n";

    #[test]
    fn replaces_value_keeping_comments_and_spacing() {
        let out = set_setting(USER, Section::Options, "gh", &text("qaf")).unwrap();
        assert_eq!(out, USER.replace("\"ghain\"   # was", "\"qaf\"   # was"));
        let cfg = load_config(&out).config;
        assert_eq!(cfg.candidates("gh").unwrap()[0], "ق");
        assert_eq!(cfg.option_value("digits"), Some(text("ascii")));
    }

    #[test]
    fn adds_missing_key_and_section() {
        let out = set_setting(
            USER,
            Section::Options,
            "number_select",
            &OptionValue::Bool(false),
        )
        .unwrap();
        assert!(out.contains("number_select = false"));
        assert!(out.contains("# I like qaf") && out.contains("v = [\"و\"]  # keep"));

        let out = set_setting(
            "# only a comment
",
            Section::Options,
            "c",
            &text("k"),
        )
        .unwrap();
        assert!(
            out.contains(
                "# only a comment
"
            ) && out.contains(
                "[options]
c = \"k\"
"
            )
        );
        let out = set_setting(
            "",
            Section::Autocomplete,
            "predict_min_chars",
            &OptionValue::Integer(3),
        )
        .unwrap();
        assert!(
            out.contains("[autocomplete]\npredict_min_chars = 3"),
            "{out}"
        );
    }

    #[test]
    fn inline_options_table_is_edited_in_place() {
        let out = set_setting(
            "options = { gh = \"ghain\" }\n",
            Section::Options,
            "gh",
            &text("qaf"),
        )
        .unwrap();
        assert_eq!(out, "options = { gh = \"qaf\" }\n");
    }

    #[test]
    fn preset_is_set_at_top_level() {
        let out = set_setting(USER, Section::Root, "preset", &text("minimal")).unwrap();
        assert!(out.contains("preset = \"minimal\"  # base"), "{out}");
        assert!(set_setting(USER, Section::Root, "preset", &text("nope")).is_err());
    }

    #[test]
    fn invalid_values_are_refused_with_core_message() {
        let e = set_setting(USER, Section::Options, "gh", &text("kaf")).unwrap_err();
        let EditError::Invalid(msg) = e else {
            panic!("{e:?}")
        };
        assert!(msg.contains("\"ghain\" or \"qaf\""), "{msg}");
        let e = set_setting(USER, Section::Options, "number_select", &text("yes")).unwrap_err();
        assert!(matches!(e, EditError::Invalid(_)), "{e:?}");
        let e = set_setting(USER, Section::Options, "diigts", &text("ascii")).unwrap_err();
        assert!(e.to_string().contains("digits"), "{e}");
    }

    #[test]
    fn broken_file_is_not_touched() {
        let broken = "[options]\ngh = \"ghain\n";
        assert_eq!(
            set_setting(broken, Section::Options, "gh", &text("qaf")),
            Err(EditError::FileUnreadable)
        );
        assert_eq!(
            set_setting("options = 3\n", Section::Options, "gh", &text("qaf")),
            Err(EditError::NotATable("options"))
        );
    }

    #[test]
    fn list_values_respect_flags_in_the_same_file() {
        let file = "[options]\nallow_sequences = true\n";
        let list = OptionValue::List(vec!["لا".into(), "س".into()]);
        assert!(set_setting(file, Section::Options, "s_order", &list).is_ok());
        assert!(set_setting("", Section::Options, "s_order", &list).is_err());
    }

    #[test]
    fn save_creates_from_default_and_preserves_bom() {
        let dir = TempDir::new("save");
        let path = dir.path().join("new").join("config.toml");
        save_setting(&path, Section::Options, "digits", &text("ascii")).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with(crate::defaults::DEFAULT_CONFIG.lines().next().unwrap()));
        assert!(written.contains("digits = \"ascii\""));
        assert!(load_config(&written).issues.is_empty());

        std::fs::write(&path, b"\xEF\xBB\xBF[options]\ngh = \"ghain\"\n").unwrap();
        save_setting(&path, Section::Options, "gh", &text("qaf")).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\xEF\xBB\xBF[options]\ngh = \"qaf\"\n"
        );
        // No temporary file left behind.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn save_refuses_unreadable_file() {
        let dir = TempDir::new("save-bad");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "gh = \"\n").unwrap();
        let e = save_setting(&path, Section::Options, "gh", &text("qaf")).unwrap_err();
        assert!(matches!(e, SaveError::Edit(EditError::FileUnreadable)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "gh = \"\n");
    }

    #[test]
    fn check_value_in_isolation() {
        assert_eq!(check_value(Section::Options, "c", &text("s")), None);
        assert!(check_value(Section::Options, "c", &text("x"))
            .unwrap()
            .contains("\"none\""));
    }
}
