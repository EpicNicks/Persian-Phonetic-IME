use super::*;
use crate::config::load_config;

fn session(toml: &str) -> Session {
    let loaded = load_config(toml);
    assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
    Session::new(Arc::new(loaded.config), Arc::new(Dictionary::empty()))
}

fn key(c: char) -> KeyEvent {
    let (vk, shift) = vk_for_char(c).unwrap();
    KeyEvent {
        vk,
        shift,
        ..Default::default()
    }
}

fn type_str(s: &mut Session, text: &str) -> Output {
    let mut last = Output::default();
    for c in text.chars() {
        last = s.on_key(&key(c));
    }
    last
}

#[test]
fn letters_are_eaten_and_preedit_grows() {
    let mut s = session("");
    let out = s.on_key(&key('s'));
    assert!(out.eaten);
    assert_eq!(out.preedit, "س");
    let list = out.candidates.unwrap();
    assert_eq!(list.items, ["س", "ص", "ث"]);
    assert_eq!(
        (list.highlighted, list.kind),
        (0, CandidateKind::Alternates)
    );
    assert!(s.is_composing());
}

#[test]
fn would_eat_agrees_and_has_no_side_effects() {
    let mut s = session("");
    type_str(&mut s, "sa");
    let keys = [
        KeyEvent::plain(VK_LEFT),
        KeyEvent::plain(VK_BACK),
        KeyEvent::plain(VK_RETURN),
        KeyEvent::plain(VK_SHIFT),
        KeyEvent {
            vk: VK_A,
            ctrl: true,
            ..Default::default()
        },
        key('1'),
    ];
    for k in keys {
        let before = s.snapshot();
        let predicted = s.would_eat(&k);
        assert_eq!(s.snapshot(), before);
        assert_eq!(s.clone().on_key(&k).eaten, predicted, "{k:?}");
    }
}

#[test]
fn modifiers_alone_change_nothing() {
    let mut s = session("");
    type_str(&mut s, "sh");
    for vk in [VK_SHIFT, VK_LSHIFT, VK_CONTROL, VK_RMENU, VK_CAPITAL] {
        let out = s.on_key(&KeyEvent {
            vk,
            shift: true,
            ..Default::default()
        });
        assert!(!out.eaten);
        assert_eq!(out.commit, "");
        assert_eq!(out.preedit, "ش");
    }
}

#[test]
fn chords_and_navigation_commit_then_pass() {
    for k in [
        KeyEvent {
            vk: VK_A + 2,
            ctrl: true,
            ..Default::default()
        },
        KeyEvent {
            vk: VK_TAB,
            alt: true,
            ..Default::default()
        },
        KeyEvent::plain(VK_LWIN),
        KeyEvent::plain(VK_HOME),
        KeyEvent::plain(VK_DELETE),
        KeyEvent::plain(0x70), // F1
    ] {
        let mut s = session("");
        type_str(&mut s, "sh");
        let out = s.on_key(&k);
        assert_eq!(
            (out.commit.as_str(), out.preedit.as_str(), out.eaten),
            ("ش", "", false)
        );
        let idle = s.on_key(&k);
        assert_eq!(
            (idle.commit.as_str(), idle.eaten),
            ("", false),
            "{k:?} idle passes"
        );
    }
}

#[test]
fn lone_c_is_pending() {
    let mut s = session("");
    let out = s.on_key(&key('c'));
    assert_eq!(out.preedit, "c");
    assert_eq!(out.preedit_pending, Some(0..1));
    let out = s.on_key(&key('h'));
    assert_eq!((out.preedit.as_str(), out.preedit_pending), ("چ", None));
    let out = type_str(&mut s, "ac");
    assert_eq!(out.preedit, "چاc");
    assert_eq!(out.preedit_pending, Some(2..3));
}

#[test]
fn escape_closes_then_tab_reopens_without_moving() {
    let mut s = session("");
    s.on_key(&key('s'));
    let out = s.on_key(&KeyEvent::plain(VK_ESCAPE));
    assert!(out.eaten && out.candidates.is_none());
    assert_eq!(out.preedit, "س");
    let out = s.on_key(&KeyEvent::plain(VK_TAB));
    assert_eq!(out.candidates.unwrap().highlighted, 0);
    let out = s.on_key(&KeyEvent::plain(VK_TAB));
    assert_eq!(out.candidates.unwrap().highlighted, 1);
    assert_eq!(out.preedit, "ص");
    let out = s.on_key(&KeyEvent::shifted(VK_TAB));
    assert_eq!(out.candidates.unwrap().highlighted, 0, "Shift+Tab moves up");
    // A digit types a digit while the list is closed.
    s.on_key(&KeyEvent::plain(VK_ESCAPE));
    let out = s.on_key(&key('2'));
    assert_eq!(out.commit, "س۲");
}

