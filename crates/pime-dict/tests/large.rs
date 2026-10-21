//! A generated dictionary large enough to need TOP sections (> 256 keys under one prefix):
//! `Dictionary::complete` must equal a brute-force ranking of the contract (3.1, 3.3), with
//! and without history, and stay within the trait contract.

mod common;

use common::{build_dict, default_build_options};
use pime_core::{match_key, LearnEvent, LearnKind};
use pime_dict::{Day, History, RankCtx, ScoreParams};
use std::collections::BTreeMap;

const LETTERS: [char; 8] = ['ا', 'ب', 'ر', 'م', 'ن', 'ی', 'ک', 'ت'];

struct Gen(u64);
impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// `(word, zipf with one decimal)`, unique words; heavy under "سل" and "سب".
fn words() -> Vec<(String, f32)> {
    let mut g = Gen(0x2545_F491_4F6C_DD1D);
    let mut out: BTreeMap<String, f32> = BTreeMap::new();
    while out.len() < 3000 {
        let stem = ["سل", "سب", "کت", "م"][(g.next() % 4) as usize];
        let len = 1 + g.next() % 4;
        let mut w = stem.to_string();
        for _ in 0..len {
            w.push(LETTERS[(g.next() % 8) as usize]);
        }
        let zipf = (10 + g.next() % 60) as f32 / 10.0;
        out.entry(w).or_insert(zipf);
    }
    out.into_iter().collect()
}

fn brute(
    words: &[(String, f32)],
    prefix: &str,
    limit: usize,
    learned: &dyn Fn(&str) -> f64,
) -> Vec<String> {
    let k = match_key(prefix);
    let mut pool: Vec<(f64, f64, &str)> = words
        .iter()
        .filter(|(w, _)| w.starts_with(&k) && *w != k)
        .map(|(w, z)| {
            let prior = f64::from((z * 10.0).round()) / 10.0;
            (prior + learned(w), prior, w.as_str())
        })
        .collect();
    pool.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(b.1.total_cmp(&a.1))
            .then(a.2.chars().count().cmp(&b.2.chars().count()))
            .then(a.2.cmp(b.2))
    });
    pool.into_iter()
        .take(limit)
        .map(|(_, _, w)| w.to_string())
        .collect()
}

fn prefixes(words: &[(String, f32)]) -> Vec<String> {
    let mut v: Vec<String> = words
        .iter()
        .flat_map(|(w, _)| (1..=4).map(move |n| w.chars().take(n).collect::<String>()))
        .collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn matches_brute_force_without_history() {
    let words = words();
    let tsv: String = words
        .iter()
        .map(|(w, z)| format!("{w}\t{z:.1}\n"))
        .collect();
    let (dict, report) = build_dict(&[&tsv], &default_build_options()).unwrap();
    assert_eq!(report.kept, words.len());
    let params = ScoreParams::default();
    let rank = RankCtx {
        history: None,
        today: Day(20_000),
        params: &params,
    };
    let mut bad = Vec::new();
    for p in prefixes(&words) {
        for limit in [1, 5, 9] {
            let got: Vec<String> = dict
                .complete(&p, limit, &rank)
                .into_iter()
                .map(|c| c.text)
                .collect();
            let want = brute(&words, &p, limit, &|_| 0.0);
            if got != want {
                bad.push(format!("{p} limit {limit}: expected {want:?}, got {got:?}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches, first: {}",
        bad.len(),
        bad[0]
    );
}

#[test]
fn matches_brute_force_with_accepts_and_rejections() {
    let words = words();
    let tsv: String = words
        .iter()
        .map(|(w, z)| format!("{w}\t{z:.1}\n"))
        .collect();
    let (dict, _) = build_dict(&[&tsv], &default_build_options()).unwrap();
    let params = ScoreParams::default();
    let today = Day(20_000);
    let mut h = History::new();
    // Twice-accepted words get +1.5*log2(3) (not a multiple of 0.1, so no ties with priors);
    // twice-dismissed words (well under the 55-of-64 approximation bound) get -log2(3).
    let mut boosted = Vec::new();
    let mut dismissed = Vec::new();
    for (i, (w, _)) in words.iter().enumerate() {
        if i % 97 == 0 {
            for _ in 0..2 {
                h.record(
                    &LearnEvent {
                        word: w.clone(),
                        kind: LearnKind::Accepted,
                    },
                    today,
                    &params,
                );
            }
            boosted.push(w.clone());
        } else if i % 89 == 0 {
            for _ in 0..2 {
                h.record(
                    &LearnEvent {
                        word: w.clone(),
                        kind: LearnKind::Dismissed,
                    },
                    today,
                    &params,
                );
            }
            dismissed.push(w.clone());
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
    let rank = RankCtx {
        history: Some(&h),
        today,
        params: &params,
    };
    let mut bad = Vec::new();
    for p in prefixes(&words) {
        let got: Vec<String> = dict
            .complete(&p, 9, &rank)
            .into_iter()
            .map(|c| c.text)
            .collect();
        let want = brute(&words, &p, 9, &learned);
        if got != want {
            bad.push(format!("{p}: expected {want:?}, got {got:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches, first: {}",
        bad.len(),
        bad[0]
    );
}

#[test]
fn accepted_rare_word_surfaces_under_a_crowded_prefix() {
    let words = words();
    let tsv: String = words
        .iter()
        .map(|(w, z)| format!("{w}\t{z:.1}\n"))
        .collect();
    let (dict, _) = build_dict(&[&tsv], &default_build_options()).unwrap();
    let params = ScoreParams::default();
    let rare = words
        .iter()
        .filter(|(w, _)| w.starts_with("سل"))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0
        .clone();
    let mut h = History::new();
    for _ in 0..20 {
        h.record_accept(&rare, Day(20_000), &params);
    }
    let rank = RankCtx {
        history: Some(&h),
        today: Day(20_000),
        params: &params,
    };
    let got = dict.complete("س", 9, &rank);
    assert_eq!(got.first().map(|c| c.text.as_str()), Some(rare.as_str()));
}
