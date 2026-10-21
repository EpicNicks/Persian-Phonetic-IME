//! Runs `data/golden/dict.json` (format: docs/autocomplete.md section 8). Dictionaries are
//! built in memory with `build_from_tsv`; history is replayed with `History::record`.
//! Reports every failing case at once.

mod common;

use common::{build_bytes, build_dict, default_build_options, parse_json, Json};
use pime_core::{LearnEvent, LearnKind};
use pime_dict::{BuildOptions, BuildReport, Day, Dictionary, History, RankCtx, ScoreParams};
use std::path::PathBuf;

fn golden_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/golden/dict.json")
}

fn tsv_inputs(case: &Json) -> Result<Vec<String>, String> {
    match case.get("tsv") {
        Some(Json::Str(s)) => Ok(vec![s.clone()]),
        Some(v @ Json::Arr(_)) => v.strings().ok_or_else(|| "bad tsv array".to_string()),
        _ => Err("missing tsv".into()),
    }
}

fn options(case: &Json) -> Result<BuildOptions, String> {
    let mut o = default_build_options();
    if let Some(opts) = case.get("options").and_then(Json::obj) {
        for (k, v) in opts {
            let n = v
                .num()
                .ok_or_else(|| format!("option {k} is not a number"))?;
            match k.as_str() {
                "min_zipf" => o.min_zipf = n as f32,
                "max_entries" => o.max_entries = n as usize,
                "max_bytes" => o.max_bytes = n as usize,
                _ => return Err(format!("unknown option {k}")),
            }
        }
    }
    Ok(o)
}

fn kind(name: &str) -> Result<LearnKind, String> {
    Ok(match name {
        "accepted" => LearnKind::Accepted,
        "skipped" => LearnKind::Skipped,
        "ignored" => LearnKind::Ignored,
        "dismissed" => LearnKind::Dismissed,
        _ => return Err(format!("unknown event {name:?}")),
    })
}

fn replay(case: &Json, params: &ScoreParams) -> Result<History, String> {
    let mut h = History::new();
    for item in case.get("history").and_then(Json::arr).unwrap_or(&[]) {
        let word = item.get("word").and_then(Json::str).ok_or("history.word")?;
        let day = item.get("day").and_then(Json::num).ok_or("history.day")? as u32;
        let events = item
            .get("events")
            .and_then(Json::strings)
            .ok_or("history.events")?;
        for ev in events {
            let ev = LearnEvent {
                word: word.to_string(),
                kind: kind(&ev)?,
            };
            h.record(&ev, Day(day), params);
        }
    }
    Ok(h)
}

