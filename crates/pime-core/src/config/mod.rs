//! Configuration: built-in presets, the user `config.toml`, and resolution into
//! an immutable [`ResolvedConfig`] (PLAN.md 3, docs/config-schema.md).
//!
//! Resolution order: preset -> `[options]` (preset values, then user values)
//! -> option expansion into map patches (`s_order`, `z_order`, `gh`, `q`, `c`)
//! -> raw `[map.*]` overrides -> validation. Loading never fails: problems are
//! returned as [`ConfigIssue`]s and the offending entry (or, for unreadable
//! files, the whole file) is skipped.

mod decode;
mod issues;
mod punctuation;

pub use decode::{decode_config_bytes, load_config_bytes, DecodedConfig, TextEncoding};

pub use issues::{
    fallback_of, format_issues, format_issues_for, ConfigIssue, Fallback, IssueKind, Severity,
};

use issues::{line_col, line_text, or_list, suggest, syntax_reason};
use std::collections::{BTreeMap, BTreeSet};
use toml_edit::{ImDocument, Item, TableLike, Value};

include!(concat!(env!("OUT_DIR"), "/embedded_data.rs"));

/// Used only when the build had no `data/presets/qamooscheh.toml` (the crate
/// built outside this repo); normally the data file is embedded instead.
const EMERGENCY_PRESET: &str = include_str!("emergency_preset.toml");

/// Name of the preset used when the user file names none (or an unknown one).
pub const DEFAULT_PRESET: &str = "qamooscheh";

/// Zero-width non-joiner, the only non-letter allowed as a candidate.
pub const ZWNJ: char = '\u{200C}';

/// Names of the built-in presets, default first.
pub fn builtin_preset_names() -> Vec<&'static str> {
    let mut names = vec![DEFAULT_PRESET];
    names.extend(
        DATA_PRESETS
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| *n != DEFAULT_PRESET),
    );
    names
}

fn preset_source(name: &str) -> Option<&'static str> {
    DATA_PRESETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, src)| *src)
        .or((name == DEFAULT_PRESET).then_some(EMERGENCY_PRESET))
}

// ---------------------------------------------------------------------------
// Option values and the typed view of them

/// A setting's value, as it appears in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    /// A text value such as `"ghain"`.
    Text(String),
    /// `true` / `false`.
    Bool(bool),
    /// A whole number.
    Integer(i64),
    /// A list of text values (candidate lists).
    List(Vec<String>),
}

/// How digits `0-9` are inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Digits {
    /// U+06F0..U+06F9.
    Persian,
    /// ASCII digits.
    Ascii,
}

/// Whether `, ; ? %` are converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punctuation {
    /// `,`->`،` `;`->`؛` `?`->`؟` `%`->`٪`.
    Persian,
    /// ASCII unchanged.
    None,
}

/// Optional « » insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guillemets {
    /// Off.
    None,
    /// `<` `>` insert « ».
    Angle,
    /// `{` `}` insert « ».
    Brace,
}

/// What Shift+letter does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftLetters {
    /// Fold to lowercase.
    Lowercase,
    /// Pick the token's second candidate.
    Alternate,
}

/// What Space commits while composing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceMode {
    /// Exactly what the preedit shows.
    Preedit,
    /// The top candidate (identical to `Preedit` in v1, which has no dictionary).
    TopCandidate,
}

/// Where the candidate window opens relative to the preedit (the host still
/// flips it when there is no room).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidatePosition {
    /// Below the preedit (Google Japanese IME style; default).
    Below,
    /// Above the preedit.
    Above,
}

/// Key combination that switches between Persian and Latin typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleHotkey {
    /// No hotkey (default; the language-bar button and Win+Space still work).
    None,
    /// Alt+` (the key above Tab).
    AltBackquote,
    /// Ctrl+`.
    CtrlBackquote,
    /// Ctrl+Space.
    CtrlSpace,
    /// Ctrl+Shift+Space.
    CtrlShiftSpace,
    /// Ctrl+Alt+Space.
    CtrlAltSpace,
}

impl ToggleHotkey {
    /// `(virtual key, ctrl, shift, alt)`; `None` when unbound.
    pub fn chord(self) -> Option<(u32, bool, bool, bool)> {
        use crate::keys::{VK_OEM_3, VK_SPACE};
        match self {
            ToggleHotkey::None => None,
            ToggleHotkey::AltBackquote => Some((VK_OEM_3, false, false, true)),
            ToggleHotkey::CtrlBackquote => Some((VK_OEM_3, true, false, false)),
            ToggleHotkey::CtrlSpace => Some((VK_SPACE, true, false, false)),
            ToggleHotkey::CtrlShiftSpace => Some((VK_SPACE, true, true, false)),
            ToggleHotkey::CtrlAltSpace => Some((VK_SPACE, true, false, true)),
        }
    }
}

/// What Enter does while composing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterMode {
    /// Commit; the key is eaten.
    Commit,
    /// Commit, then the key passes to the application.
    CommitAndPass,
}

