//! Digit and punctuation conversion tables, from `data/punctuation.toml`
//! (embedded at build time) with a compiled-in fallback of the same content.

use super::{Digits, Guillemets, Options, Punctuation, DATA_PUNCTUATION};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use toml_edit::ImDocument;

type Tables = BTreeMap<String, BTreeMap<char, String>>;

const FALLBACK: &[(&str, &[(char, &str)])] = &[
    (
        "digits.persian",
        &[
            ('0', "\u{06F0}"),
            ('1', "\u{06F1}"),
            ('2', "\u{06F2}"),
            ('3', "\u{06F3}"),
            ('4', "\u{06F4}"),
            ('5', "\u{06F5}"),
            ('6', "\u{06F6}"),
            ('7', "\u{06F7}"),
            ('8', "\u{06F8}"),
            ('9', "\u{06F9}"),
        ],
    ),
    ("digits.ascii", &[]),
    (
        "punctuation.persian",
        &[
            (',', "\u{060C}"),
            (';', "\u{061B}"),
            ('?', "\u{061F}"),
            ('%', "\u{066A}"),
        ],
    ),
    ("punctuation.none", &[]),
    ("guillemets.angle", &[('<', "\u{00AB}"), ('>', "\u{00BB}")]),
    ("guillemets.brace", &[('{', "\u{00AB}"), ('}', "\u{00BB}")]),
];

fn fallback() -> Tables {
    FALLBACK
        .iter()
        .map(|(name, pairs)| {
            (
                name.to_string(),
                pairs.iter().map(|(k, v)| (*k, v.to_string())).collect(),
            )
        })
        .collect()
}

/// Parses the punctuation data file: `[group.variant]` tables of
/// single-character keys to text values. `None` if it is not usable.
fn parse(src: &str) -> Option<Tables> {
    let doc = ImDocument::parse(src).ok()?;
    let mut out = Tables::new();
    for group in ["digits", "punctuation", "guillemets"] {
        let variants = doc.as_table().get(group)?.as_table_like()?;
        for (variant, table) in variants.iter() {
            let table = table.as_table_like()?;
            let mut entries = BTreeMap::new();
            for (key, value) in table.iter() {
                let mut chars = key.chars();
                let (Some(k), None) = (chars.next(), chars.next()) else {
                    return None;
                };
                entries.insert(k, value.as_str()?.to_string());
            }
            out.insert(format!("{group}.{variant}"), entries);
        }
    }
    Some(out)
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| DATA_PUNCTUATION.and_then(parse).unwrap_or_else(fallback))
}

/// The conversion table for a set of options: typed char -> inserted text.
/// Characters not in the table insert themselves.
pub(super) fn table(options: &Options) -> BTreeMap<char, String> {
    let t = tables();
    let mut out = BTreeMap::new();
    let digits = match options.digits {
        Digits::Persian => "digits.persian",
        Digits::Ascii => "digits.ascii",
    };
    let punct = match options.punctuation {
        Punctuation::Persian => Some("punctuation.persian"),
        Punctuation::None => None,
    };
    let guill = match options.guillemets {
        Guillemets::None => None,
        Guillemets::Angle => Some("guillemets.angle"),
        Guillemets::Brace => Some("guillemets.brace"),
    };
    for name in [Some(digits), punct, guill].into_iter().flatten() {
        if let Some(entries) = t.get(name) {
            out.extend(
                entries
                    .iter()
                    .filter(|(k, v)| v.chars().ne([**k]))
                    .map(|(k, v)| (*k, v.clone())),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_file_matches_fallback() {
        let Some(src) = DATA_PUNCTUATION else { return };
        let parsed = parse(src).expect("data/punctuation.toml should parse");
        let fb = fallback();
        for (name, entries) in &fb {
            let data = parsed
                .get(name)
                .unwrap_or_else(|| panic!("missing [{name}]"));
            // digits.ascii is an identity table in the data file; the fallback leaves it empty.
            for (k, v) in entries {
                assert_eq!(data.get(k), Some(v), "[{name}] {k}");
            }
            for (k, v) in data {
                if v.chars().ne([*k]) {
                    assert_eq!(entries.get(k), Some(v), "[{name}] {k} only in data");
                }
            }
        }
    }

    #[test]
    fn rejects_malformed_tables() {
        assert!(parse(
            "[digits.persian]\n\"12\" = \"x\"\n[punctuation.persian]\n[guillemets.angle]\n"
        )
        .is_none());
        assert!(parse("not toml at all [").is_none());
    }
}