#[test]
fn number_select_uses_the_highlight_list() {
    let mut s = session("");
    type_str(&mut s, "ba");
    s.on_key(&KeyEvent::plain(VK_DOWN));
    let out = s.on_key(&key('1'));
    assert_eq!(out.commit, "با");
    let mut s = session("");
    s.on_key(&key('s'));
    let out = s.on_key(&key('0'));
    assert_eq!(out.commit, "س۰", "0 never selects");
    let mut s = session("");
    s.on_key(&key('s'));
    let out = s.on_key(&KeyEvent::plain(VK_NUMPAD0 + 2));
    assert_eq!(out.commit, "ص", "numpad digits select too");
}

#[test]
fn mouse_selection() {
    let mut s = session("");
    type_str(&mut s, "baz");
    let out = s.select_candidate(9);
    assert_eq!((out.commit.as_str(), out.preedit.as_str()), ("", "باز"));
    let out = s.select_candidate(3);
    assert_eq!((out.commit.as_str(), out.preedit.as_str()), ("باظ", ""));
    assert!(!s.is_composing());
    assert_eq!(s.select_candidate(0), Output::default());
}

#[test]
fn zwnj_toggles() {
    let mut s = session("");
    s.on_key(&key('m'));
    let zw = KeyEvent::shifted(VK_SPACE);
    assert_eq!(s.on_key(&zw).preedit, "م\u{200C}");
    assert_eq!(s.on_key(&zw).preedit, "م", "second tap removes it");
    s.on_key(&KeyEvent::plain(VK_BACK));
    let out = s.on_key(&zw);
    assert_eq!(out.commit, "\u{200C}");
    assert!(out.zwnj_toggle && out.eaten);
    assert!(!s.is_composing());
}

#[test]
fn focus_and_termination() {
    let mut s = session("");
    type_str(&mut s, "salam");
    let out = s.on_focus_lost();
    assert_eq!((out.commit.as_str(), out.eaten), ("سالام", false));
    assert_eq!(s.on_focus_lost(), Output::default());
    type_str(&mut s, "sal");
    s.on_composition_terminated();
    assert!(!s.is_composing());
    assert_eq!(s.on_key(&key('b')).preedit, "ب");
}

#[test]
fn config_swap_mid_composition() {
    let mut s = session("");
    type_str(&mut s, "zac");
    s.on_key(&KeyEvent::plain(VK_UP)); // no-op: last token c has no list -> commits
    type_str(&mut s, "z");
    s.on_key(&KeyEvent::plain(VK_UP)); // ظ
    assert_eq!(s.snapshot().preedit, "ظ");
    s.set_config(Arc::new(
        load_config("[options]\nz_order = [\"ز\", \"ض\"]\n").config,
    ));
    assert_eq!(
        s.snapshot().preedit,
        "ز",
        "out-of-range choice falls back to the default"
    );
    s.set_config(Arc::new(load_config("[options]\nc = \"k\"\n").config));
    let out = type_str(&mut s, "c");
    assert_eq!(out.preedit, "زک");
}

#[test]
fn enter_modes() {
    let mut s = session("[options]\nenter = \"commit-and-pass\"\n");
    s.on_key(&key('b'));
    let out = s.on_key(&KeyEvent::plain(VK_RETURN));
    assert_eq!((out.commit.as_str(), out.eaten), ("ب", false));
    let mut s = session("");
    s.on_key(&key('b'));
    let out = s.on_key(&KeyEvent::shifted(VK_RETURN));
    assert_eq!((out.commit.as_str(), out.eaten), ("ب", true));
    assert!(
        !s.on_key(&KeyEvent::plain(VK_RETURN)).eaten,
        "idle Enter passes"
    );
}

#[test]
fn printable_pass_through_uses_us_table() {
    let mut s = session("");
    let out = s.on_key(&key('"'));
    assert_eq!((out.commit.as_str(), out.eaten), ("\"", true));
    let out = s.on_key(&key('`'));
    assert_eq!(out.commit, "`");
    let out = s.on_key(&key('!'));
    assert_eq!(out.commit, "!");
    type_str(&mut s, "ab");
    let out = s.on_key(&KeyEvent::shifted(VK_0 + 5));
    assert_eq!(out.commit, "اب٪", "Shift+5 is % -> ٪");
}

#[test]
fn non_letter_map_keys_compose() {
    let mut s =
        session("[map.singles]\n\"-\" = [\"\u{200C}\"]\n[map.digraphs]\n\"a1\" = [\"آ\"]\n");
    let out = type_str(&mut s, "a-b");
    assert_eq!(out.preedit, "ا\u{200C}ب");
    let mut s = session("[options]\nnumber_select = false\n[map.digraphs]\n\"a1\" = [\"آ\"]\n");
    let out = type_str(&mut s, "ba1");
    assert_eq!((out.preedit.as_str(), out.commit.as_str()), ("بآ", ""));
}

