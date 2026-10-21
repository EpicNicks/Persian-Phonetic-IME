//! Offline evaluation of completion quality and latency (docs/autocomplete.md section 9).
//! Used by `xtask eval-dict`; not on any runtime path.
//!
//! The adaptive metric replays a corpus through [`Dictionary::complete`] plus a [`History`]
//! following the episode rules of section 2.6 (accept when the target is visible, `Skipped`
//! for rows above it, `Ignored` for the first three rows when a word is typed out). It
//! simulates the user at the completer level rather than driving a `Session` with Latin
//! keys, because no reverse Persian-to-Latin key mapping exists.

use crate::{Day, Dictionary, History, RankCtx, ScoreParams};
use pime_core::{completable, match_key, LearnEvent, LearnKind, ZWNJ};
use std::fmt::Write as _;
use std::time::Instant;

/// Words of an evaluation set: the first column of each non-comment line, in file order,
/// deduplicated.
pub fn parse_set(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let word = line.split('\t').next().unwrap_or("").trim();
        if !word.is_empty() && !out.iter().any(|w| w == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// Words of a corpus (one sentence per line, `#` comments), in order; tokens that are not
/// completable Persian words are skipped.
pub fn parse_corpus(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        for tok in line.split(|c: char| c.is_whitespace() || c.is_ascii_punctuation()) {
            let tok = tok.trim_matches(|c: char| !is_letter(c));
            if completable(tok) {
                out.push(tok.to_string());
            }
        }
    }
    out
}

fn is_letter(c: char) -> bool {
    matches!(
        c,
        '\u{0621}'
            ..='\u{064A}'
                | '\u{067E}'
                | '\u{0686}'
                | '\u{0698}'
                | '\u{06A9}'
                | '\u{06AF}'
                | '\u{06CC}'
    )
}

/// The word as a correct typist has it after `l` letters: everything up to and including
/// its `l`-th non-ZWNJ char (ZWNJs before it included).
pub fn prefix_letters(word: &str, l: usize) -> &str {
    let mut seen = 0;
    for (i, c) in word.char_indices() {
        if c != ZWNJ {
            seen += 1;
            if seen == l {
                return &word[..i + c.len_utf8()];
            }
        }
    }
    word
}

fn letters(word: &str) -> usize {
    match_key(word).chars().count()
}

/// Settings of the static run.
#[derive(Debug, Clone)]
pub struct StaticOptions {
    /// First prefix length evaluated (`predict_min_chars`).
    pub min_letters: usize,
    /// Completions requested per query (popup rows).
    pub limit: usize,
}

impl Default for StaticOptions {
    fn default() -> Self {
        StaticOptions {
            min_letters: 2,
            limit: 9,
        }
    }
}

/// Hit counts over a set of queries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HitStats {
    /// Queries made.
    pub queries: usize,
    /// Target was the first completion.
    pub hit1: usize,
    /// Target was among the first five.
    pub hit5: usize,
    /// Sum of reciprocal ranks (0 when absent from the `limit` results).
    pub rr_sum: f64,
}

impl HitStats {
    fn add(&mut self, rank: Option<usize>) {
        self.queries += 1;
        if let Some(r) = rank {
            self.hit1 += usize::from(r == 0);
            self.hit5 += usize::from(r < 5);
            self.rr_sum += 1.0 / (r + 1) as f64;
        }
    }
    /// hit@1 rate.
    pub fn hit1_rate(&self) -> f64 {
        ratio(self.hit1, self.queries)
    }
    /// hit@5 rate.
    pub fn hit5_rate(&self) -> f64 {
        ratio(self.hit5, self.queries)
    }
    /// Mean reciprocal rank.
    pub fn mrr(&self) -> f64 {
        if self.queries == 0 {
            0.0
        } else {
            self.rr_sum / self.queries as f64
        }
    }
}

fn ratio(a: usize, b: usize) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64
    }
}

/// `complete()` latency distribution, in microseconds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Latency {
    /// Number of timed calls.
    pub samples: usize,
    /// Mean.
    pub mean_us: f64,
    /// Median.
    pub p50_us: f64,
    /// 99th percentile.
    pub p99_us: f64,
    /// Maximum.
    pub max_us: f64,
}

