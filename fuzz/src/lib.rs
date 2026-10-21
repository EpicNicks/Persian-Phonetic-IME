//! Fuzz invariants as plain functions, so they run under libFuzzer and, via
//! `tests/seeds.rs`, over the seed corpus without it.

use pime_core::keys::*;
use pime_core::{
    load_config, load_config_bytes, Dictionary, KeyEvent, Output, ResolvedConfig, Session,
};
use std::sync::Arc;

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
    VK_MULTIPLY,
    VK_DECIMAL,
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
];

/// Arabic-only forms that must not appear unless `allow_arabic_forms` is on.
const ARABIC_ONLY: [char; 3] = ['\u{064A}', '\u{0643}', '\u{0629}'];

fn decode_key(b0: u8, b1: u8) -> KeyEvent {
    let pick = b1 as u32 >> 3;
    let vk = match b0 % 4 {
        0 | 1 => VK_A + pick % 26,
        2 => VK_0 + pick % 10,
        _ => VKS[pick as usize % VKS.len()],
    };
    KeyEvent {
        vk,
        shift: b1 & 1 != 0,
        ctrl: b1 & 2 != 0 && b0 & 0x80 != 0,
        alt: b1 & 4 != 0 && b0 & 0x40 != 0,
    }
}

fn check_output(out: &Output, s: &Session, allow_arabic: bool) {
    if !allow_arabic {
        for c in out.commit.chars().chain(out.preedit.chars()) {
            assert!(
                !ARABIC_ONLY.contains(&c),
                "Arabic-only form U+{:04X}",
                c as u32
            );
        }
    }
    assert_eq!(
        out.preedit.is_empty(),
        !s.is_composing(),
        "preedit vs composing"
    );
    assert!(
        !out.preedit.chars().any(char::is_control),
        "control char in preedit"
    );
    if !out.commit.is_empty() || out.zwnj_toggle {
        assert!(out.preedit.is_empty(), "commit must end composition");
    }
    // Composition is bounded by the longest key, never by how much was typed.
    assert!(out.preedit.chars().count() <= 64, "preedit grows unbounded");
    assert!(
        out.commit.chars().count() <= 64,
        "commit unbounded per step"
    );
    if let Some(r) = &out.preedit_pending {
        let units: Vec<u16> = out.preedit.encode_utf16().collect();
        assert!(r.start < r.end && r.end <= units.len(), "bad pending range");
        assert!(
            String::from_utf16(&units[r.start..r.end]).is_ok(),
            "split surrogate"
        );
    }
    if let Some(list) = &out.candidates {
        assert!(
            list.highlighted < list.items.len(),
            "highlight out of range"
        );
        assert!(list.items.len() <= 64, "candidate list unbounded");
    }
}

/// Byte 0 picks the preset; then 2 bytes per event (a key, or rarely a focus event).
pub fn check_session(data: &[u8]) {
    let Some((&first, rest)) = data.split_first() else {
        return;
    };
    let cfg = match first % 3 {
        0 => ResolvedConfig::default(),
        1 => ResolvedConfig::builtin("minimal").unwrap_or_default(),
        _ => ResolvedConfig::builtin("qamooscheh").unwrap_or_default(),
    };
    let allow_arabic = cfg.options().allow_arabic_forms;
    let mut s = Session::new(Arc::new(cfg), Arc::new(Dictionary::empty()));
    for pair in rest.chunks(2) {
        let (b0, b1) = (pair[0], pair.get(1).copied().unwrap_or(0));
        let out = match b0 {
            0xF0 => s.on_focus_lost(),
            0xF1 => {
                s.on_composition_terminated();
                s.snapshot()
            }
            0xF2 => s.select_candidate(b1 as usize),
            _ => {
                let k = decode_key(b0, b1);
                let _ = s.would_eat(&k);
                s.on_key(&k)
            }
        };
        check_output(&out, &s, allow_arabic);
    }
    s.on_focus_lost();
    assert!(!s.is_composing(), "focus loss left composition");
}

/// Arbitrary bytes as a config file: never panics, always yields a usable config.
pub fn check_config(data: &[u8]) {
    let loaded = load_config_bytes(data);
    assert_usable(&loaded.config);
    if let Ok(text) = std::str::from_utf8(data) {
        assert_usable(&load_config(text).config);
    }
    let _ = pime_core::format_issues(&loaded.issues);
}

