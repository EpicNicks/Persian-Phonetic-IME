//! Generated dictionaries checked against brute-force rankings of the contract (3.1, 3.3, 13):
//! a large one that needs TOP sections (> 256 keys under one prefix), with and without
//! history and flagged words, and a smaller one for fuzzy matching (small enough that the
//! 4096-step work bound of section 13 never truncates a query, so results must be exact).

mod common;

use common::{build_dict, check_rows, default_build_options, max_edits, prefix_distance, rank};
use pime_core::{match_key, Completion, LearnEvent, LearnKind};
use pime_dict::{Day, Dictionary, History, ScoreParams};
use std::collections::BTreeMap;

const LETTERS: [char; 8] = ['ا', 'ب', 'ر', 'م', 'ن', 'ی', 'ک', 'ت'];
/// Letters used only for typos: never in a generated word.
const TYPO_LETTERS: [char; 2] = ['ژ', 'ث'];

struct Gen(u64);
impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// `(word, zipf with one decimal, flagged)`, unique words.
type Words = Vec<(String, f32, bool)>;

fn generate(
    seed: u64,
    count: usize,
    extra: std::ops::RangeInclusive<u64>,
    flag_every: usize,
) -> Words {
    let mut g = Gen(seed);
    let mut out: BTreeMap<String, f32> = BTreeMap::new();
    let span = extra.end() - extra.start() + 1;
    while out.len() < count {
        let stem = ["سل", "سب", "کت", "م"][g.below(4)];
        let len = extra.start() + g.next() % span;
        let mut w = stem.to_string();
        for _ in 0..len {
            w.push(LETTERS[g.below(8)]);
        }
        let zipf = (10 + g.next() % 60) as f32 / 10.0;
        out.entry(w).or_insert(zipf);
    }
    out.into_iter()
        .enumerate()
        .map(|(i, (w, z))| (w, z, flag_every != 0 && i % flag_every == 3))
        .collect()
}

/// Heavy under "سل" and "سب" (TOP sections), unflagged.
fn words() -> Words {
    generate(0x2545_F491_4F6C_DD1D, 3000, 1..=4, 0)
}

fn tsv(words: &Words) -> String {
    words
        .iter()
        .map(|(w, z, f)| format!("{w}\t{z:.1}{}\n", if *f { "\tp" } else { "" }))
        .collect()
}

fn prior(z: f32) -> f64 {
    f64::from((z * 10.0).round()) / 10.0
}

struct Query<'a> {
    prefix: &'a str,
    limit: usize,
    /// `Some(fuzzy_min_chars)` when fuzzy matching is on.
    fuzzy: Option<usize>,
    hide_flagged: bool,
}

/// The contract's ranking by brute force: `(text, edits)` rows.
fn brute(words: &Words, q: &Query, learned: &dyn Fn(&str) -> f64) -> Vec<(String, u8)> {
    let k = match_key(q.prefix);
    if k.is_empty() || q.limit == 0 {
        return Vec::new();
    }
    let visible = || words.iter().filter(|(_, _, f)| !(q.hide_flagged && *f));
    let mut exact: Vec<(f64, f64, &str)> = visible()
        .filter(|(w, _, _)| w.starts_with(&k) && *w != k)
        .map(|(w, z, _)| (prior(*z) + learned(w), prior(*z), w.as_str()))
        .collect();
    exact.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(b.1.total_cmp(&a.1))
            .then(a.2.chars().count().cmp(&b.2.chars().count()))
            .then(a.2.cmp(b.2))
    });
    let mut out: Vec<(String, u8)> = exact
        .into_iter()
        .take(q.limit)
        .map(|(_, _, w)| (w.to_string(), 0))
        .collect();
    let n = k.chars().count();
    if out.len() < q.limit && q.fuzzy.is_some_and(|min| n >= min) {
        let mut fz: Vec<(usize, f64, &str)> = visible()
            .filter(|(w, _, _)| *w != k)
            .filter_map(|(w, z, _)| {
                let d = prefix_distance(&k, w);
                (1..=max_edits(n))
                    .contains(&d)
                    .then(|| (d, prior(*z) + learned(w), w.as_str()))
            })
            .collect();
        fz.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(b.1.total_cmp(&a.1))
                .then(a.2.chars().count().cmp(&b.2.chars().count()))
                .then(a.2.cmp(b.2))
        });
        let room = q.limit - out.len();
        out.extend(
            fz.into_iter()
                .take(room)
                .map(|(d, _, w)| (w.to_string(), d as u8)),
        );
    }
    out
}