/// `[autocomplete]` (phase 2; parsed and validated, unused in v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Autocomplete {
    /// Master switch.
    pub enabled: bool,
    /// Letters typed before completions are offered.
    pub predict_min_chars: i64,
    /// Whether committed words are remembered.
    pub learning: bool,
}

/// Behaviour options (the map-shaping options are already folded into the map).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// `digits`.
    pub digits: Digits,
    /// `punctuation`.
    pub punctuation: Punctuation,
    /// `guillemets`.
    pub guillemets: Guillemets,
    /// `shift_letters`.
    pub shift_letters: ShiftLetters,
    /// `space`.
    pub space: SpaceMode,
    /// `enter`.
    pub enter: EnterMode,
    /// `number_select`: 1-9 pick from an open candidate list.
    pub number_select: bool,
    /// `candidate_labels`: show learner hints in the candidate window.
    pub candidate_labels: bool,
    /// `candidate_position`: where the candidate window opens.
    pub candidate_position: CandidatePosition,
    /// `toggle_hotkey`: Persian/Latin switch.
    pub toggle_hotkey: ToggleHotkey,
    /// `candidate_size`: candidate window size in percent, clamped to
    /// `CANDIDATE_SIZE_MIN..=CANDIDATE_SIZE_MAX`.
    pub candidate_size: u32,
    /// `allow_arabic_forms`.
    pub allow_arabic_forms: bool,
    /// `allow_sequences`.
    pub allow_sequences: bool,
    /// `[autocomplete]`.
    pub autocomplete: Autocomplete,
}

/// `candidate_size` lower bound, in percent of the original (100%) window size.
pub const CANDIDATE_SIZE_MIN: u32 = 25;
/// `candidate_size` upper bound.
pub const CANDIDATE_SIZE_MAX: u32 = 300;
/// `candidate_size` default.
pub const CANDIDATE_SIZE_DEFAULT: u32 = 50;

/// The kind of value a setting takes, with its allowed values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    /// One of these text values; the first is the default.
    Choice(&'static [&'static str]),
    /// `true` / `false`.
    Bool,
    /// A list of candidate letters (`s_order`, `z_order`).
    List,
    /// A whole number of at least `min`.
    Integer {
        /// Smallest allowed value.
        min: i64,
    },
}

/// One setting of `[options]` or `[autocomplete]`: the definition the loader
/// validates against (allowed-value messages are generated from it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    /// The setting's name in its section.
    pub name: &'static str,
    /// What values it takes.
    pub kind: OptionKind,
    default: Option<DefaultValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultValue {
    Text(&'static str),
    Bool(bool),
    Int(i64),
}

impl OptionSpec {
    /// The built-in default, or `None` when absence means "keep the preset's
    /// map" (`s_order`, `z_order`).
    pub fn default_value(&self) -> Option<OptionValue> {
        self.default.map(|d| match d {
            DefaultValue::Text(s) => OptionValue::Text(s.to_string()),
            DefaultValue::Bool(b) => OptionValue::Bool(b),
            DefaultValue::Int(i) => OptionValue::Integer(i),
        })
    }

    /// The allowed text values (empty unless the kind is [`OptionKind::Choice`]).
    pub fn allowed_values(&self) -> &'static [&'static str] {
        match self.kind {
            OptionKind::Choice(values) => values,
            _ => &[],
        }
    }
}

const fn choice(name: &'static str, values: &'static [&'static str]) -> OptionSpec {
    OptionSpec {
        name,
        kind: OptionKind::Choice(values),
        default: Some(DefaultValue::Text(values[0])),
    }
}
const fn flag(name: &'static str, default: bool) -> OptionSpec {
    OptionSpec {
        name,
        kind: OptionKind::Bool,
        default: Some(DefaultValue::Bool(default)),
    }
}
const fn list(name: &'static str) -> OptionSpec {
    OptionSpec {
        name,
        kind: OptionKind::List,
        default: None,
    }
}

const OPTIONS: &[OptionSpec] = &[
    choice("gh", &["ghain", "qaf"]),
    choice("q", &["qaf", "ghain"]),
    choice("c", &["none", "k", "s"]),
    choice("kh", &["khe"]),
    choice("ch", &["che"]),
    list("s_order"),
    list("z_order"),
    choice("digits", &["persian", "ascii"]),
    choice("punctuation", &["persian", "none"]),
    choice("guillemets", &["none", "angle", "brace"]),
    choice("shift_letters", &["lowercase", "alternate"]),
    choice("space", &["preedit", "top-candidate"]),
    choice("enter", &["commit", "commit-and-pass"]),
    flag("number_select", true),
    flag("candidate_labels", false),
    choice("candidate_position", &["below", "above"]),
    choice(
        "toggle_hotkey",
        &[
            "none",
            "alt+`",
            "ctrl+`",
            "ctrl+space",
            "ctrl+shift+space",
            "ctrl+alt+space",
        ],
    ),
    OptionSpec {
        name: "candidate_size",
        kind: OptionKind::Integer { min: 1 },
        default: Some(DefaultValue::Int(CANDIDATE_SIZE_DEFAULT as i64)),
    },
    flag("allow_arabic_forms", false),
    flag("allow_sequences", false),
];

const AUTOCOMPLETE: &[OptionSpec] = &[
    flag("enabled", false),
    OptionSpec {
        name: "predict_min_chars",
        kind: OptionKind::Integer { min: 1 },
        default: Some(DefaultValue::Int(2)),
    },
    flag("learning", true),
];

/// Every `[options]` setting, in schema order.
pub fn option_specs() -> &'static [OptionSpec] {
    OPTIONS
}