fn assert_usable(cfg: &ResolvedConfig) {
    assert!((1..=3).contains(&cfg.max_key_len()));
    for key in cfg.keys() {
        assert!(cfg.candidates(key).is_some_and(|l| !l.is_empty()));
    }
    let mut s = Session::new(Arc::new(cfg.clone()), Arc::new(Dictionary::empty()));
    for c in "salam".chars() {
        s.on_key(&KeyEvent::plain(VK_A + (c as u32 - 'a' as u32)));
    }
    assert!(s.is_composing(), "cannot type with resolved config");
}

// ---- Autocomplete (docs/autocomplete.md): ListCompleter sessions, dict.bin, history.bin ----

use pime_core::{
    completable, load_config_with_base, match_key, Completer, Completion, ContextPolicy,
    LearnEvent, LearnKind, ListCompleter, ZWNJ,
};
use pime_dict::{build_from_tsv, BuildOptions, Day, History, RankCtx, ScoreParams};

/// Normalised words (as pime-dict would emit them), mostly reachable from preset letters.
const VOCAB: &[&str] = &[
    "سلام",
    "سلامت",
    "سلطان",
    "سبد",
    "سیب",
    "کتاب",
    "کتابخانه",
    "کار",
    "کردن",
    "میز",
    "میخک",
    "می\u{200C}خواهم",
    "می\u{200C}روم",
    "نمی\u{200C}دانم",
    "با",
    "باران",
    "بازار",
    "برای",
    "بهتر",
    "دست",
    "دوست",
    "خانه",
    "خوب",
    "خواهر",
    "تاریخ",
    "من",
    "ما",
    "مادر",
    "شب",
    "شهر",
    "شما",
    "روز",
    "زبان",
    "آب",
    "آمد",
    "اسم",
    "این",
    "آن\u{200C}ها",
    "هم",
    "همه",
    "گل",
    "پدر",
    "چای",
    // Arabic-only letters: the builder normalises them; a ListCompleter offering them as
    // fuzzy rows must be filtered by the session.
    "مكتب",
    "عربي",
];

/// `VOCAB` indices written with the `p` (vulgar) flag in `base_dict`.
fn flagged(i: usize) -> bool {
    i % 7 == 2
}

fn vocab_from(seed: u8) -> Vec<(String, f32)> {
    VOCAB
        .iter()
        .enumerate()
        .filter(|(i, _)| seed == 0 || !(seed as usize + i).is_multiple_of(3))
        .map(|(i, w)| (w.to_string(), ((i * 7 + seed as usize) % 50) as f32 / 10.0))
        .collect()
}

/// Like `decode_key`, but a quarter of the events are keys autocomplete changes.
fn decode_ac_key(b0: u8, b1: u8) -> KeyEvent {
    if b0 % 4 == 3 && b1 & 0x80 != 0 {
        let vk = [
            VK_DOWN,
            VK_UP,
            VK_RETURN,
            VK_SPACE,
            VK_ESCAPE,
            VK_TAB,
            VK_BACK,
            VK_0 + 1,
            VK_0 + 2,
            VK_0 + 3,
            VK_0 + 5,
            VK_0 + 9,
        ][(b1 as usize >> 1) % 12];
        return KeyEvent {
            vk,
            shift: b1 & 1 != 0 && vk == VK_TAB,
            ..Default::default()
        };
    }
    decode_key(b0, b1)
}

struct AcState {
    enabled: bool,
    learning: bool,
    min_chars: usize,
    max_completions: usize,
    fuzzy: bool,
    fuzzy_min_chars: usize,
    has_completer: bool,
    policy: ContextPolicy,
    /// Completion rows shown in the current episode; learn events may only name these.
    shown: Vec<String>,
    /// Of those, the ones shown as exact rows; only these may be Skipped/Ignored/Dismissed.
    shown_exact: Vec<String>,
}

/// Arabic-only forms a fuzzy row may never hold (docs/autocomplete.md 13).
const NEVER_IN_FUZZY: [char; 4] = ['\u{064A}', '\u{0649}', '\u{0643}', '\u{0629}'];

