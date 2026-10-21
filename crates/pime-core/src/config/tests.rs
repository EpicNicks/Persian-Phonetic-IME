use super::*;

fn load(toml: &str) -> LoadedConfig {
    load_config(toml)
}

fn text_of(issues: &[ConfigIssue]) -> String {
    format_issues(issues)
}

fn list(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn every_builtin_preset_is_clean() {
    let names = builtin_preset_names();
    assert_eq!(names[0], DEFAULT_PRESET);
    assert!(
        names.contains(&"minimal"),
        "data/presets/minimal.toml should be embedded"
    );
    for name in names {
        let issues = builtin_preset_issues(name).unwrap();
        assert!(issues.is_empty(), "preset {name}:\n{}", text_of(&issues));
        let cfg = ResolvedConfig::builtin(name).unwrap();
        assert_eq!(cfg.preset_name(), name);
        for key in cfg.keys() {
            for c in cfg.candidates(key).unwrap() {
                assert!(
                    c.chars().all(|ch| arabic_form(ch).is_none()),
                    "preset {name} key {key} uses an Arabic-only form"
                );
            }
        }
    }
    assert!(ResolvedConfig::builtin("nope").is_none());
}

#[test]
fn qamooscheh_e_equals_a_and_c_is_unmapped() {
    let cfg = ResolvedConfig::default();
    assert_eq!(cfg.preset_name(), "qamooscheh");
    assert_eq!(cfg.candidates("e"), cfg.candidates("a"));
    assert!(cfg.candidates("c").is_none());
    assert!(cfg.is_key_prefix("c"), "c starts ch");
    assert_eq!(
        cfg.candidates("s").unwrap(),
        list(&["س", "ص", "ث"]).as_slice()
    );
}

#[test]
fn minimal_has_no_ambiguity() {
    let cfg = ResolvedConfig::builtin("minimal").unwrap();
    for key in cfg.keys() {
        assert_eq!(cfg.candidates(key).unwrap().len(), 1, "minimal key {key}");
    }
}

#[test]
fn emergency_preset_matches_the_data_file() {
    let Some((_, data)) = DATA_PRESETS.iter().find(|(n, _)| *n == DEFAULT_PRESET) else {
        return;
    };
    let mut a = Vec::new();
    let mut b = Vec::new();
    let pa = parse(data, false, &mut a).unwrap();
    let pb = parse(EMERGENCY_PRESET, false, &mut b).unwrap();
    let ra = resolve_with_preset("x", &pa, &Parsed::default(), &mut Vec::new(), &mut a);
    let rb = resolve_with_preset("x", &pb, &Parsed::default(), &mut Vec::new(), &mut b);
    assert_eq!(
        ra, rb,
        "update src/config/emergency_preset.toml from data/presets/qamooscheh.toml"
    );
}

#[test]
fn gh_and_q_options_promote() {
    let cfg = load("[options]\ngh = \"qaf\"\nq = \"ghain\"\n").config;
    assert_eq!(cfg.candidates("gh").unwrap(), list(&["ق", "غ"]).as_slice());
    assert_eq!(cfg.candidates("q").unwrap(), list(&["غ", "ق"]).as_slice());
    let min = load_config_with_base("minimal", "[options]\ngh = \"qaf\"\n").config;
    assert_eq!(min.candidates("gh").unwrap(), list(&["ق", "غ"]).as_slice());
    assert_eq!(
        min.option_value("gh"),
        Some(OptionValue::Text("qaf".into()))
    );
}

#[test]
fn c_options() {
    assert!(load("[options]\nc = \"none\"\n")
        .config
        .candidates("c")
        .is_none());
    assert_eq!(
        load("[options]\nc = \"k\"\n")
            .config
            .candidates("c")
            .unwrap(),
        ["ک"]
    );
    let s = load("[options]\nc = \"s\"\ns_order = [\"ث\", \"س\"]\n").config;
    assert_eq!(s.candidates("c").unwrap(), list(&["ث", "س"]).as_slice());
    assert_eq!(s.candidates("ch").unwrap(), ["چ"]);
}

#[test]
fn raw_map_wins_over_options() {
    let cfg = load("[options]\nc = \"k\"\n[map.singles]\nc = [\"س\"]\n").config;
    assert_eq!(cfg.candidates("c").unwrap(), ["س"]);
}

#[test]
fn behaviour_options() {
    let cfg = load(
        "[options]\ndigits = \"ascii\"\npunctuation = \"none\"\nguillemets = \"angle\"\n\
         shift_letters = \"alternate\"\nspace = \"top-candidate\"\nenter = \"commit-and-pass\"\n\
         number_select = false\ncandidate_labels = true\nallow_arabic_forms = true\n\
         allow_sequences = true\n[autocomplete]\nenabled = true\npredict_min_chars = 3\n\
         learning = false\n",
    );
    assert!(cfg.issues.is_empty(), "{}", text_of(&cfg.issues));
    let o = *cfg.config.options();
    assert_eq!(o.digits, Digits::Ascii);
    assert_eq!(o.punctuation, Punctuation::None);
    assert_eq!(o.guillemets, Guillemets::Angle);
    assert_eq!(o.shift_letters, ShiftLetters::Alternate);
    assert_eq!(o.space, SpaceMode::TopCandidate);
    assert_eq!(o.enter, EnterMode::CommitAndPass);
    assert!(!o.number_select && o.candidate_labels && o.allow_arabic_forms && o.allow_sequences);
    assert_eq!(
        o.autocomplete,
        Autocomplete {
            enabled: true,
            predict_min_chars: 3,
            learning: false,
            max_completions: 5,
        }
    );
    let c = &cfg.config;
    assert_eq!(c.convert_char('1'), None);
    assert_eq!(c.convert_char(','), None);
    assert_eq!(c.convert_char('<'), Some("«"));
    assert_eq!(
        c.option_value("autocomplete.predict_min_chars"),
        Some(OptionValue::Integer(3))
    );
}

#[test]
fn default_conversions() {
    let c = ResolvedConfig::default();
    assert_eq!(c.convert_char('0'), Some("۰"));
    assert_eq!(c.convert_char('9'), Some("۹"));
    assert_eq!(c.convert_char('?'), Some("؟"));
    assert_eq!(c.convert_char('<'), None);
    assert_eq!(c.convert_char('-'), None);
    let brace = load("[options]\nguillemets = \"brace\"\n").config;
    assert_eq!(brace.convert_char('{'), Some("«"));
    assert_eq!(brace.convert_char('<'), None);
}

#[test]
fn option_value_paths() {
    let c = ResolvedConfig::default();
    assert_eq!(
        c.option_value("digits"),
        Some(OptionValue::Text("persian".into()))
    );
    assert_eq!(
        c.option_value("number_select"),
        Some(OptionValue::Bool(true))
    );
    assert_eq!(
        c.option_value("map.singles.b"),
        Some(OptionValue::List(list(&["ب"])))
    );
    assert_eq!(
        c.option_value("map.digraphs.sh"),
        Some(OptionValue::List(list(&["ش"])))
    );
    assert_eq!(c.option_value("map.singles.sh"), None);
    assert_eq!(c.option_value("map.nope.b"), None);
    assert_eq!(
        c.option_value("s_order"),
        Some(OptionValue::List(list(&["س", "ص", "ث"])))
    );
    assert_eq!(c.option_value("nonsense"), None);
}

fn only_issue(toml: &str) -> ConfigIssue {
    let loaded = load(toml);
    assert_eq!(
        loaded.issues.len(),
        1,
        "{toml:?}:\n{}",
        text_of(&loaded.issues)
    );
    loaded.issues[0].clone()
}

#[test]
fn syntax_errors_point_at_the_line() {
    let cases: &[(&str, usize, &str)] = &[
        ("[options]\ngh = \"qaf\n", 2, "closing quote"),
        ("[options]\n\n\ngh = 'qaf\n", 4, "closing quote"),
        ("[options\ngh = \"qaf\"\n", 1, "closing bracket"),
        ("[options]\ngh \"qaf\"\n", 2, "\"=\" is missing"),
        (
            "[options]\ndigits = \"ascii\"\ndigits = \"persian\"\n",
            3,
            "\"digits\" is set twice",
        ),
        (
            "[options]\ndigits = \"ascii\"\nthis is not toml\n",
            3,
            "couldn't be read",
        ),
        ("[map.singles]\nb = [\"ب\"\n", 2, "closing bracket"),
        (
            "[options]\ngh = qaf\n",
            2,
            "text needs quotes around it, like \"qaf\"",
        ),
    ];
    for (src, line, needle) in cases {
        let issue = only_issue(src);
        assert_eq!(issue.kind, IssueKind::Syntax, "{src:?}");
        assert_eq!(issue.line, *line, "{src:?}: {}", issue.message);
        let text = text_of(&[issue]);
        assert!(text.contains(needle), "{src:?}: {text}");
        assert!(text.contains("whole settings file was skipped"), "{text}");
    }
}

#[test]
fn syntax_error_discards_whole_file() {
    let loaded = load("preset = \"minimal\"\n[options]\ndigits = \"ascii\"\nbroken\n");
    assert_eq!(fallback_of(&loaded.issues), Fallback::WholeFile);
    assert_eq!(loaded.config, ResolvedConfig::default());
}

#[test]
fn unknown_keys_suggest() {
    let i = only_issue("[options]\ndiigts = \"ascii\"\n");
    assert_eq!((i.kind, i.line), (IssueKind::UnknownKey, 2));
    assert_eq!(
        text_of(&[i]),
        "config.toml, line 2: unknown setting \"diigts\". Did you mean \"digits\"?\n"
    );
    let i = only_issue("digits = \"ascii\"\n");
    assert!(i.hint.contains("[options]"), "{i:?}");
    let i = only_issue("prest = \"minimal\"\n");
    assert!(i.hint.contains("\"preset\""), "{i:?}");
    let i = only_issue("[map.single]\nb = [\"ب\"]\n");
    assert_eq!(i.kind, IssueKind::UnknownSection);
    assert!(i.hint.contains("map.singles"), "{i:?}");
    let i = only_issue("[optons]\ngh = \"qaf\"\n");
    assert_eq!((i.kind, i.line), (IssueKind::UnknownSection, 1));
}

#[test]
fn wrong_types_are_explained() {
    let i = only_issue("[options]\nnumber_select = \"yes\"\n");
    assert_eq!(
        i.message,
        "number_select must be true or false (for example true), not text."
    );
    let i = only_issue("[options]\ndigits = 5\n");
    assert!(
        i.message.contains("not a number") && i.hint.contains("\"ascii\""),
        "{i:?}"
    );
    let i = only_issue("[autocomplete]\npredict_min_chars = 0\n");
    assert_eq!(i.kind, IssueKind::InvalidValue);
    let i = only_issue("preset = 3\n");
    assert_eq!(i.kind, IssueKind::WrongType);
    let i = only_issue("options = 3\n");
    assert_eq!(i.kind, IssueKind::WrongType);
    let i = only_issue("[map.singles]\nb = [1, 2]\n");
    assert!(i.message.contains("must be a list"), "{i:?}");
}

#[test]
fn map_problems_skip_the_entry() {
    let i = only_issue("[map.singles]\nS = [\"ص\"]\n");
    assert_eq!(i.kind, IssueKind::BadMapKey);
    let i = only_issue("[map.singles]\nb = [\"\"]\n");
    assert_eq!(i.kind, IssueKind::BadCandidate);
    let i = only_issue("[map.singles]\nb = [\"\u{0007}\"]\n");
    assert_eq!(
        i.kind,
        IssueKind::Syntax,
        "TOML forbids raw control characters"
    );
    assert!(
        i.message.contains("control character") && i.snippet.contains("U+0007"),
        "{i:?}"
    );
    let i = only_issue("[map.singles]\nb = [\"\\u0007\"]\n");
    assert!(i.message.contains("U+0007"), "{i:?}");
    let loaded = load("[options]\nallow_sequences = true\n[map.singles]\na = [\"ا\u{0653}\"]\n");
    assert_eq!(loaded.issues.len(), 1);
    assert!(loaded.issues[0].message.contains("آ"));
    let loaded = load("[options]\ns_order = [\"س\", \"x\"]\n");
    assert_eq!(fallback_of(&loaded.issues), Fallback::Entry);
    assert_eq!(loaded.config.candidates("s").unwrap().len(), 3);
}

#[test]
fn arabic_forms_warn_everywhere() {
    let i = only_issue("[options]\ns_order = [\"ك\", \"س\"]\n");
    assert_eq!(
        (i.severity, i.kind),
        (Severity::Warning, IssueKind::ArabicForm)
    );
    let loaded = load("[options]\nallow_arabic_forms = true\ns_order = [\"ك\"]\n");
    assert!(loaded.issues.is_empty());
}

#[test]
fn alternative_toml_shapes_work() {
    let dotted = load("map.singles.v = [\"ف\"]\n");
    assert!(dotted.issues.is_empty(), "{}", text_of(&dotted.issues));
    assert_eq!(dotted.config.candidates("v").unwrap(), ["ف"]);
    let inline = load("map = { digraphs = { dh = [\"ذ\"] } }\noptions = { digits = \"ascii\" }\n");
    assert!(inline.issues.is_empty(), "{}", text_of(&inline.issues));
    assert_eq!(inline.config.candidates("dh").unwrap(), ["ذ"]);
    assert_eq!(inline.config.options().digits, Digits::Ascii);
    let i = only_issue("map.singles.v = []\n");
    assert_eq!(i.line, 1);
}

#[test]
fn issues_are_sorted_and_all_reported() {
    let loaded =
        load("[options]\ngh = \"kaf\"\ndigits = \"binary\"\n[map.singles]\nb = []\nv = [\"x\"]\n");
    let lines: Vec<usize> = loaded.issues.iter().map(|i| i.line).collect();
    assert_eq!(lines, [2, 3, 5, 6]);
}

#[test]
fn messages_avoid_jargon() {
    let inputs = [
        "[options]\ngh = \"qaf\n",
        "[options\n",
        "x",
        "= 3",
        "[[options]]\n",
        "[options]\ngh = 1979-05-27\n",
        "[options]\ngh = { a = 1 }\n",
        "a.b.c = 1\n",
        "[map]\nsingles = 5\n",
        "[map.singles]\n\"\" = [\"ب\"]\n",
        "[map.singles]\nb = [\"ب\", \"ب\"]\n",
        "[options]\nnumber_select = 1.5\n",
        "\u{0}",
        "[options]\n\"\\u0000\" = 1\n",
        "preset = \"qamoosheh\"\n",
    ];
    for src in inputs {
        let loaded = load(src);
        assert!(!loaded.issues.is_empty(), "{src:?} should have issues");
        let text = text_of(&loaded.issues);
        for word in [
            "TOML",
            "toml_edit",
            "serde",
            "parse",
            "expected",
            "invalid",
            "`",
            "Error",
        ] {
            assert!(!text.contains(word), "{src:?} mentions {word:?}:\n{text}");
        }
        for issue in &loaded.issues {
            assert!(
                issue.message.ends_with('.') || issue.message.ends_with('?'),
                "{issue:?}"
            );
        }
    }
    let i = only_issue("preset = \"qamoosheh\"\n");
    assert!(i.hint.contains("qamooscheh"), "{i:?}");
}

#[test]
fn messages_never_quote_control_characters() {
    let loaded = load("[map.singles]\n\"\\u0001\" = [\"\\u0002\"]\n");
    let text = text_of(&loaded.issues);
    assert!(
        !text.chars().any(|c| c.is_control() && c != '\n'),
        "{text:?}"
    );
}

#[test]
fn option_specs_are_the_validators_source() {
    let names: Vec<&str> = option_specs().iter().map(|s| s.name).collect();
    assert_eq!(names.len(), 20);
    for spec in option_specs() {
        // The name is accepted (no "unknown setting") and defaults match the resolved values.
        if let Some(default) = spec.default_value() {
            let src = match &default {
                OptionValue::Text(t) => format!("[options]\n{} = \"{t}\"\n", spec.name),
                OptionValue::Bool(b) => format!("[options]\n{} = {b}\n", spec.name),
                OptionValue::Integer(i) => format!("[options]\n{} = {i}\n", spec.name),
                other => panic!("{other:?}"),
            };
            let loaded = load(&src);
            assert!(
                loaded.issues.is_empty(),
                "{src}: {}",
                text_of(&loaded.issues)
            );
            assert_eq!(
                ResolvedConfig::default().option_value(spec.name),
                Some(default)
            );
        }
        match spec.kind {
            OptionKind::Choice(values) => {
                assert_eq!(spec.allowed_values(), values);
                assert_eq!(
                    spec.default_value(),
                    Some(OptionValue::Text(values[0].into()))
                );
                let i = only_issue(&format!("[options]\n{} = \"zzz\"\n", spec.name));
                assert_eq!(i.hint, format!("Use {}.", or_list(values)), "{}", spec.name);
                for v in values {
                    let loaded = load(&format!("[options]\n{} = \"{v}\"\n", spec.name));
                    assert!(loaded.issues.is_empty(), "{} = {v}", spec.name);
                }
            }
            OptionKind::Bool => {
                assert!(spec.allowed_values().is_empty());
                let i = only_issue(&format!("[options]\n{} = \"yes\"\n", spec.name));
                assert!(i.message.contains("true or false"), "{i:?}");
            }
            OptionKind::List => {
                assert_eq!(spec.default_value(), None);
                let i = only_issue(&format!("[options]\n{} = 3\n", spec.name));
                assert!(i.message.contains("a list"), "{i:?}");
            }
            OptionKind::Integer { .. } => {
                let i = only_issue(&format!(
                    "[options]
{} = \"big\"
",
                    spec.name
                ));
                assert!(i.message.contains("whole number"), "{i:?}");
            }
        }
    }
    let ac: Vec<&str> = autocomplete_specs().iter().map(|s| s.name).collect();
    assert_eq!(
        ac,
        [
            "enabled",
            "predict_min_chars",
            "learning",
            "max_completions"
        ]
    );
    let min = autocomplete_specs()[1];
    assert_eq!(min.kind, OptionKind::Integer { min: 1 });
    assert_eq!(min.default_value(), Some(OptionValue::Integer(2)));
    let max = autocomplete_specs()[3];
    assert_eq!(max.kind, OptionKind::Integer { min: 1 });
    assert_eq!(max.default_value(), Some(OptionValue::Integer(5)));
}

#[test]
fn max_completions_option() {
    let d = ResolvedConfig::default();
    assert_eq!(d.options().autocomplete.max_completions, 5);
    assert_eq!(
        d.option_value("autocomplete.max_completions"),
        Some(OptionValue::Integer(5))
    );
    let max = |v: &str| {
        load(&format!("[autocomplete]\nmax_completions = {v}\n"))
            .config
            .options()
            .autocomplete
            .max_completions
    };
    assert_eq!(max("1"), 1);
    assert_eq!(max("7"), 7);
    assert_eq!(max("9"), 9);
    assert_eq!(max("10"), 9, "clamped silently");
    assert_eq!(max("1000000"), 9);
    assert!(load("[autocomplete]\nmax_completions = 40\n")
        .issues
        .is_empty());
    for bad in ["0", "-3"] {
        let loaded = load(&format!(
            "[autocomplete]\nmax_completions = {bad}\nlearning = false\n"
        ));
        assert_eq!(loaded.issues.len(), 1, "{}", text_of(&loaded.issues));
        let i = &loaded.issues[0];
        assert_eq!(i.kind, IssueKind::InvalidValue);
        assert_eq!(
            i.message,
            format!("max_completions must be at least 1, not {bad}.")
        );
        assert_eq!(fallback_of(&loaded.issues), Fallback::Entry);
        let ac = loaded.config.options().autocomplete;
        assert_eq!(ac.max_completions, 5, "default kept");
        assert!(!ac.learning, "the rest still applies");
    }
    let i = only_issue("[autocomplete]\nmax_completions = \"five\"\n");
    assert_eq!(
        i.message,
        "max_completions must be a whole number (for example 5), not text."
    );
    let i = only_issue("[autocomplete]\nenabled = \"yes\"\n");
    assert_eq!(
        i.message,
        "enabled must be true or false (for example true), not text."
    );
    let i = only_issue("[autocomplete]\nmax_completion = 3\n");
    assert_eq!(i.kind, IssueKind::UnknownKey);
    assert_eq!(i.hint, "Did you mean \"max_completions\"?");
}

#[test]
fn candidate_position_option() {
    let default = ResolvedConfig::default();
    assert_eq!(
        default.options().candidate_position,
        CandidatePosition::Below
    );
    assert_eq!(
        default.option_value("candidate_position"),
        Some(OptionValue::Text("below".into()))
    );
    let min = ResolvedConfig::builtin("minimal").unwrap();
    assert_eq!(min.options().candidate_position, CandidatePosition::Below);
    let above = load(
        "[options]
candidate_position = \"above\"
",
    );
    assert!(above.issues.is_empty(), "{}", text_of(&above.issues));
    assert_eq!(
        above.config.options().candidate_position,
        CandidatePosition::Above
    );
    let below = load(
        "[options]
candidate_position = \"below\"
",
    );
    assert!(below.issues.is_empty());
    assert_eq!(
        below.config.options().candidate_position,
        CandidatePosition::Below
    );
    let bad = load(
        "[options]
candidate_position = \"top\"
digits = \"ascii\"
",
    );
    assert_eq!(fallback_of(&bad.issues), Fallback::Entry);
    assert_eq!(bad.issues[0].hint, "Use \"below\" or \"above\".");
    assert_eq!(
        bad.config.options().candidate_position,
        CandidatePosition::Below
    );
    assert_eq!(bad.config.options().digits, Digits::Ascii);
    let spec = option_specs()
        .iter()
        .find(|s| s.name == "candidate_position")
        .unwrap();
    assert_eq!(spec.allowed_values(), ["below", "above"]);
    assert_eq!(
        spec.default_value(),
        Some(OptionValue::Text("below".into()))
    );
}

#[test]
fn candidate_size_option() {
    assert_eq!(ResolvedConfig::default().options().candidate_size, 50);
    let size = |v: i64| {
        load(&format!(
            "[options]
candidate_size = {v}
"
        ))
        .config
        .options()
        .candidate_size
    };
    assert_eq!(size(100), 100);
    assert_eq!(size(5), 25, "clamped up");
    assert_eq!(size(5000), 300, "clamped down");
}

#[test]
fn toggle_hotkey_option() {
    assert_eq!(
        ResolvedConfig::default().options().toggle_hotkey,
        ToggleHotkey::None
    );
    let hk = |v: &str| {
        load(&format!(
            "[options]
toggle_hotkey = \"{v}\"
"
        ))
        .config
        .options()
        .toggle_hotkey
    };
    assert_eq!(hk("alt+`"), ToggleHotkey::AltBackquote);
    assert_eq!(hk("ctrl+space").chord(), Some((0x20, true, false, false)));
    assert_eq!(hk("alt+`").chord(), Some((0xC0, false, false, true)));
    assert_eq!(ToggleHotkey::None.chord(), None);
    let i = only_issue(
        "[options]
toggle_hotkey = \"f9\"
",
    );
    assert!(i.hint.contains("alt+`"), "{i:?}");
}

#[test]
fn c_as_s_follows_the_final_s() {
    let cfg = load("[options]\nc = \"s\"\n[map.singles]\ns = [\"ث\", \"س\"]\n").config;
    assert_eq!(cfg.candidates("c").unwrap(), list(&["ث", "س"]).as_slice());
    let cfg = load("[options]\nc = \"s\"\n[map.singles]\ns = [\"ث\"]\nc = [\"ک\"]\n").config;
    assert_eq!(cfg.candidates("c").unwrap(), ["ک"]);
}
