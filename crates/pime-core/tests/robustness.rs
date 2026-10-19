//! Seeded random robustness tests (stand-in for cargo-fuzz, which needs
//! nightly): random key/modifier/focus sequences and random or garbage configs.

use pime_core::keys::*;
use pime_core::{
    load_config, load_config_with_base, Dictionary, KeyEvent, Output, ResolvedConfig, Session,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// xorshift64*: small, deterministic, good enough for test generation.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

const KEYS: &[&str] = &[
    "a", "b", "c", "s", "z", "gh", "sh", "q", "'", "tch", "xyz", "-", "1", "S", "", "ch", "kh",
    "aa", "é", "a1", "abcd", "\"", "e", "h",
];
const CANDIDATES: &[&str] = &[
    "ا",
    "آ",
    "س",
    "ص",
    "ث",
    "ق",
    "غ",
    "ي",
    "ك",
    "ة",
    "\u{200C}",
    "لا",
    "ا\u{0653}",
    "B",
    "",
    "x",
    "۱",
    "\u{FDF2}",
    "\u{0007}",
    "ب",
    "پ",
];
const OPTION_NAMES: &[&str] = &[
    "gh",
    "q",
    "c",
    "kh",
    "ch",
    "s_order",
    "z_order",
    "digits",
    "punctuation",
    "guillemets",
    "shift_letters",
    "space",
    "enter",
    "number_select",
    "candidate_labels",
    "allow_arabic_forms",
    "allow_sequences",
    "diigts",
    "bogus",
];
const WORDS: &[&str] = &[
    "ghain",
    "qaf",
    "none",
    "k",
    "s",
    "khe",
    "che",
    "persian",
    "ascii",
    "angle",
    "brace",
    "lowercase",
    "alternate",
    "preedit",
    "top-candidate",
    "commit",
    "commit-and-pass",
    "kaf",
];

fn toml_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn random_value(rng: &mut Rng) -> String {
    match rng.below(6) {
        0 => toml_string(rng.pick(WORDS)),
        1 => (if rng.chance(50) { "true" } else { "false" }).to_string(),
        2 => format!("{}", rng.below(10) as i64 - 2),
        3 | 4 => {
            let n = rng.below(5);
            let items: Vec<String> = (0..n).map(|_| toml_string(rng.pick(CANDIDATES))).collect();
            format!("[{}]", items.join(", "))
        }
        _ => toml_string(rng.pick(CANDIDATES)),
    }
}

fn random_config(rng: &mut Rng) -> String {
    let mut out = String::new();
    if rng.chance(20) {
        out.push_str(&format!(
            "preset = {}\n",
            toml_string(rng.pick(&["minimal", "qamooscheh", "nope"]))
        ));
    }
    if rng.chance(70) {
        out.push_str("[options]\n");
        let mut used = BTreeSet::new();
        for _ in 0..rng.below(6) {
            let name = rng.pick(OPTION_NAMES);
            if used.insert(name) {
                out.push_str(&format!("{name} = {}\n", random_value(rng)));
            }
        }
    }
    for section in ["singles", "digraphs", "trigraphs"] {
        if rng.chance(50) {
            out.push_str(&format!("[map.{section}]\n"));
            let mut used = BTreeSet::new();
            for _ in 0..rng.below(5) {
                let key = rng.pick(KEYS);
                if used.insert(key) {
                    out.push_str(&format!("{} = {}\n", toml_string(key), random_value(rng)));
                }
            }
        }
    }
    if rng.chance(15) {
        // Damage it: delete, duplicate or insert random characters.
        let mut chars: Vec<char> = out.chars().collect();
        for _ in 0..1 + rng.below(3) {
            let at = rng.below(chars.len() + 1);
            match rng.below(3) {
                0 if at < chars.len() => {
                    chars.remove(at);
                }
                _ => chars.insert(
                    at,
                    rng.pick(&['"', '[', ']', '=', '\n', '#', '\'', 'x', '\0']),
                ),
            }
        }
        out = chars.into_iter().collect();
    }
    out
}

fn assert_usable(cfg: &ResolvedConfig, context: &str) {
    assert!((1..=3).contains(&cfg.max_key_len()), "{context}");
    for key in cfg.keys() {
        let list = cfg.candidates(key).unwrap();
        assert!(!list.is_empty(), "{context}: empty list for {key:?}");
        assert!(
            list.iter().all(|c| !c.is_empty()),
            "{context}: empty candidate for {key:?}"
        );
        assert!(
            (1..=3).contains(&key.chars().count()),
            "{context}: key {key:?}"
        );
    }
    let mut s = Session::new(Arc::new(cfg.clone()), Arc::new(Dictionary::empty()));
    for c in "salam".chars() {
        s.on_key(&KeyEvent::plain(VK_A + (c as u32 - 'a' as u32)));
    }
    assert!(!s.snapshot().preedit.is_empty(), "{context}: cannot type");
}

#[test]
fn random_configs_always_resolve() {
    let mut rng = Rng::new(0xC0FFEE);
    let mut seen = BTreeSet::new();
    for i in 0..3000 {
        let src = random_config(&mut rng);
        let loaded = load_config(&src);
        seen.insert(format!("{:?}", pime_core::fallback_of(&loaded.issues)));
        let context = format!("case {i}:\n{src}");
        assert_usable(&loaded.config, &context);
        let lines = src.split('\n').count();
        for issue in &loaded.issues {
            assert!(!issue.message.is_empty(), "{context}");
            assert!(
                issue.line <= lines,
                "{context}: line {} of {lines}",
                issue.line
            );
        }
    }
    assert_eq!(
        seen.len(),
        3,
        "generator should hit every fallback kind: {seen:?}"
    );
}

#[test]
fn garbage_configs_never_panic() {
    let mut rng = Rng::new(42);
    let alphabet: Vec<char> = "[]{}=\"'#.,\n\t \\abcghqsz019_-ا\u{200C}ي\u{0}\u{7F}\u{FEFF}"
        .chars()
        .collect();
    for i in 0..5000 {
        let len = rng.below(80);
        let src: String = (0..len).map(|_| rng.pick(&alphabet)).collect();
        let base = rng.pick(&["qamooscheh", "minimal", "unknown"]);
        let loaded = load_config_with_base(base, &src);
        assert_usable(&loaded.config, &format!("garbage {i}: {src:?}"));
        let _ = pime_core::format_issues(&loaded.issues);
    }
}

const VKS: &[u32] = &[
    VK_BACK,
    VK_TAB,
    VK_RETURN,
    VK_SHIFT,
    VK_CONTROL,
    VK_MENU,
    VK_CAPITAL,
    VK_ESCAPE,
    VK_SPACE,
    VK_PRIOR,
    VK_NEXT,
    VK_END,
    VK_HOME,
    VK_LEFT,
    VK_UP,
    VK_RIGHT,
    VK_DOWN,
    VK_INSERT,
    VK_DELETE,
    VK_LWIN,
    VK_APPS,
    VK_NUMPAD0,
    VK_NUMPAD0 + 5,
    VK_MULTIPLY,
    VK_DECIMAL,
    VK_LSHIFT,
    VK_RMENU,
    VK_OEM_1,
    VK_OEM_PLUS,
    VK_OEM_COMMA,
    VK_OEM_MINUS,
    VK_OEM_PERIOD,
    VK_OEM_2,
    VK_OEM_3,
    VK_OEM_4,
    VK_OEM_5,
    VK_OEM_6,
    VK_OEM_7,
    VK_OEM_102,
    0x70,
    0xFF,
    0,
    u32::MAX,
];

fn random_key(rng: &mut Rng) -> KeyEvent {
    let vk = match rng.below(10) {
        0..=4 => VK_A + rng.below(26) as u32,
        5 => VK_0 + rng.below(10) as u32,
        6..=8 => rng.pick(VKS),
        _ => rng.next() as u32 & 0x1FF,
    };
    KeyEvent {
        vk,
        shift: rng.chance(25),
        ctrl: rng.chance(4),
        alt: rng.chance(3),
    }
}

fn allowed_chars(cfg: &ResolvedConfig) -> BTreeSet<char> {
    let mut set: BTreeSet<char> = (' '..='~').collect();
    set.insert('\u{200C}');
    for key in cfg.keys() {
        for c in cfg.candidates(key).unwrap() {
            set.extend(c.chars());
        }
    }
    for c in ' '..='~' {
        if let Some(s) = cfg.convert_char(c) {
            set.extend(s.chars());
        }
    }
    set
}

fn check_output(out: &Output, s: &Session, allowed: &BTreeSet<char>, ctx: &str) {
    for c in out.commit.chars() {
        assert!(
            allowed.contains(&c),
            "{ctx}: committed {c:?} ({:04X})",
            c as u32
        );
    }
    assert!(
        !out.preedit.chars().any(char::is_control),
        "{ctx}: control char in preedit"
    );
    assert_eq!(
        out.preedit.is_empty(),
        !s.is_composing(),
        "{ctx}: preedit vs composing"
    );
    let units: Vec<u16> = out.preedit.encode_utf16().collect();
    if let Some(r) = &out.preedit_pending {
        assert!(
            r.start < r.end && r.end <= units.len(),
            "{ctx}: pending {r:?}"
        );
        assert!(
            String::from_utf16(&units[r.start..r.end]).is_ok(),
            "{ctx}: split surrogate"
        );
        assert!(
            String::from_utf16(&units[..r.start]).is_ok(),
            "{ctx}: split surrogate"
        );
    }
    if let Some(list) = &out.candidates {
        assert!(list.items.len() >= 2, "{ctx}");
        assert!(list.highlighted < list.items.len(), "{ctx}");
        assert!(
            out.preedit.ends_with(&list.items[list.highlighted]),
            "{ctx}: highlight not shown"
        );
    }
    if !out.commit.is_empty() || out.zwnj_toggle {
        assert!(
            out.preedit.is_empty(),
            "{ctx}: commit leaves no composition"
        );
    }
}

#[test]
fn random_key_sequences_keep_invariants() {
    let mut rng = Rng::new(7);
    let mut configs: Vec<ResolvedConfig> = vec![
        ResolvedConfig::default(),
        ResolvedConfig::builtin("minimal").unwrap(),
    ];
    for _ in 0..30 {
        let src = random_config(&mut rng);
        configs.push(load_config(&src).config);
    }
    let (mut commits, mut lists, mut pending) = (0, 0, 0);
    for seq in 0..1500 {
        let mut cfg = configs[rng.below(configs.len())].clone();
        let mut allowed = allowed_chars(&cfg);
        let mut s = Session::new(Arc::new(cfg.clone()), Arc::new(Dictionary::empty()));
        for step in 0..1 + rng.below(80) {
            let ctx = format!("seq {seq} step {step}");
            let out = match rng.below(100) {
                0 => s.on_focus_lost(),
                1 => {
                    s.on_composition_terminated();
                    s.snapshot()
                }
                2 => s.select_candidate(rng.below(5)),
                3 => {
                    cfg = configs[rng.below(configs.len())].clone();
                    allowed = allowed_chars(&cfg);
                    s.set_config(Arc::new(cfg.clone()));
                    s.snapshot()
                }
                _ => {
                    let k = random_key(&mut rng);
                    let predicted = s.would_eat(&k);
                    let out = s.on_key(&k);
                    assert_eq!(predicted, out.eaten, "{ctx}: would_eat({k:?})");
                    out
                }
            };
            check_output(&out, &s, &allowed, &ctx);
            commits += usize::from(!out.commit.is_empty());
            lists += usize::from(out.candidates.is_some());
            pending += usize::from(out.preedit_pending.is_some());
        }
        // Backspace always empties the composition, one Latin char at a time.
        let mut guard = 0;
        while s.is_composing() {
            let out = s.on_key(&KeyEvent::plain(VK_BACK));
            assert!(out.eaten && out.commit.is_empty(), "seq {seq}: backspace");
            guard += 1;
            assert!(
                guard < 1000,
                "seq {seq}: backspace never ended the composition"
            );
        }
        let out = s.snapshot();
        assert!(out.preedit.is_empty() && out.candidates.is_none());
        assert!(
            !s.on_key(&KeyEvent::plain(VK_BACK)).eaten,
            "seq {seq}: idle backspace"
        );
    }
    assert!(
        commits > 1000 && lists > 1000 && pending > 100,
        "{commits} {lists} {pending}"
    );
}