impl Latency {
    fn from_ns(mut ns: Vec<u64>) -> Latency {
        if ns.is_empty() {
            return Latency::default();
        }
        ns.sort_unstable();
        let pick = |q: f64| ns[((ns.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
        Latency {
            samples: ns.len(),
            mean_us: ns.iter().sum::<u64>() as f64 / ns.len() as f64 / 1000.0,
            p50_us: pick(0.5),
            p99_us: pick(0.99),
            max_us: *ns.last().unwrap_or(&0) as f64 / 1000.0,
        }
    }
}

/// Result of [`evaluate_static`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StaticReport {
    /// Words in the set.
    pub words: usize,
    /// Words with at least `min_letters + 1` letters (the ones queried).
    pub evaluated: usize,
    /// Evaluated words absent from the dictionary (every query a miss).
    pub missing: usize,
    /// `(L, stats)` for each prefix length L that occurred, ascending.
    pub per_length: Vec<(usize, HitStats)>,
    /// All queries.
    pub overall: HitStats,
    /// Evaluated words that are hit@1 at some L.
    pub hit1_any: usize,
    /// Mean over evaluated words of `(m - L* - 1) / m` (0 for words never hit@1).
    pub letters_saved: f64,
    /// Queries that missed the exact text but offered the same word with different ZWNJ.
    pub zwnj_form_misses: usize,
    /// Latency of every query.
    pub latency: Latency,
}

/// Static metrics of section 9: empty history, each word queried at every prefix length
/// `min_letters..m`.
pub fn evaluate_static(dict: &Dictionary, words: &[String], opts: &StaticOptions) -> StaticReport {
    let params = ScoreParams::default();
    let rank = RankCtx {
        history: None,
        today: Day(0),
        params: &params,
    };
    let mut r = StaticReport {
        words: words.len(),
        ..StaticReport::default()
    };
    let mut ns = Vec::new();
    let mut saved_sum = 0.0;
    for word in words {
        let m = letters(word);
        if m <= opts.min_letters.max(1) {
            continue;
        }
        r.evaluated += 1;
        r.missing += usize::from(dict.prior(word).is_none());
        let key = match_key(word);
        let mut first_hit1 = None;
        for l in opts.min_letters.max(1)..m {
            let prefix = prefix_letters(word, l);
            let t = Instant::now();
            let got = dict.complete(prefix, opts.limit, &rank);
            ns.push(t.elapsed().as_nanos() as u64);
            let pos = got.iter().position(|c| c.text == *word);
            if pos.is_none() && got.iter().any(|c| match_key(&c.text) == key) {
                r.zwnj_form_misses += 1;
            }
            if pos == Some(0) && first_hit1.is_none() {
                first_hit1 = Some(l);
            }
            r.overall.add(pos);
            match r.per_length.iter_mut().find(|(x, _)| *x == l) {
                Some((_, s)) => s.add(pos),
                None => {
                    let mut s = HitStats::default();
                    s.add(pos);
                    r.per_length.push((l, s));
                }
            }
        }
        if let Some(l) = first_hit1 {
            r.hit1_any += 1;
            saved_sum += (m - l - 1) as f64 / m as f64;
        }
    }
    r.per_length.sort_by_key(|(l, _)| *l);
    r.letters_saved = if r.evaluated == 0 {
        0.0
    } else {
        saved_sum / r.evaluated as f64
    };
    r.latency = Latency::from_ns(ns);
    r
}

/// Settings of the adaptive replay.
#[derive(Debug, Clone)]
pub struct AdaptiveOptions {
    /// Letters typed before completions appear (`predict_min_chars`).
    pub min_letters: usize,
    /// Completion rows visible (`max_completions`).
    pub visible: usize,
    /// Simulated clock: one day passes per this many words.
    pub words_per_day: usize,
    /// Times the corpus is replayed back to back.
    pub passes: usize,
    /// Day of the first word.
    pub start_day: u32,
    /// Ranking constants under test.
    pub params: ScoreParams,
}

impl Default for AdaptiveOptions {
    fn default() -> Self {
        AdaptiveOptions {
            min_letters: 2,
            visible: 5,
            words_per_day: 500,
            passes: 3,
            start_day: 20_000,
            params: ScoreParams::default(),
        }
    }
}

/// One side (learning on or off) of the adaptive replay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayStats {
    /// Word occurrences in the second half of the replay.
    pub occurrences: usize,
    /// Of those, target was the first completion as soon as completions appeared.
    pub hit1_first: usize,
    /// Of those, target was accepted at some prefix length.
    pub accepted: usize,
    /// Letters typed plus one key per accept (Down presses not counted), second half.
    pub keys: usize,
    /// Letters of the second-half words.
    pub letters: usize,
}

/// Result of [`evaluate_adaptive`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdaptiveReport {
    /// Words replayed in total (all passes).
    pub replayed: usize,
    /// History-based ranking.
    pub learning: ReplayStats,
    /// Frequency-only ranking.
    pub frozen: ReplayStats,
    /// History entries at the end of the learning run.
    pub history_len: usize,
}