impl AcState {
    fn note_shown(&mut self, list: &pime_core::CompletionList) {
        for (i, item) in list.items.iter().enumerate() {
            if !self.shown.contains(item) {
                self.shown.push(item.clone());
            }
            if list.edits.get(i) == Some(&0) && !self.shown_exact.contains(item) {
                self.shown_exact.push(item.clone());
            }
        }
    }
}

fn check_completions(out: &Output, st: &mut AcState, allow_arabic: bool) {
    if let Some(list) = &out.completions {
        assert!(st.enabled && st.has_completer, "completions while off");
        assert!(st.policy.completions, "completions against policy");
        assert!(
            out.preedit_pending.is_none(),
            "completions with pending letters"
        );
        assert!(
            completable(&out.preedit),
            "completions for non-Persian preedit"
        );
        let letters = out.preedit.chars().filter(|&c| c != ZWNJ).count();
        assert!(
            letters >= st.min_chars,
            "completions before predict_min_chars"
        );
        let alts = out.candidates.as_ref().map_or(0, |c| c.items.len());
        assert!(
            list.items.len() <= st.max_completions,
            "more than max_completions"
        );
        assert!(alts + list.items.len() <= 9, "more than 9 popup rows");
        if let Some(h) = list.highlighted {
            assert!(h < list.items.len(), "completion highlight out of range");
        }
        assert_eq!(
            list.edits.len(),
            list.items.len(),
            "edits not parallel to items"
        );
        let key = match_key(&out.preedit);
        let n = key.chars().count();
        for (i, item) in list.items.iter().enumerate() {
            let edits = list.edits[i];
            assert!(!list.items[..i].contains(item), "duplicate completion");
            if i > 0 {
                assert!(list.edits[i - 1] <= edits, "fuzzy row before a closer row");
            }
            if edits == 0 {
                let tail = item
                    .strip_prefix(out.preedit.as_str())
                    .expect("exact completion does not start with the preedit");
                assert!(tail.chars().any(|c| c != ZWNJ), "empty completion");
                let checked = if allow_arabic { tail } else { item.as_str() };
                assert!(
                    !checked.chars().any(|c| ARABIC_ONLY.contains(&c)),
                    "Arabic-only form in completion"
                );
                continue;
            }
            assert!(st.fuzzy, "fuzzy row with fuzzy off");
            assert!(n >= st.fuzzy_min_chars, "fuzzy row below fuzzy_min_chars");
            assert!(
                usize::from(edits) <= pime_core::fuzzy_max_edits(n),
                "fuzzy row over the edit budget"
            );
            assert_ne!(*item, out.preedit, "fuzzy row equals the preedit");
            assert!(
                !match_key(item).starts_with(&key),
                "fuzzy row extends the preedit exactly"
            );
            assert!(
                !item.chars().any(|c| NEVER_IN_FUZZY.contains(&c)),
                "Arabic-only form in a fuzzy row"
            );
        }
        st.note_shown(list);
    }
    if !out.learn.is_empty() {
        assert!(st.learning, "learn events with learning off");
        assert!(
            !out.commit.is_empty() || out.preedit.is_empty(),
            "learn events before the episode ended"
        );
        for (i, ev) in out.learn.iter().enumerate() {
            assert!(
                st.shown.contains(&ev.word),
                "learn event for a word never shown"
            );
            assert!(
                ev.kind == LearnKind::Accepted || st.shown_exact.contains(&ev.word),
                "fuzzy row penalised ({:?})",
                ev.kind
            );
            let k = match_key(&ev.word);
            assert!(
                out.learn[..i].iter().all(|e| match_key(&e.word) != k),
                "learn events not deduplicated"
            );
        }
    }
    assert!(out.learn.len() <= 64, "learn events unbounded");
    if !out.commit.is_empty() || out.preedit.is_empty() {
        st.shown.clear();
        st.shown_exact.clear();
        if let Some(list) = &out.completions {
            st.note_shown(list);
        }
    }
}

/// Byte 0: preset and autocomplete settings; byte 1: vocabulary; then 2 bytes per event
/// (keys, focus loss, termination, clicks, policy and completer changes). Checks the v1
/// invariants plus the completion and learning rules of docs/autocomplete.md section 2.
pub fn check_session_complete(data: &[u8]) {
    let _ = session_complete_coverage(data);
}

