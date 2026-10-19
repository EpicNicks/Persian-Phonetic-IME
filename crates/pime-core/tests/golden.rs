//! Runs `data/golden/*.json` (format: docs/contracts.md). Tolerates a missing
//! or empty directory; reports every failing case at once.

use pime_core::keys::{self, vk_for_char};
use pime_core::{
    fallback_of, format_issues, load_config, load_config_with_base, Dictionary, Fallback, KeyEvent,
    LoadedConfig, OptionValue, Output, Session,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/golden")
}

fn load_case_config(config: &Value) -> Result<LoadedConfig, String> {
    match config {
        Value::String(name) => Ok(load_config_with_base(name, "")),
        Value::Object(o) => {
            let preset = o
                .get("preset")
                .and_then(Value::as_str)
                .unwrap_or("qamooscheh");
            let toml = o.get("toml").and_then(Value::as_str).unwrap_or("");
            Ok(load_config_with_base(preset, toml))
        }
        Value::Null => Ok(load_config("")),
        other => Err(format!("bad config field {other}")),
    }
}

fn session_for(config: &Value) -> Result<Session, String> {
    let loaded = load_case_config(config)?;
    // Warnings (e.g. a trigraph without its 2-letter start) are fine here.
    if fallback_of(&loaded.issues) != Fallback::None {
        return Err(format!(
            "case config has errors:\n{}",
            format_issues(&loaded.issues)
        ));
    }
    Ok(Session::new(
        Arc::new(loaded.config),
        Arc::new(Dictionary::empty()),
    ))
}

fn key_event(name: &str) -> Result<KeyEvent, String> {
    let named = match name {
        "Space" => Some(KeyEvent::plain(keys::VK_SPACE)),
        "ShiftSpace" => Some(KeyEvent::shifted(keys::VK_SPACE)),
        "Enter" => Some(KeyEvent::plain(keys::VK_RETURN)),
        "Escape" => Some(KeyEvent::plain(keys::VK_ESCAPE)),
        "Backspace" => Some(KeyEvent::plain(keys::VK_BACK)),
        "Tab" => Some(KeyEvent::plain(keys::VK_TAB)),
        "Up" => Some(KeyEvent::plain(keys::VK_UP)),
        "Down" => Some(KeyEvent::plain(keys::VK_DOWN)),
        "Left" => Some(KeyEvent::plain(keys::VK_LEFT)),
        "Right" => Some(KeyEvent::plain(keys::VK_RIGHT)),
        _ => None,
    };
    if let Some(k) = named {
        return Ok(k);
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => vk_for_char(c)
            .map(|(vk, shift)| KeyEvent {
                vk,
                shift,
                ..Default::default()
            })
            .ok_or_else(|| format!("no US key types {c:?}")),
        _ => Err(format!("unknown key name {name:?}")),
    }
}

fn string_list(v: &Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|x| x.as_str().map(str::to_string))
        .collect()
}