/// Every `[autocomplete]` setting (phase 2), in schema order.
pub fn autocomplete_specs() -> &'static [OptionSpec] {
    AUTOCOMPLETE
}

const SECTIONS: &[&str] = &["options", "autocomplete", "map"];
const MAP_SECTIONS: &[(&str, usize)] = &[("singles", 1), ("digraphs", 2), ("trigraphs", 3)];

// ---------------------------------------------------------------------------
// ResolvedConfig

/// The immutable result of loading a configuration; shared with the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    preset: String,
    map: BTreeMap<String, Vec<String>>,
    /// Every non-empty prefix of every key (keys included): the derived starters.
    prefixes: BTreeSet<String>,
    options: Options,
    /// Effective value of every `[options]` / `autocomplete.*` setting.
    values: BTreeMap<String, OptionValue>,
    convert: BTreeMap<char, String>,
    max_key_len: usize,
}

/// A loaded configuration plus everything that was wrong with the user file.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// Always usable, whatever the file contained.
    pub config: ResolvedConfig,
    /// Problems in the user file, in file order.
    pub issues: Vec<ConfigIssue>,
}

impl Default for ResolvedConfig {
    /// The default preset with no user file.
    fn default() -> Self {
        load_config("").config
    }
}

impl ResolvedConfig {
    /// A built-in preset with no user changes, or `None` if `name` is unknown.
    pub fn builtin(name: &str) -> Option<Self> {
        preset_source(name)?;
        Some(load_config_with_base(name, "").config)
    }

    /// Name of the preset this configuration was built on.
    pub fn preset_name(&self) -> &str {
        &self.preset
    }

    /// Behaviour options.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Ordered candidates for a Latin key (index 0 is the default).
    pub fn candidates(&self, key: &str) -> Option<&[String]> {
        self.map.get(key).map(Vec::as_slice)
    }

    /// All map keys, sorted.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }

    /// Longest key length (1-3).
    pub fn max_key_len(&self) -> usize {
        self.max_key_len
    }

    /// True if `s` is a key or the start of one (a "digraph starter").
    pub fn is_key_prefix(&self, s: &str) -> bool {
        self.prefixes.contains(s)
    }

    /// What a typed non-letter character inserts (digits, punctuation,
    /// guillemets), or `None` when it inserts itself unchanged.
    pub fn convert_char(&self, c: char) -> Option<&str> {
        self.convert.get(&c).map(String::as_str)
    }

    /// The effective value of a setting by its file name: an `[options]` name
    /// (`"digits"`), `"autocomplete.<name>"`, or `"map.<section>.<key>"`.
    pub fn option_value(&self, name: &str) -> Option<OptionValue> {
        if let Some(rest) = name.strip_prefix("map.") {
            let (section, key) = rest.split_once('.')?;
            let len = MAP_SECTIONS.iter().find(|(s, _)| *s == section)?.1;
            if key.chars().count() != len {
                return None;
            }
            return self.map.get(key).map(|v| OptionValue::List(v.clone()));
        }
        if let Some(v) = self.values.get(name) {
            return Some(v.clone());
        }
        match name {
            "s_order" => self.map.get("s").map(|v| OptionValue::List(v.clone())),
            "z_order" => self.map.get("z").map(|v| OptionValue::List(v.clone())),
            _ => None,
        }
    }
}

/// Loads a user config file's text on top of the preset it names (default
/// [`DEFAULT_PRESET`]). Never fails; see [`LoadedConfig::issues`].
pub fn load_config(user_toml: &str) -> LoadedConfig {
    load_config_with_base(DEFAULT_PRESET, user_toml)
}

/// Like [`load_config`], but `base` is the preset used when the file has no
/// `preset` key (an unknown `base` falls back to [`DEFAULT_PRESET`]).
pub fn load_config_with_base(base: &str, user_toml: &str) -> LoadedConfig {
    let mut issues = Vec::new();
    let user = parse(user_toml, true, &mut issues).unwrap_or_default();

    let mut preset_name = if preset_source(base).is_some() {
        base
    } else {
        DEFAULT_PRESET
    };
    if let Some((name, loc)) = &user.preset {
        if preset_source(name).is_some() {
            preset_name = builtin_preset_names()
                .into_iter()
                .find(|n| n == name)
                .unwrap();
        } else {
            let names = builtin_preset_names();
            let hint = match suggest(name, names.iter().copied()) {
                Some(s) => format!("Did you mean \"{s}\"?"),
                None => String::new(),
            };
            issues.push(loc.issue(
                Severity::Error,
                IssueKind::UnknownPreset,
                format!(
                    "unknown preset \"{name}\". Available presets: {}. The default preset is in use.",
                    names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
                ),
                hint,
            ));
            preset_name = DEFAULT_PRESET;
        }
    }

    let config = resolve(preset_name, &user, &mut issues);
    issues.sort_by_key(|i| (i.line, i.column));
    LoadedConfig { config, issues }
}