fn run_complete(case: &Json) -> Result<(), String> {
    let tsvs = tsv_inputs(case)?;
    let refs: Vec<&str> = tsvs.iter().map(String::as_str).collect();
    let (dict, _) = build_dict(&refs, &options(case)?)?;
    let params = ScoreParams::default();
    let history = replay(case, &params)?;
    let use_history = case.get("use_history").and_then(Json::bool).unwrap_or(true);
    let today = case.get("today").and_then(Json::num).unwrap_or(20000.0) as u32;
    let prefix = case
        .get("prefix")
        .and_then(Json::str)
        .ok_or("missing prefix")?;
    let limit = case
        .get("limit")
        .and_then(Json::num)
        .ok_or("missing limit")? as usize;
    let want = case
        .get("expect")
        .and_then(Json::strings)
        .ok_or("missing expect")?;
    let rank = RankCtx {
        history: use_history.then_some(&history),
        today: Day(today),
        params: &params,
    };
    let got: Vec<String> = dict
        .complete(prefix, limit, &rank)
        .into_iter()
        .map(|c| c.text)
        .collect();
    let mut errs = Vec::new();
    if got != want {
        errs.push(format!("expected {want:?}, got {got:?}"));
    }
    // Trait-level invariants (docs/autocomplete.md 2.2), independent of the expectation.
    if got.len() > limit {
        errs.push(format!("{} results exceed limit {limit}", got.len()));
    }
    for (i, t) in got.iter().enumerate() {
        if !t.starts_with(prefix) || t == prefix {
            errs.push(format!("{t:?} does not extend the prefix"));
        }
        if got[..i].contains(t) {
            errs.push(format!("{t:?} duplicated"));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

const FIRST_CHARS: &[std::ops::RangeInclusive<u32>] = &[
    0x0621..=0x063A,
    0x0641..=0x0648,
    0x067E..=0x067E,
    0x0686..=0x0686,
    0x0698..=0x0698,
    0x06A9..=0x06A9,
    0x06AF..=0x06AF,
    0x06CC..=0x06CC,
];

/// Every display form, recovered through `complete` with each possible first letter (a
/// one-letter prefix never ends in ZWNJ, so the output is the display itself).
fn all_displays(dict: &Dictionary) -> Vec<String> {
    let params = ScoreParams::default();
    let rank = RankCtx {
        history: None,
        today: Day(0),
        params: &params,
    };
    let mut out: Vec<String> = FIRST_CHARS
        .iter()
        .flat_map(|r| r.clone())
        .filter_map(char::from_u32)
        // A ك or ة prefix matches the normalised word but echoes the Arabic letter.
        .filter(|c| !matches!(c, '\u{0629}' | '\u{0643}'))
        .flat_map(|c| dict.complete(&c.to_string(), dict.len() + 1, &rank))
        .map(|c| c.text)
        .collect();
    out.sort();
    out
}

fn report_field(r: &BuildReport, name: &str) -> Option<usize> {
    Some(match name {
        "read" => r.read,
        "kept" => r.kept,
        "dropped_chars" => r.dropped_chars,
        "merged" => r.merged,
        "with_zwnj" => r.with_zwnj,
        "bytes" => r.bytes,
        _ => return None,
    })
}

fn run_build(case: &Json) -> Result<(), String> {
    let tsvs = tsv_inputs(case)?;
    let refs: Vec<&str> = tsvs.iter().map(String::as_str).collect();
    let opts = options(case)?;
    if case.get("expect_error").and_then(Json::bool) == Some(true) {
        return match build_bytes(&refs, &opts) {
            Err(_) => Ok(()),
            Ok(_) => Err("expected a build error".into()),
        };
    }
    let (dict, report) = build_dict(&refs, &opts)?;
    let mut errs = Vec::new();
    if let Some(want) = case.get("expect_display").and_then(Json::strings) {
        let mut want = want;
        want.sort();
        let got = all_displays(&dict);
        if got != want {
            errs.push(format!("displays: expected {want:?}, got {got:?}"));
        }
        if dict.len() != want.len() {
            errs.push(format!("len: expected {}, got {}", want.len(), dict.len()));
        }
    }
    for (k, v) in case.get("expect_report").and_then(Json::obj).unwrap_or(&[]) {
        let want = v.num().ok_or("report value")? as usize;
        match report_field(&report, k) {
            Some(got) if got == want => {}
            Some(got) => errs.push(format!("report.{k}: expected {want}, got {got}")),
            None => errs.push(format!("unknown report field {k}")),
        }
    }
    for (w, v) in case.get("expect_prior").and_then(Json::obj).unwrap_or(&[]) {
        let got = dict.prior(w);
        let ok = match (v.num(), got) {
            (Some(want), Some(got)) => (f64::from(got) - want).abs() < 0.051,
            (None, None) => true,
            _ => false,
        };
        if !ok {
            errs.push(format!("prior({w}): expected {v:?}, got {got:?}"));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

#[test]
fn dict_golden_cases() {
    let text = std::fs::read_to_string(golden_file()).expect("data/golden/dict.json");
    let doc = parse_json(&text).expect("dict.json is valid JSON");
    assert_eq!(doc.get("version").and_then(Json::num), Some(1.0));
    let mut failures = Vec::new();
    let mut ran = 0;
    for case in doc.get("cases").and_then(Json::arr).unwrap_or(&[]) {
        let name = case.get("name").and_then(Json::str).unwrap_or("<unnamed>");
        let result = match case.get("type").and_then(Json::str) {
            Some("complete") => run_complete(case),
            Some("build") => run_build(case),
            other => Err(format!("unknown case type {other:?}")),
        };
        ran += 1;
        if let Err(e) = result {
            failures.push(format!("{name}: {e}"));
        }
    }
    eprintln!("dict golden: ran {ran} cases");
    assert!(ran > 0, "no cases in dict.json");
    assert!(
        failures.is_empty(),
        "{} dict golden failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