/// [`check_session_complete`], returning how many Outputs carried completions, learn events
/// and fuzzy rows (so the seed test can show the invariants are not vacuous).
pub fn session_complete_coverage(data: &[u8]) -> (usize, usize, usize) {
    let (Some(&b0), Some(&b1)) = (data.first(), data.get(1)) else {
        return (0, 0, 0);
    };
    let mut seen = (0, 0, 0);
    let preset = if b0 & 1 == 0 { "qamooscheh" } else { "minimal" };
    let mut st = AcState {
        enabled: b0 & 2 == 0,
        learning: b0 & 4 == 0,
        min_chars: 1 + (b0 as usize >> 3) % 3,
        max_completions: 1 + (b0 as usize >> 5) % 9,
        // Byte 1 also picks the vocabulary; its top bits pick the fuzzy settings.
        fuzzy: b1 & 0x40 == 0,
        fuzzy_min_chars: 2 + usize::from(b1 >> 7),
        has_completer: true,
        policy: ContextPolicy::default(),
        shown: Vec::new(),
        shown_exact: Vec::new(),
    };
    let config = |st: &AcState| {
        let toml = format!(
            "[autocomplete]\nenabled = {}\nlearning = {}\npredict_min_chars = {}\n\
             max_completions = {}\nfuzzy = {}\nfuzzy_min_chars = {}\n",
            st.enabled, st.learning, st.min_chars, st.max_completions, st.fuzzy, st.fuzzy_min_chars
        );
        Arc::new(load_config_with_base(preset, &toml).config)
    };
    let cfg = config(&st);
    let allow_arabic = cfg.options().allow_arabic_forms;
    let completer: Arc<dyn Completer> = Arc::new(ListCompleter::new(vocab_from(b1)));
    let mut s = Session::new(cfg, Arc::new(Dictionary::empty()));
    s.set_completer(Some(completer.clone()));
    for pair in data[2..].chunks(2) {
        let (e0, e1) = (pair[0], pair.get(1).copied().unwrap_or(0));
        let out = match e0 {
            0xF0 => s.on_focus_lost(),
            0xF1 => {
                s.on_composition_terminated();
                st.shown.clear();
                st.shown_exact.clear();
                s.snapshot()
            }
            0xF2 => s.select_candidate(e1 as usize % 12),
            0xF3 => {
                st.policy = ContextPolicy {
                    completions: e1 & 1 != 0,
                    learning: e1 & 2 != 0,
                };
                s.set_context_policy(st.policy);
                continue;
            }
            0xF4 => {
                st.has_completer = !st.has_completer;
                s.set_completer(st.has_completer.then(|| completer.clone()));
                continue;
            }
            0xF5 => {
                // Live reload toggling fuzzy matching (or changing its threshold).
                if e1 & 1 == 0 {
                    st.fuzzy = !st.fuzzy;
                } else {
                    st.fuzzy_min_chars = 2 + usize::from(e1 >> 1) % 4;
                }
                s.set_config(config(&st));
                s.snapshot()
            }
            _ => {
                let k = decode_ac_key(e0, e1);
                let _ = s.would_eat(&k);
                s.on_key(&k)
            }
        };
        check_output(&out, &s, allow_arabic);
        check_completions(&out, &mut st, allow_arabic);
        seen.0 += usize::from(out.completions.is_some());
        seen.1 += usize::from(!out.learn.is_empty());
        seen.2 += out
            .completions
            .as_ref()
            .map_or(0, |l| l.edits.iter().filter(|&&e| e > 0).count());
    }
    let out = s.on_focus_lost();
    check_completions(&out, &mut st, allow_arabic);
    assert!(!s.is_composing(), "focus loss left composition");
    seen
}

