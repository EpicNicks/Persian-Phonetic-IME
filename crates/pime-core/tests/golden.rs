//! Runs `data/golden/*.json` (format: docs/contracts.md). Tolerates a missing
//! or empty directory; reports every failing case at once.

use pime_core::keys::{self, vk_for_char};
use pime_core::{
    fallback_of, format_issues, load_config, load_config_with_base, Completer, ContextPolicy,
    Dictionary, Fallback, KeyEvent, LearnEvent, LearnKind, ListCompleter, LoadedConfig,
    OptionValue, Output, Session,
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
        "ShiftTab" => Some(KeyEvent::shifted(keys::VK_TAB)),
        "Delete" => Some(KeyEvent::plain(keys::VK_DELETE)),
        "Home" => Some(KeyEvent::plain(keys::VK_HOME)),
        "End" => Some(KeyEvent::plain(keys::VK_END)),
        "CapsLock" => Some(KeyEvent::plain(keys::VK_CAPITAL)),
        _ => None,
    };
    if let Some(k) = named {
        return Ok(k);
    }
    // "Ctrl+x", "Alt+x", "Ctrl+Alt+x": a chord over any key name above or a letter.
    if let Some((mods, base)) = name.rsplit_once('+').filter(|(_, b)| !b.is_empty()) {
        let mut k = key_event(base)?;
        for m in mods.split('+') {
            match m {
                "Ctrl" => k.ctrl = true,
                "Alt" => k.alt = true,
                _ => return Err(format!("unknown modifier in {name:?}")),
            }
        }
        return Ok(k);
    }
    // "Numpad5": a numpad digit.
    if let Some(d) = name
        .strip_prefix("Numpad")
        .and_then(|d| d.parse::<u32>().ok())
    {
        if d <= 9 {
            return Ok(KeyEvent::plain(keys::VK_NUMPAD0 + d));
        }
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

/// `"completer": {"words": [[text, score], ...]}` -> a `ListCompleter`.
fn completer_for(v: &Value) -> Result<Arc<dyn Completer>, String> {
    let words = v["words"].as_array().ok_or("completer.words missing")?;
    let mut list = Vec::new();
    for w in words {
        match (w[0].as_str(), w[1].as_f64()) {
            (Some(t), Some(score)) => list.push((t.to_string(), score as f32)),
            _ => return Err(format!("bad completer word {w}")),
        }
    }
    Ok(Arc::new(ListCompleter::new(list)))
}

fn learn_kind(name: &str) -> Result<LearnKind, String> {
    Ok(match name {
        "accepted" => LearnKind::Accepted,
        "skipped" => LearnKind::Skipped,
        "ignored" => LearnKind::Ignored,
        "dismissed" => LearnKind::Dismissed,
        other => return Err(format!("unknown learn kind {other:?}")),
    })
}

/// One scripted step: a key, or the runner-only events `"Click<row>"`
/// (`select_candidate(row)`, 0-based), `"FocusLost"` and `"Terminated"`.
fn step(s: &mut Session, name: &str) -> Result<Output, String> {
    if let Some(row) = name.strip_prefix("Click").and_then(|r| r.parse().ok()) {
        return Ok(s.select_candidate(row));
    }
    match name {
        "FocusLost" => Ok(s.on_focus_lost()),
        "Terminated" => {
            s.on_composition_terminated();
            Ok(s.snapshot())
        }
        _ => {
            let k = key_event(name)?;
            let before = s.snapshot();
            let predicted = s.would_eat(&k);
            if s.snapshot() != before {
                return Err(format!("would_eat({name}) changed the session"));
            }
            let out = s.on_key(&k);
            if out.eaten != predicted {
                return Err(format!("would_eat({name}) disagrees with on_key"));
            }
            Ok(out)
        }
    }
}

fn run_session(case: &Value) -> Result<(), String> {
    let mut s = session_for(&case["config"])?;
    if let Some(c) = case.get("completer") {
        s.set_completer(Some(completer_for(c)?));
    }
    if let Some(p) = case.get("policy") {
        s.set_context_policy(ContextPolicy {
            completions: p["completions"].as_bool().unwrap_or(true),
            learning: p["learning"].as_bool().unwrap_or(true),
        });
    }
    let keys = case["keys"].as_array().ok_or("missing keys")?;
    let mut committed = String::new();
    let mut learn = Vec::new();
    let mut last = Output::default();
    for k in keys {
        let name = k.as_str().ok_or("key is not a string")?;
        last = step(&mut s, name)?;
        committed.push_str(&last.commit);
        learn.extend(last.learn.iter().cloned());
    }
    let mut errs = Vec::new();
    if let Some(want) = case.get("completions_after_last") {
        let want = if want.is_null() {
            None
        } else {
            Some(string_list(want).ok_or("bad completions_after_last")?)
        };
        let got = last.completions.as_ref().map(|c| c.items.clone());
        if got != want {
            errs.push(format!(
                "completions_after_last: expected {want:?}, got {got:?}"
            ));
        }
    }
    if let Some(want) = case.get("completion_highlight_after_last") {
        let want = want.as_u64().map(|h| h as usize);
        let got = last.completions.as_ref().and_then(|c| c.highlighted);
        if got != want {
            errs.push(format!(
                "completion_highlight_after_last: expected {want:?}, got {got:?}"
            ));
        }
    }
    if let Some(want) = case.get("candidates_after_last") {
        let want = if want.is_null() {
            None
        } else {
            Some(string_list(want).ok_or("bad candidates_after_last")?)
        };
        let got = last.candidates.as_ref().map(|c| c.items.clone());
        if got != want {
            errs.push(format!(
                "candidates_after_last: expected {want:?}, got {got:?}"
            ));
        }
    }
    if let Some(want) = case.get("learn") {
        let mut want_events = Vec::new();
        for e in want.as_array().ok_or("learn must be a list")? {
            let (Some(kind), Some(word)) = (e[0].as_str(), e[1].as_str()) else {
                return Err(format!("bad learn entry {e}"));
            };
            want_events.push(LearnEvent {
                word: word.to_string(),
                kind: learn_kind(kind)?,
            });
        }
        if want_events != learn {
            errs.push(format!("learn: expected {want_events:?}, got {learn:?}"));
        }
    }
    let want_commit = case["committed"].as_str().ok_or("missing committed")?;
    if want_commit != committed {
        errs.push(format!(
            "committed: expected {want_commit:?}, got {committed:?}"
        ));
    }
    if let Some(want) = case.get("last_eaten").and_then(Value::as_bool) {
        if want != last.eaten {
            errs.push(format!("last_eaten: expected {want}, got {}", last.eaten));
        }
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
                // Dictionary cases (data/golden/dict.json) run in pime-dict's tests.
                Some("complete" | "build") => continue,
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