#[test]
fn shift_alternate_on_digraph_second_letter() {
    let mut s = session("[options]\nshift_letters = \"alternate\"\n");
    s.on_key(&key('g'));
    let out = s.on_key(&key('H'));
    assert_eq!(
        out.preedit, "ق",
        "Shift on the h of gh picks gh's second candidate"
    );
}

#[test]
fn backspace_through_everything_ends_composition() {
    let mut s = session("");
    type_str(&mut s, "sh");
    s.on_key(&KeyEvent::shifted(VK_SPACE));
    type_str(&mut s, "c");
    for _ in 0..4 {
        let out = s.on_key(&KeyEvent::plain(VK_BACK));
        assert!(out.eaten);
    }
    assert!(!s.is_composing());
    let out = s.on_key(&KeyEvent::plain(VK_BACK));
    assert!(!out.eaten, "idle Backspace passes");
}

#[test]
fn trailing_zwnj_is_not_committed() {
    let mut s = session("");
    type_str(&mut s, "m");
    s.on_key(&KeyEvent::shifted(VK_SPACE));
    let out = s.on_key(&KeyEvent::plain(VK_SPACE));
    assert_eq!(out.commit, "م ");
}

#[test]
fn out_of_range_digit_is_eaten_with_list_open() {
    let mut s = session("");
    type_str(&mut s, "s");
    let out = s.on_key(&key('5'));
    assert!(out.eaten);
    assert_eq!(out.commit, "");
    assert_eq!(out.preedit, "س");
    assert!(out.candidates.is_some());
}

#[test]
fn preedit_is_capped() {
    let mut s = session("");
    let mut committed = String::new();
    for _ in 0..(MAX_PREEDIT * 2 + 3) {
        let out = s.on_key(&key('b'));
        assert!(out.eaten);
        assert!(out.preedit.chars().count() <= MAX_PREEDIT);
        committed.push_str(&out.commit);
    }
    assert_eq!(committed.chars().count(), MAX_PREEDIT * 2);
    assert_eq!(s.snapshot().preedit.chars().count(), 3);
}

mod autocomplete {
    use super::*;
    use crate::complete::{CompleteCtx, Completion, ListCompleter};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts queries; answers like a `ListCompleter` over a fixed list.
    struct Counting {
        inner: ListCompleter,
        calls: AtomicUsize,
    }