/// The trait contract of a `Dictionary::complete` result (docs/autocomplete.md 2.2, 13);
/// `fuzzy` is `Some(fuzzy_min_chars)` when fuzzy matching was requested.
fn check_completion_list(prefix: &str, got: &[Completion], limit: usize, fuzzy: Option<usize>) {
    assert!(got.len() <= limit, "more than limit");
    let key = match_key(prefix);
    let n = key.chars().count();
    for (i, c) in got.iter().enumerate() {
        assert!(
            got[..i].iter().all(|o| o.text != c.text),
            "duplicate completion"
        );
        assert!(c.score.is_finite(), "non-finite score");
        if i > 0 {
            assert!(got[i - 1].edits <= c.edits, "fuzzy row before a closer row");
        }
        if c.edits == 0 {
            let tail = c
                .text
                .strip_prefix(prefix)
                .expect("completion does not extend the prefix");
            assert!(!tail.is_empty(), "completion equals the prefix");
            continue;
        }
        let min = fuzzy.expect("fuzzy row with fuzzy off");
        assert!(n >= min.max(2), "fuzzy row below fuzzy_min_chars");
        assert!(
            usize::from(c.edits) <= pime_core::fuzzy_max_edits(n),
            "fuzzy row over the edit budget"
        );
        assert_ne!(c.text, prefix, "fuzzy row equals the prefix");
        assert!(
            !match_key(&c.text).starts_with(&key),
            "fuzzy row extends the prefix exactly"
        );
        assert!(
            !c.text.chars().any(|ch| NEVER_IN_FUZZY.contains(&ch)),
            "Arabic-only letter in a fuzzy row"
        );
    }
}

fn prefixes_from(data: &[u8]) -> Vec<String> {
    let fixed = [
        "س",
        "سل",
        "سلم",
        "می",
        "می\u{200C}",
        "میخاهم",
        "مي",
        "کت",
        "كتب",
        "دوشت",
        "خانح",
        "ا",
        "\u{200C}",
        "",
        "x",
    ];
    let mut v: Vec<String> = fixed.iter().map(|s| s.to_string()).collect();
    v.extend(
        String::from_utf8_lossy(data)
            .split(['\n', '\t', ' '])
            .take(8)
            .map(|s| s.chars().take(40).collect::<String>()),
    );
    v
}

fn base_dict() -> Vec<u8> {
    let tsv: String = VOCAB
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let flag = if flagged(i) { "\tp" } else { "" };
            format!("{w}\t{}.{}{flag}\n", 1 + i % 6, i % 10)
        })
        .collect();
    let mut r = tsv.as_bytes();
    build_from_tsv(&mut [&mut r], &BuildOptions::default())
        .expect("vocabulary builds")
        .0
}

/// The query settings tried on every prefix: `(fuzzy, fuzzy_min_chars, hide_flagged)`.
const DICT_CTXS: [(bool, usize, bool); 4] = [
    (false, 3, true),
    (true, 3, true),
    (true, 2, false),
    (true, 0, true),
];

/// Arbitrary bytes, or a valid file with fuzzer-chosen bytes overwritten, as `dict.bin`:
/// validation never panics and an accepted file answers exact and fuzzy queries within the
/// trait contract; the unmodified file never shows a flagged word when they are hidden.
pub fn check_dict_bytes(data: &[u8]) {
    let mut pristine = false;
    let bytes = match data.split_first() {
        Some((&m, rest)) if m & 1 == 1 => {
            let mut b = base_dict();
            pristine = rest.len() < 3;
            for w in rest.chunks(3) {
                if let [hi, lo, v] = *w {
                    let at = ((usize::from(hi) << 8) | usize::from(lo)) % b.len();
                    b[at] = v;
                }
            }
            if m & 2 == 2 {
                let keep = rest.len() * 31 % (b.len() + 1);
                pristine &= keep == b.len();
                b.truncate(keep);
            }
            b
        }
        _ => data.to_vec(),
    };
    let Ok(dict) = pime_dict::Dictionary::from_bytes(bytes) else {
        return;
    };
    let _ = (dict.len(), dict.info());
    let params = ScoreParams::default();
    let mut h = History::new();
    h.record_accept("سلامت", Day(20_000), &params);
    h.record_accept("سلطان", Day(20_000), &params);
    let hidden: Vec<String> = VOCAB
        .iter()
        .enumerate()
        .filter(|&(i, _)| flagged(i))
        .map(|(_, w)| match_key(w))
        .collect();
    for (i, p) in prefixes_from(data).iter().enumerate() {
        let _ = dict.prior(p);
        for (fuzzy, fuzzy_min_chars, hide_flagged) in DICT_CTXS {
            let rank = RankCtx {
                history: (i % 2 == 0).then_some(&h),
                fuzzy,
                fuzzy_min_chars,
                hide_flagged,
                ..RankCtx::new(Day(20_000), &params)
            };
            for limit in [0, 1, 9, 100] {
                let got = dict.complete(p, limit, &rank);
                check_completion_list(p, &got, limit, fuzzy.then_some(fuzzy_min_chars));
                if pristine && hide_flagged {
                    assert!(
                        got.iter().all(|c| !hidden.contains(&match_key(&c.text))),
                        "flagged word shown with hide_flagged"
                    );
                }
            }
        }
    }
}

