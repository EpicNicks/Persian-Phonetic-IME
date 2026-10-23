//! The `[options]` rows of the settings form.
//!
//! The allowed values come from `pime_core::option_specs()`, which is the same
//! table the loader validates against. This module adds only the form order, the row
//! labels and a display label for each value. A value without a label is shown by
//! its raw name, so a value added in `pime-core` still shows up on the form.
//!
//! Labels put U+200E LEFT-TO-RIGHT MARK after each Persian letter so the letters
//! keep their order inside the left-to-right label. This is for display only; the
//! IME itself never emits direction marks.

use pime_core::{option_specs, OptionKind, OptionSpec, OptionValue, ResolvedConfig};

/// One row of the settings form.
#[derive(Debug, Clone, Copy)]
pub struct FormOption {
    /// Name in `[options]` (an `option_specs()` name).
    pub key: &'static str,
    /// Row label.
    pub label: &'static str,
    /// Display label per value: the choice name, or `"true"` / `"false"`.
    pub value_labels: &'static [(&'static str, &'static str)],
}

const ON_OFF: &[(&str, &str)] = &[("true", "On"), ("false", "Off")];

/// The main options, in form order.
pub const FORM_OPTIONS: &[FormOption] = &[
    FormOption {
        key: "gh",
        label: "gh types",
        value_labels: &[
            ("ghain", "ghain غ\u{200E} (alternative: qaf ق\u{200E})"),
            ("qaf", "qaf ق\u{200E} (alternative: ghain غ\u{200E})"),
        ],
    },
    FormOption {
        key: "q",
        label: "q types",
        value_labels: &[
            ("qaf", "qaf ق\u{200E} (alternative: ghain غ\u{200E})"),
            ("ghain", "ghain غ\u{200E} (alternative: qaf ق\u{200E})"),
        ],
    },
    FormOption {
        key: "c",
        label: "c on its own types",
        value_labels: &[
            ("none", "Nothing yet (waits for the h of ch)"),
            ("k", "kaf ک\u{200E}"),
            ("s", "sin س\u{200E} (alternatives: ص\u{200E} ث\u{200E})"),
        ],
    },
    FormOption {
        key: "digits",
        label: "Digits",
        value_labels: &[("persian", "Persian ۰۱۲۳"), ("ascii", "Latin 0123")],
    },
    FormOption {
        key: "punctuation",
        label: "Punctuation",
        value_labels: &[
            ("persian", "Persian ،\u{200E} ؛\u{200E} ؟\u{200E} ٪\u{200E}"),
            ("none", "As typed , ; ? %"),
        ],
    },
    FormOption {
        key: "guillemets",
        label: "« » quotes",
        value_labels: &[
            ("none", "Off"),
            ("angle", "< and > type « »"),
            ("brace", "{ and } type « »"),
        ],
    },
    FormOption {
        key: "shift_letters",
        label: "Shift + letter",
        value_labels: &[
            ("lowercase", "Same as without Shift"),
            ("alternate", "Second choice (Shift+a = آ)"),
        ],
    },
    FormOption {
        key: "space",
        label: "Space commits",
        value_labels: &[
            ("preedit", "What is shown"),
            ("top-candidate", "The first suggestion"),
        ],
    },
    FormOption {
        key: "enter",
        label: "Enter",
        value_labels: &[
            ("commit", "Finishes the word"),
            ("commit-and-pass", "Finishes the word and sends Enter"),
        ],
    },
    FormOption {
        key: "number_select",
        label: "1-9 pick from the list",
        value_labels: ON_OFF,
    },
    FormOption {
        key: "candidate_labels",
        label: "Letter names in the list",
        value_labels: ON_OFF,
    },
    FormOption {
        key: "toggle_hotkey",
        label: "Switch Persian / Latin with",
        value_labels: &[
            ("none", "No shortcut"),
            ("alt+`", "Alt + ` (above Tab)"),
            ("ctrl+`", "Ctrl + `"),
            ("ctrl+space", "Ctrl + Space"),
            ("ctrl+shift+space", "Ctrl + Shift + Space"),
            ("ctrl+alt+space", "Ctrl + Alt + Space"),
        ],
    },
    FormOption {
        key: "candidate_position",
        label: "List of alternatives opens",
        value_labels: &[("below", "Below the word"), ("above", "Above the word")],
    },
    FormOption {
        key: "theme",
        label: "Settings window look",
        value_labels: &[
            ("system", "Follow Windows"),
            ("light", "Light"),
            ("dark", "Dark"),
        ],
    },
];