    impl Completer for Counting {
        fn complete(&self, prefix: &str, limit: usize, ctx: &CompleteCtx) -> Vec<Completion> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.complete(prefix, limit, ctx)
        }
    }

    fn words() -> ListCompleter {
        ListCompleter::new([
            ("سلام".to_string(), 5.0),
            ("سلامت".to_string(), 4.2),
            ("سلطان".to_string(), 4.0),
        ])
    }

    fn ac_session(toml: &str) -> (Session, Arc<Counting>) {
        let mut s = session(&format!("[autocomplete]\nenabled = true\n{toml}"));
        let c = Arc::new(Counting {
            inner: words(),
            calls: AtomicUsize::new(0),
        });
        s.set_completer(Some(c.clone()));
        (s, c)
    }

    #[test]
    fn session_stays_clone_and_debug() {
        let (s, _) = ac_session("");
        let t = s.clone();
        assert!(format!("{t:?}").contains("Completer"));
    }

    #[test]
    fn would_eat_is_side_effect_free_with_completions() {
        let (mut s, _) = ac_session("");
        type_str(&mut s, "sl");
        s.on_key(&KeyEvent::plain(VK_ESCAPE));
        let keys = [
            KeyEvent::plain(VK_DOWN),
            KeyEvent::plain(VK_UP),
            KeyEvent::plain(VK_RETURN),
            KeyEvent::plain(VK_SPACE),
            KeyEvent::plain(VK_ESCAPE),
            KeyEvent::plain(VK_TAB),
            KeyEvent::plain(VK_BACK),
            key('2'),
            key('a'),
        ];
        for k in keys {
            let before = s.clone();
            let predicted = s.would_eat(&k);
            assert_eq!(s.snapshot(), before.snapshot());
            // Dismissed events collected before stay exactly as they were.
            let out = s.clone().on_key(&KeyEvent::plain(VK_ESCAPE));
            let again = before.clone().on_key(&KeyEvent::plain(VK_ESCAPE));
            assert_eq!(out, again);
            assert_eq!(s.clone().on_key(&k).eaten, predicted, "{k:?}");
        }
    }

    #[test]
    fn completer_is_queried_only_when_the_preedit_changes() {
        let (mut s, c) = ac_session("");
        type_str(&mut s, "sl");
        let after_typing = c.calls.load(Ordering::SeqCst);
        assert_eq!(after_typing, 1, "s is below predict_min_chars");
        for vk in [VK_DOWN, VK_DOWN, VK_UP] {
            s.on_key(&KeyEvent::plain(vk));
            assert!(!s.would_eat(&KeyEvent::plain(VK_SHIFT)));
        }
        let _ = s.snapshot();
        assert_eq!(c.calls.load(Ordering::SeqCst), after_typing);
        s.on_key(&key('a'));
        assert_eq!(c.calls.load(Ordering::SeqCst), after_typing + 1);
    }

    #[test]
    fn set_completer_none_restores_v1() {
        let (mut s, _) = ac_session("");
        type_str(&mut s, "sl");
        assert!(s.snapshot().completions.is_some());
        s.set_completer(None);
        assert_eq!(s.snapshot().completions, None);
        let out = s.on_key(&KeyEvent::plain(VK_DOWN));
        assert_eq!((out.commit.as_str(), out.eaten), ("سل", false));
    }

    #[test]
    fn policy_and_config_changes_apply_to_an_open_composition() {
        let (mut s, _) = ac_session("");
        type_str(&mut s, "sl");
        s.set_context_policy(ContextPolicy {
            completions: false,
            learning: false,
        });
        assert_eq!(s.snapshot().completions, None);
        s.set_context_policy(ContextPolicy::default());
        assert_eq!(s.snapshot().completions.unwrap().items.len(), 3);
        let off = load_config("").config;
        s.set_config(Arc::new(off));
        assert_eq!(s.snapshot().completions, None);
    }

    #[test]
    fn misbehaving_completer_output_is_filtered() {
        struct Bad;
        impl Completer for Bad {
            fn complete(&self, prefix: &str, _: usize, _: &CompleteCtx) -> Vec<Completion> {
                [
                    prefix.to_string(),
                    "x".into(),
                    format!("{prefix}ام"),
                    format!("{prefix}ام"),
                ]
                .into_iter()
                .chain(["ي", "ك", "ة"].map(|a| format!("{prefix}{a}")))
                .chain((0..20).map(|i| format!("{prefix}{}", "ب".repeat(i + 2))))
                .map(|text| Completion {
                    text,
                    score: 1.0,
                    edits: 0,
                })
                .collect()
            }
        }
        let mut s = session("[autocomplete]\nenabled = true\n");
        s.set_completer(Some(Arc::new(Bad)));
        let out = type_str(&mut s, "sl");
        let list = out.completions.unwrap();
        assert_eq!(list.items.len(), 5);
        assert_eq!(list.items[0], "سلام");
        assert!(list.items[1..].iter().all(|w| w.starts_with("سلبب")));
        assert_eq!(list.edits, [0; 5]);
    }

    #[test]
    fn fuzzy_rows_are_filtered_but_need_not_extend_the_preedit() {
        struct Fuzzy;
        impl Completer for Fuzzy {
            fn complete(&self, prefix: &str, _: usize, _: &CompleteCtx) -> Vec<Completion> {
                [prefix, "", "سالم", "سليم", "كتاب", "خانة", "سالم", "سرد"]
                    .into_iter()
                    .map(|t| Completion {
                        text: t.to_string(),
                        score: 1.0,
                        edits: 1,
                    })
                    .collect()
            }
        }
        let mut s = session("[autocomplete]\nenabled = true\n");
        s.set_completer(Some(Arc::new(Fuzzy)));
        let list = type_str(&mut s, "sl").completions.unwrap();
        assert_eq!(list.items, ["سالم", "سرد"]);
        assert_eq!(list.edits, [1, 1]);
    }

    #[test]
    fn completer_sees_the_configured_ctx() {
        struct Spy(std::sync::Mutex<Option<CompleteCtx>>);
        impl Completer for Spy {
            fn complete(&self, _: &str, _: usize, ctx: &CompleteCtx) -> Vec<Completion> {
                *self.0.lock().unwrap() = Some(*ctx);
                Vec::new()
            }
        }
        let spy = Arc::new(Spy(Default::default()));
        let mut s = session("[autocomplete]\nenabled = true\n");
        s.set_completer(Some(spy.clone()));
        type_str(&mut s, "sl");
        assert_eq!(*spy.0.lock().unwrap(), Some(CompleteCtx::default()));
        let cfg = load_config(
            "[autocomplete]\nenabled = true\nlearning = false\nfuzzy = false\n\
             fuzzy_min_chars = 4\nfilter_profanity = false\n",
        )
        .config;
        s.set_config(Arc::new(cfg));
        assert_eq!(
            *spy.0.lock().unwrap(),
            Some(CompleteCtx {
                use_history: false,
                fuzzy: false,
                fuzzy_min_chars: 4,
                hide_flagged: false,
            })
        );
    }
}
