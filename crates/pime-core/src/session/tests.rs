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