fn value_name(v: &OptionValue) -> String {
    match v {
        OptionValue::Text(s) => s.clone(),
        OptionValue::Bool(b) => b.to_string(),
        OptionValue::Integer(i) => i.to_string(),
        OptionValue::List(l) => l.join(" "),
    }
}

impl FormOption {
    /// The `pime-core` spec of this option (`None` only if core dropped it).
    pub fn spec(&self) -> Option<&'static OptionSpec> {
        option_specs().iter().find(|s| s.name == self.key)
    }

    /// The values offered, in `pime-core`'s order (Bool: true, false).
    pub fn values(&self) -> Vec<OptionValue> {
        match self.spec().map(|s| s.kind) {
            Some(OptionKind::Choice(values)) => values
                .iter()
                .map(|v| OptionValue::Text(v.to_string()))
                .collect(),
            Some(OptionKind::Bool) => vec![OptionValue::Bool(true), OptionValue::Bool(false)],
            _ => Vec::new(),
        }
    }

    /// The setting value of choice `index`.
    pub fn value(&self, index: usize) -> Option<OptionValue> {
        self.values().into_iter().nth(index)
    }

    /// Display label of choice `index` (the raw value name if it has no label).
    pub fn label_of(&self, index: usize) -> Option<String> {
        let name = value_name(&self.value(index)?);
        Some(
            self.value_labels
                .iter()
                .find(|(v, _)| *v == name)
                .map_or(name, |(_, l)| l.to_string()),
        )
    }

    /// Number of choices.
    pub fn len(&self) -> usize {
        self.values().len()
    }

    /// True if `pime-core` offers no choices for this key (never for the form keys).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Index of the effective value in `cfg`, if it is one of the choices.
    pub fn current(&self, cfg: &ResolvedConfig) -> Option<usize> {
        let v = cfg.option_value(self.key)?;
        self.values().iter().position(|c| *c == v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{check_value, Section};
    use pime_core::load_config;

    #[test]
    fn every_form_key_is_a_core_choice_or_flag_with_full_labels() {
        for o in FORM_OPTIONS {
            let spec = o
                .spec()
                .unwrap_or_else(|| panic!("{} not in option_specs()", o.key));
            assert!(
                matches!(spec.kind, OptionKind::Choice(_) | OptionKind::Bool),
                "{}",
                o.key
            );
            assert_eq!(o.value_labels.len(), o.len(), "{}: label count", o.key);
            for i in 0..o.len() {
                let name = value_name(&o.value(i).unwrap());
                assert!(
                    o.value_labels.iter().any(|(v, _)| *v == name),
                    "{}: no label for {name}",
                    o.key
                );
            }
        }
    }

    #[test]
    fn every_choice_is_accepted_by_core() {
        for o in FORM_OPTIONS {
            for v in o.values() {
                assert_eq!(
                    check_value(Section::Options, o.key, &v),
                    None,
                    "{} {v:?}",
                    o.key
                );
            }
        }
    }

    #[test]
    fn current_and_labels() {
        let cfg = load_config("[options]\nc = \"s\"\nnumber_select = false\n").config;
        let find = |k| FORM_OPTIONS.iter().find(|o| o.key == k).unwrap();
        assert_eq!(find("c").current(&cfg), Some(2));
        assert_eq!(find("number_select").current(&cfg), Some(1));
        assert_eq!(find("gh").current(&cfg), Some(0));
        assert_eq!(find("gh").value(9), None);
        assert_eq!(find("number_select").label_of(1).as_deref(), Some("Off"));
        assert_eq!(find("digits").label_of(1).as_deref(), Some("Latin 0123"));
    }
}