fn replay(
    dict: &Dictionary,
    words: &[&String],
    opts: &AdaptiveOptions,
    learn: bool,
) -> (ReplayStats, usize) {
    let mut history = History::new();
    let mut st = ReplayStats::default();
    let half = words.len() / 2;
    let min_l = opts.min_letters.max(1);
    for (i, word) in words.iter().enumerate() {
        let today = Day(opts.start_day + (i / opts.words_per_day.max(1)) as u32);
        let m = letters(word);
        let rank = RankCtx {
            history: learn.then_some(&history),
            today,
            params: &opts.params,
        };
        let mut events: Vec<LearnEvent> = Vec::new();
        let mut accepted_at = None;
        let mut first_hit1 = false;
        for l in min_l..m {
            let got = dict.complete(prefix_letters(word, l), opts.visible, &rank);
            if let Some(j) = got.iter().position(|c| c.text == **word) {
                first_hit1 = l == min_l && j == 0;
                events.push(LearnEvent {
                    word: got[j].text.clone(),
                    kind: LearnKind::Accepted,
                });
                events.extend(got[..j].iter().map(|c| LearnEvent {
                    word: c.text.clone(),
                    kind: LearnKind::Skipped,
                }));
                accepted_at = Some(l);
                break;
            }
        }
        if accepted_at.is_none() && m > min_l {
            // Typed out in full and committed: the list then visible is ignored.
            let got = dict.complete(word, opts.visible, &rank);
            events.extend(got.iter().take(3).map(|c| LearnEvent {
                word: c.text.clone(),
                kind: LearnKind::Ignored,
            }));
        }
        if learn {
            for ev in &events {
                history.record(ev, today, &opts.params);
            }
        }
        if i >= half {
            st.occurrences += 1;
            st.letters += m;
            st.hit1_first += usize::from(first_hit1);
            match accepted_at {
                Some(l) => {
                    st.accepted += 1;
                    st.keys += l + 1;
                }
                None => st.keys += m,
            }
        }
    }
    (st, history.len())
}