/// Arbitrary bytes as TSV: the builder fails cleanly or emits a file that loads, matches its
/// report, holds no Arabic-only letters and completes (exact and fuzzy) within the contract.
pub fn check_dict_tsv(data: &[u8]) {
    let mut r = data;
    let opts = BuildOptions {
        max_bytes: 1 << 20,
        ..BuildOptions::default()
    };
    let Ok((bytes, report)) = build_from_tsv(&mut [&mut r], &opts) else {
        return;
    };
    assert_eq!(report.bytes, bytes.len(), "report.bytes");
    let dict = pime_dict::Dictionary::from_bytes(bytes).expect("builder output must load");
    assert_eq!(dict.len(), report.kept, "len vs report.kept");
    assert!(report.kept <= report.read, "kept more than read");
    assert!(report.flagged <= report.kept, "more flagged than kept");
    let params = ScoreParams::default();
    for p in prefixes_from(data) {
        for (fuzzy, fuzzy_min_chars, hide_flagged) in DICT_CTXS {
            let rank = RankCtx {
                fuzzy,
                fuzzy_min_chars,
                hide_flagged,
                ..RankCtx::new(Day(0), &params)
            };
            let got = dict.complete(&p, 9, &rank);
            check_completion_list(&p, &got, 9, fuzzy.then_some(fuzzy_min_chars));
            for c in &got {
                let added = if c.edits == 0 {
                    &c.text[p.len()..]
                } else {
                    &c.text
                };
                assert!(
                    !added.chars().any(|c| NEVER_IN_FUZZY.contains(&c)),
                    "Arabic-only letter in built dictionary"
                );
            }
        }
    }
}

/// Arbitrary bytes as `history.bin`: parsing never panics; an accepted file round-trips,
/// stays bounded and survives record/decay/merge with finite, clamped learned terms.
pub fn check_history_bytes(data: &[u8]) {
    let Ok(mut h) = History::from_bytes(data) else {
        return;
    };
    assert!(h.len() <= 10_000, "history over the entry cap");
    let bytes = h.to_bytes();
    assert!(bytes.len() <= 2 << 20, "history file over 2 MiB");
    let again = History::from_bytes(&bytes).expect("own output must parse");
    assert_eq!(again.len(), h.len(), "round trip changed the entry count");
    assert_eq!(again.to_bytes(), bytes, "serialisation not stable");
    let p = ScoreParams::default();
    let keys: Vec<String> = h.prefix("").map(|(k, _)| k.to_string()).take(16).collect();
    let kinds = [
        LearnKind::Accepted,
        LearnKind::Skipped,
        LearnKind::Ignored,
        LearnKind::Dismissed,
    ];
    for (i, k) in keys.iter().enumerate() {
        for today in [0, 20_000, u32::MAX] {
            let l = h.learned(k, Day(today), &p);
            assert!(
                l.is_finite() && (p.learned_min..=p.learned_max).contains(&l),
                "learned term out of bounds"
            );
        }
        let ev = LearnEvent {
            word: k.clone(),
            kind: kinds[i % 4],
        };
        h.record(&ev, Day(20_000), &p);
    }
    let mut merged = History::new();
    merged.merge(&h);
    merged.merge(&again);
    merged.decay(Day(20_100), &p);
    assert!(merged.len() <= 10_000, "merge over the entry cap");
    History::from_bytes(&merged.to_bytes()).expect("merged output must parse");
}

/// A valid `history.bin` (seed for the history target).
pub fn sample_history() -> Vec<u8> {
    let p = ScoreParams::default();
    let mut h = History::new();
    for (i, w) in VOCAB.iter().enumerate() {
        if i % 2 == 0 {
            h.record_accept(w, Day(20_000 + i as u32), &p);
        } else {
            h.record_reject(w, pime_dict::Rejection::Dismissed, Day(20_000), &p);
        }
    }
    h.to_bytes()
}

/// A valid `dict.bin` (seed for the dict target).
pub fn sample_dict() -> Vec<u8> {
    base_dict()
}