/// Problems in a built-in preset itself (for lint tests); `None` if unknown.
pub fn builtin_preset_issues(name: &str) -> Option<Vec<ConfigIssue>> {
    let src = preset_source(name)?;
    let mut issues = Vec::new();
    let parsed = parse(src, false, &mut issues).unwrap_or_default();
    let mut ignored = Vec::new();
    resolve_with_preset(name, &parsed, &Parsed::default(), &mut ignored, &mut issues);
    Some(issues)
}

fn resolve(preset_name: &str, user: &Parsed, issues: &mut Vec<ConfigIssue>) -> ResolvedConfig {
    let src = preset_source(preset_name).unwrap_or(EMERGENCY_PRESET);
    // Problems inside a built-in preset are a build defect caught by lint
    // tests; the user cannot fix them, so they are not reported to them.
    let mut preset_issues = Vec::new();
    let preset = match parse(src, false, &mut preset_issues) {
        Some(p) => p,
        None => parse(EMERGENCY_PRESET, false, &mut preset_issues).unwrap_or_default(),
    };
    resolve_with_preset(preset_name, &preset, user, issues, &mut preset_issues)
}

fn resolve_with_preset(
    preset_name: &str,
    preset: &Parsed,
    user: &Parsed,
    issues: &mut Vec<ConfigIssue>,
    preset_issues: &mut Vec<ConfigIssue>,
) -> ResolvedConfig {
    // 1. Merge scalar options: preset, then user (both already type-checked).
    let mut set: BTreeMap<&'static str, OptionValue> = BTreeMap::new();
    for sv in preset.options.iter().chain(&user.options) {
        if !matches!(sv.value, OptionValue::List(_)) {
            set.insert(sv.name, sv.value.clone());
        }
    }
    let flag_on = |name: &str| matches!(set.get(name), Some(OptionValue::Bool(true)));
    let rules = CandidateRules {
        allow_sequences: flag_on("allow_sequences"),
        allow_arabic_forms: flag_on("allow_arabic_forms"),
    };

    // 2. List options (validated as candidate lists now that the flags are known).
    for (list_issues, options) in [
        (&mut *preset_issues, &preset.options),
        (&mut *issues, &user.options),
    ] {
        for sv in options {
            if let OptionValue::List(list) = &sv.value {
                if check_candidates(sv.name, list, &rules, &sv.loc, list_issues) {
                    set.insert(sv.name, sv.value.clone());
                }
            }
        }
    }

    // 3. Preset map.
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &preset.map {
        if check_map_entry(e, &rules, preset_issues) {
            map.insert(e.key.clone(), e.candidates.clone());
        }
    }

    // 4. Expand options into map patches, fixed order.
    let text = |name: &str| match set.get(name) {
        Some(OptionValue::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    if let Some(OptionValue::List(l)) = set.get("s_order") {
        map.insert("s".into(), l.clone());
    }
    if let Some(OptionValue::List(l)) = set.get("z_order") {
        map.insert("z".into(), l.clone());
    }
    let promote = |map: &mut BTreeMap<String, Vec<String>>, key: &str, letter: &str| {
        let list = map.entry(key.to_string()).or_default();
        list.retain(|c| c != letter);
        list.insert(0, letter.to_string());
    };
    match text("gh") {
        Some("qaf") => promote(&mut map, "gh", "ق"),
        Some("ghain") => promote(&mut map, "gh", "غ"),
        _ => {}
    }
    match text("q") {
        Some("qaf") => promote(&mut map, "q", "ق"),
        Some("ghain") => promote(&mut map, "q", "غ"),
        _ => {}
    }
    match text("c") {
        Some("none") => {
            map.remove("c");
        }
        Some("k") => {
            map.insert("c".into(), vec!["ک".into()]);
        }
        Some("s") => {
            let s = map.get("s").cloned().unwrap_or_else(|| vec!["س".into()]);
            map.insert("c".into(), s);
        }
        _ => {}
    }

    // 5. Raw user overrides, entry by entry.
    for e in &user.map {
        if check_map_entry(e, &rules, issues) {
            map.insert(e.key.clone(), e.candidates.clone());
        }
    }
    // 6. Reachability of 3-letter keys the user added (PLAN 3.3: warn only).
    for e in &user.map {
        if e.key.chars().count() == 3 && map.contains_key(&e.key) {
            let prefix: String = e.key.chars().take(2).collect();
            if !map.contains_key(&prefix) {
                issues.push(e.loc.issue(
                    Severity::Warning,
                    IssueKind::UnreachablePrefix,
                    format!(
                        "{} is applied, but its start \"{prefix}\" has no entry, so typing it \
                         step by step shows the letters separately first.",
                        e.key
                    ),
                    String::new(),
                ));
            }
        }
    }

    // 7. Freeze.
    let mut values = BTreeMap::new();
    for spec in OPTIONS {
        if let Some(v) = set.get(spec.name).cloned().or_else(|| spec.default_value()) {
            values.insert(spec.name.to_string(), v);
        }
    }
    let mut ac: BTreeMap<&str, OptionValue> = BTreeMap::new();
    for sv in preset.autocomplete.iter().chain(&user.autocomplete) {
        ac.insert(sv.name, sv.value.clone());
    }
    for spec in AUTOCOMPLETE {
        if let Some(v) = ac.get(spec.name).cloned().or_else(|| spec.default_value()) {
            values.insert(format!("autocomplete.{}", spec.name), v);
        }
    }
    let opt_text = |name: &str| match values.get(name) {
        Some(OptionValue::Text(t)) => t.as_str(),
        _ => "",
    };
    let opt_bool = |name: &str| matches!(values.get(name), Some(OptionValue::Bool(true)));
    let opt_int = |name: &str, d: i64| match values.get(name) {
        Some(OptionValue::Integer(i)) => *i,
        _ => d,
    };
    let options = Options {
        digits: if opt_text("digits") == "ascii" {
            Digits::Ascii
        } else {
            Digits::Persian
        },
        punctuation: if opt_text("punctuation") == "none" {
            Punctuation::None
        } else {
            Punctuation::Persian
        },
        guillemets: match opt_text("guillemets") {
            "angle" => Guillemets::Angle,
            "brace" => Guillemets::Brace,
            _ => Guillemets::None,
        },
        shift_letters: if opt_text("shift_letters") == "alternate" {
            ShiftLetters::Alternate
        } else {
            ShiftLetters::Lowercase
        },
        space: if opt_text("space") == "top-candidate" {
            SpaceMode::TopCandidate
        } else {
            SpaceMode::Preedit
        },
        enter: if opt_text("enter") == "commit-and-pass" {
            EnterMode::CommitAndPass
        } else {
            EnterMode::Commit
        },
        number_select: opt_bool("number_select"),
        candidate_labels: opt_bool("candidate_labels"),
        candidate_position: if opt_text("candidate_position") == "above" {
            CandidatePosition::Above
        } else {
            CandidatePosition::Below
        },
        toggle_hotkey: match opt_text("toggle_hotkey") {
            "alt+`" => ToggleHotkey::AltBackquote,
            "ctrl+`" => ToggleHotkey::CtrlBackquote,
            "ctrl+space" => ToggleHotkey::CtrlSpace,
            "ctrl+shift+space" => ToggleHotkey::CtrlShiftSpace,
            "ctrl+alt+space" => ToggleHotkey::CtrlAltSpace,
            _ => ToggleHotkey::None,
        },
        candidate_size: opt_int("candidate_size", CANDIDATE_SIZE_DEFAULT as i64)
            .clamp(CANDIDATE_SIZE_MIN as i64, CANDIDATE_SIZE_MAX as i64)
            as u32,
        allow_arabic_forms: rules.allow_arabic_forms,
        allow_sequences: rules.allow_sequences,
        autocomplete: Autocomplete {
            enabled: opt_bool("autocomplete.enabled"),
            predict_min_chars: opt_int("autocomplete.predict_min_chars", 2),
            learning: opt_bool("autocomplete.learning"),
        },
    };

    let mut prefixes = BTreeSet::new();
    let mut max_key_len = 1;
    for key in map.keys() {
        let chars: Vec<char> = key.chars().collect();
        max_key_len = max_key_len.max(chars.len());
        for n in 1..=chars.len() {
            prefixes.insert(chars[..n].iter().collect::<String>());
        }
    }
    ResolvedConfig {
        preset: preset_name.to_string(),
        map,
        prefixes,
        convert: punctuation::table(&options),
        options,
        values,
        max_key_len,
    }
}

// ---------------------------------------------------------------------------
// Reading a file into entries (type checks only; content checks come later)

#[derive(Debug, Clone, Default)]
struct Loc {
    line: usize,
    column: usize,
    snippet: String,
}

impl Loc {
    fn at(src: &str, offset: Option<usize>) -> Loc {
        match offset {
            Some(o) => {
                let (line, column) = line_col(src, o);
                Loc {
                    line,
                    column,
                    snippet: printable(line_text(src, line).trim()),
                }
            }
            None => Loc::default(),
        }
    }

    fn issue(
        &self,
        severity: Severity,
        kind: IssueKind,
        message: String,
        hint: String,
    ) -> ConfigIssue {
        ConfigIssue {
            severity,
            kind,
            line: self.line,
            column: self.column,
            snippet: self.snippet.clone(),
            message,
            hint,
        }
    }
}

#[derive(Debug, Clone)]
struct SetValue {
    name: &'static str,
    value: OptionValue,
    loc: Loc,
}

#[derive(Debug, Clone)]
struct MapEntry {
    section: &'static str,
    section_len: usize,
    key: String,
    candidates: Vec<String>,
    loc: Loc,
}

#[derive(Debug, Clone, Default)]
struct Parsed {
    preset: Option<(String, Loc)>,
    options: Vec<SetValue>,
    autocomplete: Vec<SetValue>,
    map: Vec<MapEntry>,
}

fn parse(src: &str, is_user: bool, issues: &mut Vec<ConfigIssue>) -> Option<Parsed> {
    let doc = match ImDocument::parse(src) {
        Ok(doc) => doc,
        Err(err) => {
            let (line, column, reason) = syntax_reason(src, &err);
            issues.push(ConfigIssue {
                severity: Severity::Error,
                kind: IssueKind::Syntax,
                line,
                column,
                snippet: printable(line_text(src, line).trim()),
                message: format!("this line couldn't be read ({reason})."),
                hint: "Fix the line and save the file; the change is picked up automatically."
                    .into(),
            });
            return None;
        }
    };
    let mut out = Parsed::default();
    let root = doc.as_table();
    for (key, item) in root.iter() {
        let loc = locate(src, root, key, item);
        match key {
            "preset" if is_user => match item.as_str() {
                Some(name) => out.preset = Some((name.to_string(), loc)),
                None => issues.push(wrong_type(&loc, "preset", "text", "\"qamooscheh\"", item)),
            },
            "options" | "autocomplete" => {
                let specs = if key == "options" {
                    OPTIONS
                } else {
                    AUTOCOMPLETE
                };
                let Some(table) = item.as_table_like() else {
                    issues.push(not_a_section(&loc, key));
                    continue;
                };
                let dest = if key == "options" {
                    &mut out.options
                } else {
                    &mut out.autocomplete
                };
                parse_settings(src, table, specs, dest, issues);
            }
            "map" => {
                let Some(table) = item.as_table_like() else {
                    issues.push(not_a_section(&loc, key));
                    continue;
                };
                parse_map(src, table, &mut out.map, issues);
            }
            _ => issues.push(unknown_top_level(&loc, key, item, is_user)),
        }
    }
    Some(out)
}

fn locate(src: &str, table: &dyn TableLike, key: &str, item: &Item) -> Loc {
    let offset = table
        .get_key_value(key)
        .and_then(|(k, _)| k.span())
        .or_else(|| item.span())
        .map(|s| s.start);
    Loc::at(src, offset)
}

fn unknown_top_level(loc: &Loc, key: &str, item: &Item, is_user: bool) -> ConfigIssue {
    if item.is_table_like() && !item.is_inline_table() {
        let hint = suggest(key, SECTIONS.iter().copied())
            .map(|s| format!("Did you mean \"{s}\"?"))
            .unwrap_or_default();
        return loc.issue(
            Severity::Error,
            IssueKind::UnknownSection,
            format!("unknown section \"{key}\"."),
            hint,
        );
    }
    let hint = if OPTIONS.iter().any(|s| s.name == key) {
        "It belongs below the [options] line.".to_string()
    } else {
        let mut known: Vec<&str> = OPTIONS.iter().map(|s| s.name).collect();
        known.extend(SECTIONS);
        if is_user {
            known.push("preset");
        }
        suggest(key, known)
            .map(|s| format!("Did you mean \"{s}\"?"))
            .unwrap_or_default()
    };
    loc.issue(
        Severity::Error,
        IssueKind::UnknownKey,
        format!("unknown setting \"{key}\"."),
        hint,
    )
}

fn not_a_section(loc: &Loc, key: &str) -> ConfigIssue {
    loc.issue(
        Severity::Error,
        IssueKind::WrongType,
        format!("{key} must be a section of settings, written as a line [{key}] followed by its settings."),
        String::new(),
    )
}

fn found_name(item: &Item) -> &'static str {
    match item.as_value() {
        Some(Value::String(_)) => "text",
        Some(Value::Integer(_)) => "a number",
        Some(Value::Float(_)) => "a decimal number",
        Some(Value::Boolean(_)) => "true or false",
        Some(Value::Datetime(_)) => "a date",
        Some(Value::Array(_)) => "a list",
        Some(Value::InlineTable(_)) | None => "a group of settings",
    }
}

fn wrong_type(loc: &Loc, key: &str, expected: &str, example: &str, item: &Item) -> ConfigIssue {
    loc.issue(
        Severity::Error,
        IssueKind::WrongType,
        format!(
            "{key} must be {expected} (for example {example}), not {}.",
            found_name(item)
        ),
        String::new(),
    )
}

fn parse_settings(
    src: &str,
    table: &dyn TableLike,
    specs: &'static [OptionSpec],
    dest: &mut Vec<SetValue>,
    issues: &mut Vec<ConfigIssue>,
) {
    for (key, item) in table.iter() {
        let loc = locate(src, table, key, item);
        let Some(spec) = specs.iter().find(|s| s.name == key) else {
            let hint = suggest(key, specs.iter().map(|s| s.name))
                .map(|s| format!("Did you mean \"{s}\"?"))
                .unwrap_or_default();
            issues.push(loc.issue(
                Severity::Error,
                IssueKind::UnknownKey,
                format!("unknown setting \"{key}\"."),
                hint,
            ));
            continue;
        };
        let value = match spec.kind {
            OptionKind::Choice(allowed) => match item.as_str() {
                Some(s) if allowed.contains(&s) => Some(OptionValue::Text(s.to_string())),
                Some(s) => {
                    issues.push(loc.issue(
                        Severity::Error,
                        IssueKind::InvalidValue,
                        format!("\"{s}\" isn't one of the choices for {key}."),
                        format!("Use {}.", or_list(allowed)),
                    ));
                    None
                }
                None => {
                    let mut issue =
                        wrong_type(&loc, key, "text", &format!("\"{}\"", allowed[0]), item);
                    issue.hint = format!("Use {}.", or_list(allowed));
                    issues.push(issue);
                    None
                }
            },
            OptionKind::Bool => match item.as_bool() {
                Some(b) => Some(OptionValue::Bool(b)),
                None => {
                    issues.push(wrong_type(&loc, key, "true or false", "true", item));
                    None
                }
            },
            OptionKind::Integer { min } => match item.as_integer() {
                Some(i) if i >= min => Some(OptionValue::Integer(i)),
                Some(i) => {
                    issues.push(loc.issue(
                        Severity::Error,
                        IssueKind::InvalidValue,
                        format!("{key} must be at least {min}, not {i}."),
                        String::new(),
                    ));
                    None
                }
                None => {
                    issues.push(wrong_type(&loc, key, "a whole number", "2", item));
                    None
                }
            },
            OptionKind::List => match string_list(item) {
                Some(list) => Some(OptionValue::List(list)),
                None => {
                    issues.push(wrong_type(
                        &loc,
                        key,
                        "a list",
                        "[\"س\", \"ص\", \"ث\"]",
                        item,
                    ));
                    None
                }
            },
        };
        if let Some(value) = value {
            dest.push(SetValue {
                name: spec.name,
                value,
                loc,
            });
        }
    }
}

fn string_list(item: &Item) -> Option<Vec<String>> {
    let arr = item.as_array()?;
    arr.iter().map(|v| v.as_str().map(str::to_string)).collect()
}

fn parse_map(
    src: &str,
    table: &dyn TableLike,
    dest: &mut Vec<MapEntry>,
    issues: &mut Vec<ConfigIssue>,
) {
    for (name, item) in table.iter() {
        let loc = locate(src, table, name, item);
        let Some(&(section, section_len)) = MAP_SECTIONS.iter().find(|(s, _)| *s == name) else {
            let hint = suggest(name, MAP_SECTIONS.iter().map(|(s, _)| *s))
                .map(|s| format!("Did you mean \"map.{s}\"?"))
                .unwrap_or_else(|| {
                    "The map sections are [map.singles], [map.digraphs] and [map.trigraphs].".into()
                });
            issues.push(loc.issue(
                Severity::Error,
                IssueKind::UnknownSection,
                format!("unknown section \"map.{name}\"."),
                hint,
            ));
            continue;
        };
        let Some(entries) = item.as_table_like() else {
            issues.push(not_a_section(&loc, &format!("map.{name}")));
            continue;
        };
        for (key, item) in entries.iter() {
            let loc = locate(src, entries, key, item);
            match string_list(item) {
                Some(candidates) => dest.push(MapEntry {
                    section,
                    section_len,
                    key: key.to_string(),
                    candidates,
                    loc,
                }),
                None => {
                    let example = match item.as_str() {
                        Some(s) => format!("[\"{s}\"]"),
                        None => "[\"ب\"]".to_string(),
                    };
                    let mut issue = wrong_type(&loc, key, "a list", &example, item);
                    issue.kind = IssueKind::BadCandidate;
                    issues.push(issue);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Content validation

struct CandidateRules {
    allow_sequences: bool,
    allow_arabic_forms: bool,
}

fn is_allowed_code_point(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
        || c == ZWNJ
}

/// Arabic-only forms that Persian text should not use, with their Persian
/// replacements (if any).
fn arabic_form(c: char) -> Option<(&'static str, &'static str)> {
    match c {
        '\u{064A}' => Some(("U+064A", "the Persian one (\"ی\" U+06CC)")),
        '\u{0643}' => Some(("U+0643", "the Persian one (\"ک\" U+06A9)")),
        '\u{0629}' => Some((
            "U+0629",
            "a Persian letter (usually \"ه\" U+0647 or \"ت\" U+062A)",
        )),
        _ => None,
    }
}

/// Canonical compositions inside the Arabic block: a base plus a combining
/// mark that NFC writes as one letter.
fn composed(a: char, b: char) -> Option<char> {
    Some(match (a, b) {
        ('\u{0627}', '\u{0653}') => '\u{0622}',
        ('\u{0627}', '\u{0654}') => '\u{0623}',
        ('\u{0648}', '\u{0654}') => '\u{0624}',
        ('\u{0627}', '\u{0655}') => '\u{0625}',
        ('\u{064A}', '\u{0654}') => '\u{0626}',
        ('\u{06D5}', '\u{0654}') => '\u{06C0}',
        ('\u{06C1}', '\u{0654}') => '\u{06C2}',
        ('\u{06D2}', '\u{0654}') => '\u{06D3}',
        _ => return None,
    })
}

/// Validates a candidate list; pushes issues and returns whether to apply it.
fn check_candidates(
    label: &str,
    list: &[String],
    rules: &CandidateRules,
    loc: &Loc,
    issues: &mut Vec<ConfigIssue>,
) -> bool {
    let error =
        |kind, message: String, hint: String| loc.issue(Severity::Error, kind, message, hint);
    if list.is_empty() {
        issues.push(error(
            IssueKind::EmptyCandidates,
            format!("{label} needs at least one candidate letter."),
            "Write it like [\"ب\"].".into(),
        ));
        return false;
    }
    for (i, cand) in list.iter().enumerate() {
        let chars: Vec<char> = cand.chars().collect();
        if chars.is_empty() {
            issues.push(error(
                IssueKind::BadCandidate,
                format!("{label} has an empty candidate \"\"."),
                "Each candidate must be one Persian letter.".into(),
            ));
            return false;
        }
        if let Some(&bad) = chars.iter().find(|c| !is_allowed_code_point(**c)) {
            let what = if chars.len() == 1 || bad.is_control() {
                format!("\"{}\" isn't a Persian letter.", printable(cand))
            } else {
                format!(
                    "\"{}\" contains \"{}\", which isn't a Persian letter.",
                    printable(cand),
                    printable(&bad.to_string())
                )
            };
            issues.push(error(
                IssueKind::BadCandidate,
                what,
                "Candidates must be Persian letters (or the zero-width non-joiner).".into(),
            ));
            return false;
        }
        if chars.len() > 1 && !rules.allow_sequences {
            issues.push(error(
                IssueKind::BadCandidate,
                format!(
                    "\"{cand}\" is more than one character; each candidate must be one character \
                     (set allow_sequences = true to allow longer ones)."
                ),
                single_form_hint(&chars),
            ));
            return false;
        }
        if let Some(c) = chars.windows(2).find_map(|w| composed(w[0], w[1])) {
            issues.push(error(
                IssueKind::BadCandidate,
                format!("\"{cand}\" should use the single letter \"{c}\" instead of a letter plus a mark."),
                String::new(),
            ));
            return false;
        }
        if list[..i].contains(cand) {
            issues.push(error(
                IssueKind::DuplicateCandidates,
                format!("{label} lists \"{cand}\" twice."),
                "Each letter may appear only once in a list.".into(),
            ));
            return false;
        }
    }
    if !rules.allow_arabic_forms {
        for cand in list {
            if let Some((code, persian)) = cand.chars().find_map(arabic_form) {
                issues.push(loc.issue(
                    Severity::Warning,
                    IssueKind::ArabicForm,
                    format!(
                        "{label} uses \"{cand}\" ({code}), the Arabic form, not {persian}. It was applied."
                    ),
                    "Set allow_arabic_forms = true to hide this warning.".into(),
                ));
            }
        }
    }
    true
}

fn single_form_hint(chars: &[char]) -> String {
    match chars {
        [a, b] => composed(*a, *b)
            .map(|c| format!("Use the single letter \"{c}\" instead."))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Shows control characters as U+XXXX so messages stay readable.
fn printable(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() {
                format!("U+{:04X}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

fn check_map_entry(e: &MapEntry, rules: &CandidateRules, issues: &mut Vec<ConfigIssue>) -> bool {
    let len = e.key.chars().count();
    let ascii_ok = e.key.chars().all(|c| ('\u{21}'..='\u{7E}').contains(&c));
    if len == 0 || len > 3 || !ascii_ok {
        issues.push(e.loc.issue(
            Severity::Error,
            IssueKind::BadMapKey,
            format!(
                "\"{}\" can't be used as a key here: keys are 1 to 3 plain ASCII characters \
                 (English letters, digits or punctuation).",
                printable(&e.key)
            ),
            String::new(),
        ));
        return false;
    }
    if len != e.section_len {
        let right = MAP_SECTIONS
            .iter()
            .find(|(_, l)| *l == len)
            .map(|(s, _)| *s)
            .unwrap();
        let chars = if e.section_len == 1 {
            "character"
        } else {
            "characters"
        };
        issues.push(e.loc.issue(
            Severity::Error,
            IssueKind::BadMapKey,
            format!(
                "a key in [map.{}] has exactly {} {chars}; \"{}\" belongs in [map.{right}].",
                e.section, e.section_len, e.key
            ),
            String::new(),
        ));
        return false;
    }
    if e.key.chars().any(|c| c.is_ascii_uppercase()) {
        issues.push(e.loc.issue(
            Severity::Error,
            IssueKind::BadMapKey,
            format!(
                "\"{}\" can't be used as a key: keys use lowercase letters.",
                e.key
            ),
            format!(
                "Write \"{}\"; Shift+letter is controlled by shift_letters.",
                e.key.to_ascii_lowercase()
            ),
        ));
        return false;
    }
    check_candidates(&e.key, &e.candidates, rules, &e.loc, issues)
}

#[cfg(test)]
mod tests;