fn run(dict: &Dictionary, q: &Query, history: Option<&History>) -> Vec<Completion> {
    let params = ScoreParams::default();
    let ctx = rank(
        history,
        Day(20_000),
        &params,
        q.fuzzy.is_some(),
        q.fuzzy.unwrap_or(3),
        q.hide_flagged,
    );
    dict.complete(q.prefix, q.limit, &ctx)
}

fn rows(v: &[Completion]) -> Vec<(String, u8)> {
    v.iter().map(|c| (c.text.clone(), c.edits)).collect()
}

fn texts(v: &[Completion]) -> Vec<String> {
    v.iter().map(|c| c.text.clone()).collect()
}

fn prefixes(words: &Words) -> Vec<String> {
    let mut v: Vec<String> = words
        .iter()
        .flat_map(|(w, _, _)| (1..=4).map(move |n| w.chars().take(n).collect::<String>()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// One seeded typo (substitution, deletion, insertion or adjacent transposition).
fn typo(g: &mut Gen, s: &str) -> String {
    let mut cs: Vec<char> = s.chars().collect();
    if cs.is_empty() {
        return String::new();
    }
    let letter = |g: &mut Gen| {
        if g.below(4) == 0 {
            TYPO_LETTERS[g.below(2)]
        } else {
            LETTERS[g.below(8)]
        }
    };
    let at = g.below(cs.len());
    match g.below(4) {
        0 => cs[at] = letter(g),
        1 => {
            cs.remove(at);
        }
        2 => {
            let c = letter(g);
            cs.insert(g.below(cs.len() + 1), c);
        }
        _ if cs.len() >= 2 => {
            let i = at.min(cs.len() - 2);
            cs.swap(i, i + 1);
        }
        _ => cs[at] = letter(g),
    }
    cs.into_iter().collect()
}

fn assert_none(bad: &[String]) {
    assert!(
        bad.is_empty(),
        "{} mismatches, first ones:\n{}",
        bad.len(),
        bad.iter().take(4).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn matches_brute_force_without_history() {
    let words = words();
    let (dict, report) = build_dict(&[&tsv(&words)], &default_build_options()).unwrap();
    assert_eq!(report.kept, words.len());
    let mut bad = Vec::new();
    for p in prefixes(&words) {
        for limit in [1, 5, 9] {
            let q = Query {
                prefix: &p,
                limit,
                fuzzy: None,
                hide_flagged: true,
            };
            let got = rows(&run(&dict, &q, None));
            let want = brute(&words, &q, &|_| 0.0);
            if got != want {
                bad.push(format!("{p} limit {limit}: expected {want:?}, got {got:?}"));
            }
        }
    }
    assert_none(&bad);
}

#[test]
fn matches_brute_force_with_accepts_and_rejections() {
    let words = words();
    let (dict, _) = build_dict(&[&tsv(&words)], &default_build_options()).unwrap();
    let params = ScoreParams::default();
    let today = Day(20_000);
    let mut h = History::new();
    // Twice-accepted words get +1.5*log2(3) (not a multiple of 0.1, so no ties with priors);
    // twice-dismissed words (well under the 55-of-64 approximation bound) get -log2(3).
    let mut boosted = Vec::new();
    let mut dismissed = Vec::new();
    for (i, (w, _, _)) in words.iter().enumerate() {
        let kind = if i % 97 == 0 {
            boosted.push(w.clone());
            LearnKind::Accepted
        } else if i % 89 == 0 {
            dismissed.push(w.clone());
            LearnKind::Dismissed
        } else {
            continue;
        };
        for _ in 0..2 {
            let ev = LearnEvent {
                word: w.clone(),
                kind,
            };
            h.record(&ev, today, &params);
        }
    }
    let learned = |w: &str| {
        if boosted.iter().any(|b| b == w) {
            1.5 * 3f64.log2()
        } else if dismissed.iter().any(|d| d == w) {
            -(3f64.log2())
        } else {
            0.0
        }
    };
    let mut bad = Vec::new();
    for p in prefixes(&words) {
        let q = Query {
            prefix: &p,
            limit: 9,
            fuzzy: None,
            hide_flagged: true,
        };
        let got = rows(&run(&dict, &q, Some(&h)));
        let want = brute(&words, &q, &learned);
        if got != want {
            bad.push(format!("{p}: expected {want:?}, got {got:?}"));
        }
    }
    assert_none(&bad);
}

#[test]
fn accepted_rare_word_surfaces_under_a_crowded_prefix() {
    let words = words();
    let (dict, _) = build_dict(&[&tsv(&words)], &default_build_options()).unwrap();
    let params = ScoreParams::default();
    let rare = words
        .iter()
        .filter(|(w, _, _)| w.starts_with("سل"))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0
        .clone();
    let mut h = History::new();
    for _ in 0..20 {
        h.record_accept(&rare, Day(20_000), &params);
    }
    let q = Query {
        prefix: "س",
        limit: 9,
        fuzzy: None,
        hide_flagged: true,
    };
    let got = run(&dict, &q, Some(&h));
    assert_eq!(got.first().map(|c| c.text.as_str()), Some(rare.as_str()));
}

#[test]
fn flagged_words_with_top_sections_match_brute_force() {
    let words = generate(0x2545_F491_4F6C_DD1D, 3000, 1..=4, 10);
    let (dict, report) = build_dict(&[&tsv(&words)], &default_build_options()).unwrap();
    assert_eq!(report.kept, words.len());
    assert_eq!(report.flagged, words.iter().filter(|w| w.2).count());
    let mut bad = Vec::new();
    for p in prefixes(&words) {
        for hide_flagged in [true, false] {
            let q = Query {
                prefix: &p,
                limit: 9,
                fuzzy: None,
                hide_flagged,
            };
            let got = rows(&run(&dict, &q, None));
            let want = brute(&words, &q, &|_| 0.0);
            if got != want {
                bad.push(format!(
                    "{p} hide {hide_flagged}: expected {want:?}, got {got:?}"
                ));
            }
        }
    }
    assert_none(&bad);
}

/// Words of 3..=8 letters, every 10th flagged; small enough for unbounded fuzzy walks.
fn fuzzy_words() -> Words {
    generate(0x9E37_79B9_7F4A_7C15, 400, 1..=6, 10)
}

fn fuzzy_queries(words: &Words) -> Vec<String> {
    let mut g = Gen(0xD1B5_4A32_D192_ED03);
    let mut v = Vec::new();
    for (i, (w, _, _)) in words.iter().enumerate() {
        if i % 3 != 0 {
            continue;
        }
        let n = w.chars().count();
        for l in 2..=n.min(8) {
            let p: String = w.chars().take(l).collect();
            let once = typo(&mut g, &p);
            let twice = typo(&mut g, &once);
            v.extend([p, once, twice]);
        }
    }
    v.sort();
    v.dedup();
    v
}

#[test]
fn fuzzy_matches_brute_force() {
    fuzzy_brute_force(&[(5, 3, true), (9, 2, false), (1, 3, true)], false);
}

/// Section 13: "History boosts and penalties apply to the score as usual", so a boosted
/// fuzzy word must rank as the brute force says even among many candidates of equal edits.
#[test]
fn fuzzy_with_history_matches_brute_force() {
    fuzzy_brute_force(&[(9, 3, true), (1, 3, false)], true);
}

/// `configs`: `(limit, fuzzy_min_chars, hide_flagged)`.
fn fuzzy_brute_force(configs: &[(usize, usize, bool)], hist: bool) {
    let words = fuzzy_words();
    let (dict, report) = build_dict(&[&tsv(&words)], &default_build_options()).unwrap();
    assert_eq!(report.kept, words.len());
    let params = ScoreParams::default();
    let today = Day(20_000);
    let mut h = History::new();
    let mut learned_map: BTreeMap<String, f64> = BTreeMap::new();
    for (i, (w, _, _)) in words.iter().enumerate() {
        let (kind, l) = match i % 13 {
            0 => (LearnKind::Accepted, 1.5 * 3f64.log2()),
            5 => (LearnKind::Dismissed, -(3f64.log2())),
            _ => continue,
        };
        for _ in 0..2 {
            let ev = LearnEvent {
                word: w.clone(),
                kind,
            };
            h.record(&ev, today, &params);
        }
        learned_map.insert(w.clone(), l);
    }
    let learned = |w: &str| learned_map.get(w).copied().unwrap_or(0.0);
    let queries = fuzzy_queries(&words);
    let mut bad = Vec::new();
    let mut fuzzy_rows = [0usize; 3];
    for p in &queries {
        for &(limit, min, hide_flagged) in configs {
            let q = Query {
                prefix: p,
                limit,
                fuzzy: Some(min),
                hide_flagged,
            };
            let got_rows = run(&dict, &q, hist.then_some(&h));
            for c in &got_rows {
                fuzzy_rows[usize::from(c.edits.min(2))] += 1;
            }
            let got = rows(&got_rows);
            let want = if hist {
                brute(&words, &q, &learned)
            } else {
                brute(&words, &q, &|_| 0.0)
            };
            if got != want {
                bad.push(format!(
                    "{p:?} limit {limit} min {min} hide {hide_flagged}: \
                     expected {want:?}, got {got:?}"
                ));
            }
        }
    }
    eprintln!(
        "fuzzy brute force: {} queries, rows by edits {fuzzy_rows:?}",
        queries.len()
    );
    assert!(
        fuzzy_rows[1] > 50 && fuzzy_rows[2] > 10,
        "too few fuzzy rows: {fuzzy_rows:?}"
    );
    assert_none(&bad);
}

#[test]
fn fuzzy_off_and_fuzzy_on_agree_on_exact_rows() {
    let words = generate(0x2545_F491_4F6C_DD1D, 3000, 1..=6, 10);
    let (dict, _) = build_dict(&[&tsv(&words)], &default_build_options()).unwrap();
    let mut bad = Vec::new();
    let queries = fuzzy_queries(&words);
    for p in &queries {
        for limit in [1, 5, 9] {
            let on = Query {
                prefix: p,
                limit,
                fuzzy: Some(3),
                hide_flagged: true,
            };
            let off = Query { fuzzy: None, ..on };
            let got_on = run(&dict, &on, None);
            let got_off = run(&dict, &off, None);
            let exact = got_on.iter().take_while(|c| c.edits == 0).count();
            if texts(&got_on[..exact]) != texts(&got_off) {
                bad.push(format!(
                    "{p:?} limit {limit}: exact rows differ with fuzzy on"
                ));
            }
            if got_off.len() == limit && exact != got_on.len() {
                bad.push(format!(
                    "{p:?} limit {limit}: fuzzy ran with a full exact list"
                ));
            }
            for e in check_rows(p, &got_on, limit, Some(3)) {
                bad.push(format!("{p:?} limit {limit}: {e}"));
            }
        }
    }
    assert_none(&bad);
}