/// Adaptive metric of section 9: replay `corpus` (`passes` times), learning on vs off.
pub fn evaluate_adaptive(
    dict: &Dictionary,
    corpus: &[String],
    opts: &AdaptiveOptions,
) -> AdaptiveReport {
    let words: Vec<&String> = (0..opts.passes.max(1))
        .flat_map(|_| corpus.iter())
        .collect();
    let (learning, history_len) = replay(dict, &words, opts, true);
    let (frozen, _) = replay(dict, &words, opts, false);
    AdaptiveReport {
        replayed: words.len(),
        learning,
        frozen,
        history_len,
    }
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

impl StaticReport {
    /// Human-readable summary.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "words {} (evaluated {}, missing from dictionary {})",
            self.words, self.evaluated, self.missing
        );
        let _ = writeln!(
            s,
            "{:>8} {:>8} {:>8} {:>8} {:>8}",
            "L", "queries", "hit@1", "hit@5", "MRR"
        );
        for (l, h) in &self.per_length {
            if (2..=4).contains(l) {
                let _ = writeln!(
                    s,
                    "{:>8} {:>8} {:>8} {:>8} {:>8.3}",
                    l,
                    h.queries,
                    pct(h.hit1_rate()),
                    pct(h.hit5_rate()),
                    h.mrr()
                );
            }
        }
        let o = &self.overall;
        let _ = writeln!(
            s,
            "{:>8} {:>8} {:>8} {:>8} {:>8.3}",
            "all",
            o.queries,
            pct(o.hit1_rate()),
            pct(o.hit5_rate()),
            o.mrr()
        );
        let _ = writeln!(
            s,
            "hit@1 at some L: {} | mean letters saved: {} | ZWNJ-form misses: {}",
            pct(ratio(self.hit1_any, self.evaluated)),
            pct(self.letters_saved),
            self.zwnj_form_misses
        );
        let l = &self.latency;
        let _ = writeln!(
            s,
            "complete() latency over {} calls: mean {:.1} us, p50 {:.1} us, p99 {:.1} us, max {:.1} us",
            l.samples, l.mean_us, l.p50_us, l.p99_us, l.max_us
        );
        s
    }

    /// Machine-readable form.
    pub fn to_json(&self) -> String {
        let per: Vec<String> = self
            .per_length
            .iter()
            .map(|(l, h)| format!("{{\"L\":{l},{}}}", hit_json(h)))
            .collect();
        let l = &self.latency;
        format!(
            "{{\"words\":{},\"evaluated\":{},\"missing\":{},\"per_length\":[{}],\"overall\":{{{}}},\
             \"hit1_any\":{:.4},\"letters_saved\":{:.4},\"zwnj_form_misses\":{},\
             \"latency_us\":{{\"samples\":{},\"mean\":{:.2},\"p50\":{:.2},\"p99\":{:.2},\"max\":{:.2}}}}}",
            self.words,
            self.evaluated,
            self.missing,
            per.join(","),
            hit_json(&self.overall),
            ratio(self.hit1_any, self.evaluated),
            self.letters_saved,
            self.zwnj_form_misses,
            l.samples,
            l.mean_us,
            l.p50_us,
            l.p99_us,
            l.max_us
        )
    }
}

fn hit_json(h: &HitStats) -> String {
    format!(
        "\"queries\":{},\"hit1\":{:.4},\"hit5\":{:.4},\"mrr\":{:.4}",
        h.queries,
        h.hit1_rate(),
        h.hit5_rate(),
        h.mrr()
    )
}

fn replay_json(r: &ReplayStats) -> String {
    format!(
        "{{\"occurrences\":{},\"hit1_first\":{:.4},\"accepted\":{:.4},\"keys_per_letter\":{:.4}}}",
        r.occurrences,
        ratio(r.hit1_first, r.occurrences),
        ratio(r.accepted, r.occurrences),
        ratio(r.keys, r.letters)
    )
}

impl AdaptiveReport {
    /// Human-readable summary.
    pub fn to_text(&self) -> String {
        let row = |name: &str, r: &ReplayStats| {
            format!(
                "{name:>9}: hit@1 {} | accepted {} | keys per letter {:.3}\n",
                pct(ratio(r.hit1_first, r.occurrences)),
                pct(ratio(r.accepted, r.occurrences)),
                ratio(r.keys, r.letters)
            )
        };
        format!(
            "adaptive replay: {} words, second half {} (history entries at end: {})\n{}{}",
            self.replayed,
            self.learning.occurrences,
            self.history_len,
            row("learning", &self.learning),
            row("frozen", &self.frozen)
        )
    }

    /// Machine-readable form.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"replayed\":{},\"history_len\":{},\"learning\":{},\"frozen\":{}}}",
            self.replayed,
            self.history_len,
            replay_json(&self.learning),
            replay_json(&self.frozen)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_includes_zwnj_before_the_letter() {
        let w = "می\u{200C}خواهم";
        assert_eq!(prefix_letters(w, 2), "می");
        assert_eq!(prefix_letters(w, 3), "می\u{200C}خ");
        assert_eq!(prefix_letters(w, 99), w);
    }

    #[test]
    fn set_and_corpus_parsing() {
        let set = parse_set("# src\r\nسلام\tnote\r\n\r\nسلام\tdup\r\nکتاب\r\n");
        assert_eq!(set, ["سلام", "کتاب"]);
        let c = parse_corpus("# x\nسلام، دوست من! abc 12\n");
        assert_eq!(c, ["سلام", "دوست", "من"]);
    }

    #[test]
    fn latency_percentiles() {
        let l = Latency::from_ns((1..=100).map(|x| x * 1000).collect());
        assert_eq!(l.samples, 100);
        assert_eq!(l.max_us, 100.0);
        assert!((l.p99_us - 99.0).abs() < 1.01);
    }
}