fn run_typing(case: &Value) -> Result<(), String> {
    let mut s = session_for(&case["config"])?;
    let input = case["input"].as_str().ok_or("missing input")?;
    let mut shown = Vec::new();
    let mut last = Output::default();
    for c in input.chars() {
        last = s.on_key(&key_event(&c.to_string())?);
        shown.push(last.preedit.clone());
    }
    let mut errs = Vec::new();
    if let Some(expected) = case.get("preedit_after_each").and_then(string_list) {
        if expected != shown {
            errs.push(format!(
                "preedit_after_each: expected {expected:?}, got {shown:?}"
            ));
        }
    }
    if let Some(expected) = case.get("final_preedit").and_then(Value::as_str) {
        if expected != last.preedit {
            errs.push(format!(
                "final_preedit: expected {expected:?}, got {:?}",
                last.preedit
            ));
        }
    }
    if let Some(expected) = case.get("candidates_after_last") {
        let got = last.candidates.as_ref().map(|c| c.items.clone());
        let want = if expected.is_null() {
            None
        } else {
            string_list(expected)
        };
        if got != want {
            errs.push(format!(
                "candidates_after_last: expected {want:?}, got {got:?}"
            ));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

fn run_session(case: &Value) -> Result<(), String> {
    let mut s = session_for(&case["config"])?;
    let keys = case["keys"].as_array().ok_or("missing keys")?;
    let mut committed = String::new();
    let mut last = Output::default();
    for k in keys {
        let name = k.as_str().ok_or("key is not a string")?;
        last = s.on_key(&key_event(name)?);
        committed.push_str(&last.commit);
    }
    let mut errs = Vec::new();
    let want_commit = case["committed"].as_str().ok_or("missing committed")?;
    if want_commit != committed {
        errs.push(format!(
            "committed: expected {want_commit:?}, got {committed:?}"
        ));
    }
    if let Some(want) = case.get("preedit").and_then(Value::as_str) {
        if want != last.preedit {
            errs.push(format!(
                "preedit: expected {want:?}, got {:?}",
                last.preedit
            ));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

fn json_to_option(v: &Value) -> Option<OptionValue> {
    Some(match v {
        Value::String(s) => OptionValue::Text(s.clone()),
        Value::Bool(b) => OptionValue::Bool(*b),
        Value::Number(n) => OptionValue::Integer(n.as_i64()?),
        Value::Array(_) => OptionValue::List(string_list(v)?),
        _ => return None,
    })
}

fn run_config_error(case: &Value) -> Result<(), String> {
    let toml = case["user_config_toml"]
        .as_str()
        .ok_or("missing user_config_toml")?;
    let loaded = load_config(toml);
    let text = format_issues(&loaded.issues);
    let expect = &case["expect"];
    let mut errs = Vec::new();
    let want_fallback = match expect["fallback"].as_str() {
        Some("none") => Fallback::None,
        Some("entry") => Fallback::Entry,
        Some("whole_file") => Fallback::WholeFile,
        other => return Err(format!("bad fallback {other:?}")),
    };
    let got_fallback = fallback_of(&loaded.issues);
    if got_fallback != want_fallback {
        errs.push(format!(
            "fallback: expected {want_fallback:?}, got {got_fallback:?}"
        ));
    }
    if let Some(applied) = expect.get("applied").and_then(Value::as_object) {
        for (path, want) in applied {
            let want = json_to_option(want).ok_or_else(|| format!("bad applied value {want}"))?;
            let got = loaded.config.option_value(path);
            if got.as_ref() != Some(&want) {
                errs.push(format!("applied {path}: expected {want:?}, got {got:?}"));
            }
        }
    }
    let contains = expect
        .get("messages_contain")
        .and_then(string_list)
        .unwrap_or_default();
    if contains.is_empty() && !loaded.issues.is_empty() {
        errs.push(format!("expected no messages, got:\n{text}"));
    }
    for needle in &contains {
        if !text.contains(needle.as_str()) {
            errs.push(format!("messages lack {needle:?}"));
        }
    }
    if !errs.is_empty() {
        errs.push(format!("full messages:\n{text}"));
        return Err(errs.join("\n  "));
    }
    Ok(())
}

#[test]
fn golden_cases() {
    let dir = golden_dir();
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect(),
        Err(_) => {
            eprintln!("no golden directory at {}; skipping", dir.display());
            return;
        }
    };
    files.sort();
    if files.is_empty() {
        eprintln!("no golden files in {}; skipping", dir.display());
    }
    let mut failures = Vec::new();
    let mut ran = 0;
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let doc: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: invalid JSON: {e}", file.display()));
                continue;
            }
        };
        assert_eq!(doc["version"], 1, "{}: unsupported version", file.display());
        for case in doc["cases"].as_array().into_iter().flatten() {
            let name = case["name"].as_str().unwrap_or("<unnamed>");
            let result = match case["type"].as_str() {
                Some("typing") => run_typing(case),
                Some("session") => run_session(case),
                Some("config_error") => run_config_error(case),
                other => Err(format!("unknown case type {other:?}")),
            };
            ran += 1;
            if let Err(e) = result {
                let file = file.file_name().unwrap().to_string_lossy();
                failures.push(format!("{file} / {name}: {e}"));
            }
        }
    }
    eprintln!("golden: ran {ran} cases from {} files", files.len());
    assert!(
        failures.is_empty(),
        "{} golden failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
