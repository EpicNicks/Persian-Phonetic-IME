//! The loaded dictionary and ranked prefix completion (docs/autocomplete.md 3.1).

use std::cmp::Ordering;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use fst::{IntoStreamer, Streamer};
use pime_core::{complete_with_zwnj_rule, match_key, Completion};

use crate::format::{read_u32, validate, DictError, Layout, MAX_FILE};
use crate::history::{learned_for, History};
use crate::score::{Day, ScoreParams};

/// Ranking context for [`Dictionary::complete`].
#[derive(Debug, Clone, Copy)]
pub struct RankCtx<'a> {
    /// Learned history, or `None` to rank by frequency only.
    pub history: Option<&'a History>,
    /// Today, for decay.
    pub today: Day,
    /// Scoring constants (clamped before use).
    pub params: &'a ScoreParams,
}

/// Defensive bound on keys scanned without a TOP entry (a valid file never needs > 256).
const MAX_SCAN: usize = 1024;
/// Most history words with a positive learned term added to the pool.
const MAX_LEARNED_POOL: usize = 256;

/// A validated, in-memory `dict.bin`. `Debug` prints only the entry count.
pub struct Dictionary {
    buf: Arc<Vec<u8>>,
    layout: Layout,
}

impl fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dictionary")
            .field("words", &self.len())
            .finish()
    }
}

/// Open for reading, letting others rename or delete the file meanwhile.
pub(crate) fn open_shared(path: &Path) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        opts.share_mode(0x7);
    }
    opts.open(path)
}

/// Read at most `max` bytes of `path`; `Ok(None)` if it is larger.
pub(crate) fn read_capped(path: &Path, max: usize) -> io::Result<Option<Vec<u8>>> {
    let file = open_shared(path)?;
    let hint = file.metadata().map(|m| m.len()).unwrap_or(0);
    if hint > max as u64 {
        return Ok(None);
    }
    let mut buf = Vec::with_capacity(hint as usize);
    file.take(max as u64 + 1).read_to_end(&mut buf)?;
    Ok((buf.len() <= max).then_some(buf))
}

impl Dictionary {
    /// Validate `bytes` as `dict.bin` v1.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, DictError> {
        let buf = Arc::new(bytes);
        let layout = validate(&buf)?;
        Ok(Dictionary { buf, layout })
    }

    /// Read and validate a file. File I/O: never call on the key path.
    pub fn load(path: &Path) -> Result<Self, DictError> {
        match read_capped(path, MAX_FILE) {
            Ok(Some(bytes)) => Dictionary::from_bytes(bytes),
            Ok(None) => Err(DictError::TooLarge),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(DictError::Missing),
            Err(e) => Err(DictError::Io(e.kind())),
        }
    }

    /// Number of words.
    pub fn len(&self) -> usize {
        self.layout.n
    }

    /// True if the dictionary has no words.
    pub fn is_empty(&self) -> bool {
        self.layout.n == 0
    }

    /// The INFO section (`key=value` lines).
    pub fn info(&self) -> &str {
        self.buf
            .get(self.layout.info.clone())
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
    }

    /// Zipf frequency of `word` (looked up by `match_key`), if present.
    pub fn prior(&self, word: &str) -> Option<f32> {
        let id = self.layout.fst.get(match_key(word))?;
        Some(f32::from(self.prior_byte(id)) / 10.0)
    }

    fn prior_byte(&self, id: u64) -> u8 {
        match usize::try_from(id) {
            Ok(i) if i < self.layout.n => self
                .buf
                .get(self.layout.prior.start + i)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn display(&self, id: u64) -> &str {
        let Ok(i) = usize::try_from(id) else {
            return "";
        };
        let at = self
            .layout
            .display_offsets
            .saturating_add(i.saturating_mul(4));
        let (Some(s), Some(e)) = (read_u32(&self.buf, at), read_u32(&self.buf, at + 4)) else {
            return "";
        };
        let base = self.layout.display_data.start;
        self.buf
            .get(base + s as usize..base + e as usize)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
    }

    #[cfg(test)]
    pub(crate) fn display_for_test(&self, id: u64) -> String {
        self.display(id).to_string()
    }

    /// At most `limit` completions of `prefix`, best first (docs/autocomplete.md 3.1).
    pub fn complete(&self, prefix: &str, limit: usize, rank: &RankCtx) -> Vec<Completion> {
        if limit == 0 {
            return Vec::new();
        }
        let k = match_key(prefix);
        if k.is_empty() {
            return Vec::new();
        }
        let p = rank.params.clamped();
        let mut pool: Vec<u64> = Vec::with_capacity(96);
        if let Some(v) = self.layout.top.get(k.as_bytes()) {
            let start = (v >> 8) as usize;
            let count = (v & 0xFF) as usize;
            for j in start..start.saturating_add(count) {
                let at = self
                    .layout
                    .top_ids
                    .start
                    .saturating_add(j.saturating_mul(4));
                if at < self.layout.top_ids.end {
                    if let Some(id) = read_u32(&self.buf, at) {
                        pool.push(u64::from(id));
                    }
                }
            }
        } else {
            let mut s = self.layout.fst.range().ge(k.as_bytes()).into_stream();
            while let Some((key, id)) = s.next() {
                if !key.starts_with(k.as_bytes()) || pool.len() >= MAX_SCAN {
                    break;
                }
                pool.push(id);
            }
        }

        // Learned terms of history keys under the prefix, sorted by key (history order).
        let mut learned: Vec<(&str, f32)> = Vec::new();
        if let Some(h) = rank.history {
            learned = h
                .prefix(&k)
                .map(|(key, e)| (key, learned_for(e, rank.today, &p)))
                .filter(|&(_, l)| l != 0.0)
                .collect();
            let mut boosted: Vec<(&str, f32)> =
                learned.iter().copied().filter(|&(_, l)| l > 0.0).collect();
            if boosted.len() > MAX_LEARNED_POOL {
                boosted.sort_by(|a, b| b.1.total_cmp(&a.1));
                boosted.truncate(MAX_LEARNED_POOL);
            }
            for (key, _) in boosted {
                if let Some(id) = self.layout.fst.get(key) {
                    pool.push(id);
                }
            }
        }
        pool.sort_unstable();
        pool.dedup();

        let mut scored: Vec<(f32, u8, usize, &str)> = pool
            .into_iter()
            .map(|id| {
                let prior = self.prior_byte(id);
                let display = self.display(id);
                let mut score = f32::from(prior) / 10.0;
                if !learned.is_empty() {
                    let key = match_key(display);
                    if let Ok(i) = learned.binary_search_by(|(hk, _)| (*hk).cmp(key.as_str())) {
                        score += learned[i].1;
                    }
                }
                (score, prior, display.chars().count(), display)
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then(b.1.cmp(&a.1))
                .then(a.2.cmp(&b.2))
                .then_with(|| a.3.cmp(b.3))
        });

        let mut out: Vec<Completion> = Vec::with_capacity(limit.min(scored.len()));
        for (score, _, _, display) in scored {
            if out.len() >= limit {
                break;
            }
            let Some(text) = complete_with_zwnj_rule(prefix, display) else {
                continue;
            };
            if !out.iter().any(|c| c.text == text) {
                out.push(Completion { text, score });
            }
        }
        out
    }
}

/// Orders ids the way TOP lists are built: prior desc, shorter display, display.
pub(crate) fn top_order(a: (u8, &str), b: (u8, &str)) -> Ordering {
    b.0.cmp(&a.0)
        .then(a.1.chars().count().cmp(&b.1.chars().count()))
        .then_with(|| a.1.cmp(b.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build_from_tsv, BuildOptions};
    use crate::history::Rejection;
    use std::io::BufRead;

    pub(crate) fn build(tsv: &str) -> Dictionary {
        let mut r: &[u8] = tsv.as_bytes();
        let mut inputs: [&mut dyn BufRead; 1] = [&mut r];
        let (bytes, _) = build_from_tsv(&mut inputs, &BuildOptions::default()).unwrap();
        Dictionary::from_bytes(bytes).unwrap()
    }

    fn texts(v: Vec<Completion>) -> Vec<String> {
        v.into_iter().map(|c| c.text).collect()
    }

    const P: ScoreParams = ScoreParams {
        alpha: 1.5,
        beta: 1.0,
        half_life_days: 30.0,
        skip: 0.25,
        ignore: 0.5,
        dismiss: 1.0,
        learned_min: -4.0,
        learned_max: 6.0,
        cap: 64.0,
    };

    fn plain() -> RankCtx<'static> {
        RankCtx {
            history: None,
            today: Day(20_000),
            params: &P,
        }
    }

    #[test]
    fn zwnj_rule_and_order() {
        let d = build("می\u{200C}خواهم\t4.8\nمیز\t4.5\nمی\t6\nمیخک\t2\n");
        assert_eq!(d.len(), 4);
        let got = texts(d.complete("می", 5, &plain()));
        assert_eq!(got, ["می\u{200C}خواهم", "میز", "میخک"]);
        let got = texts(d.complete("میخ", 5, &plain()));
        assert_eq!(got, ["میخواهم", "میخک"]);
        let got = texts(d.complete("می\u{200C}", 5, &plain()));
        assert_eq!(got, ["می\u{200C}خواهم", "می\u{200C}ز", "می\u{200C}خک"]);
        assert!(d.complete("", 5, &plain()).is_empty());
        assert!(d.complete("می", 0, &plain()).is_empty());
        assert!(d.complete("ب", 5, &plain()).is_empty());
        assert_eq!(texts(d.complete("می", 1, &plain())), ["می\u{200C}خواهم"]);
        assert_eq!(d.prior("مي"), Some(6.0));
        assert_eq!(d.prior("نه"), None);
    }

    #[test]
    fn ties_shorter_then_lexical() {
        let d = build("سلامت\t3\nسلام\t3\nسلطان\t3\n");
        assert_eq!(
            texts(d.complete("س", 5, &plain())),
            ["سلام", "سلامت", "سلطان"]
        );
    }

    #[test]
    fn history_reranks_and_adds() {
        let d = build("میز\t4.5\nمیخ\t3.5\n");
        let mut h = History::new();
        h.record_accept("میخ", Day(20_000), &P);
        h.record_reject("میز", Rejection::Dismissed, Day(20_000), &P);
        h.record_accept("نیست", Day(20_000), &P); // not in the dictionary: ignored
        let ctx = RankCtx {
            history: Some(&h),
            ..plain()
        };
        let got = d.complete("م", 5, &ctx);
        assert_eq!(got[0].text, "میخ");
        assert!((got[0].score - 5.0).abs() < 1e-5);
        assert!((got[1].score - 3.5).abs() < 1e-5);
        assert_eq!(texts(d.complete("م", 5, &plain())), ["میز", "میخ"]);
    }

    #[test]
    fn top_entries_used_for_large_ranges() {
        // 300 words under "ب": a TOP entry exists for "ب" and is ranked like a scan.
        let mut tsv = String::new();
        let letters: Vec<char> = "ابپتثجچحخدذرزژسشصضطظعغفقکگلمنوهی".chars().collect();
        for i in 0..300usize {
            let a = letters[i % letters.len()];
            let b = letters[(i / letters.len()) % letters.len()];
            tsv.push_str(&format!("ب{a}{b}\t{}\n", 1.0 + (i % 50) as f32 / 10.0));
        }
        let d = build(&tsv);
        assert_eq!(d.len(), 300);
        assert!(d.layout.top.get("ب").is_some());
        let got = d.complete("ب", 9, &plain());
        assert_eq!(got.len(), 9);
        assert!(got[..6].iter().all(|c| (c.score - 5.9).abs() < 1e-5));
        assert!(got[6..].iter().all(|c| (c.score - 5.8).abs() < 1e-5));
        // The scan of a narrower prefix agrees with the TOP order.
        let narrow = d.complete("بی", 3, &plain());
        assert!(narrow.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn rejects_bad_files() {
        let d = build("سلام\t5\n");
        let good = d.buf.as_ref().clone();
        assert_eq!(
            Dictionary::from_bytes(vec![]).unwrap_err(),
            DictError::BadMagic
        );
        let mut v = good.clone();
        v[8] = 2;
        assert_eq!(
            Dictionary::from_bytes(v).unwrap_err(),
            DictError::UnsupportedVersion(2)
        );
        let mut v = good.clone();
        v[10] = 1;
        assert_eq!(
            Dictionary::from_bytes(v).unwrap_err(),
            DictError::Corrupt("flags")
        );
        let mut v = good.clone();
        let last = v.len() - 1;
        v[last] ^= 0x55;
        assert_eq!(
            Dictionary::from_bytes(v).unwrap_err(),
            DictError::Corrupt("checksum")
        );
        for n in 0..good.len() {
            assert!(Dictionary::from_bytes(good[..n].to_vec()).is_err());
        }
        let missing = std::env::temp_dir().join(format!("pime-missing-{}.bin", std::process::id()));
        assert_eq!(Dictionary::load(&missing).unwrap_err(), DictError::Missing);
    }

    /// `cargo test -p pime-dict --release --lib -- --ignored latency --nocapture`
    #[test]
    #[ignore]
    fn latency_on_wordfreq() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/dict/fa-wordfreq.tsv"
        );
        let Ok(tsv) = std::fs::read_to_string(path) else {
            return;
        };
        let d = build(&tsv);
        let mut h = History::new();
        for (i, line) in tsv.lines().take(5000).enumerate() {
            let w = line.split('\t').next().unwrap_or("");
            if i % 2 == 0 {
                h.record_accept(w, Day(20_000), &P);
            } else {
                h.record_reject(w, Rejection::Ignored, Day(20_000), &P);
            }
        }
        let ctx = RankCtx {
            history: Some(&h),
            ..plain()
        };
        let prefixes: Vec<String> = tsv
            .lines()
            .filter_map(|l| l.split('\t').next())
            .flat_map(|w| {
                let cs: Vec<char> = w.chars().collect();
                (1..cs.len()).map(move |n| cs[..n].iter().collect::<String>())
            })
            .take(50_000)
            .collect();
        let mut times: Vec<std::time::Duration> = prefixes
            .iter()
            .map(|p| {
                let t = std::time::Instant::now();
                std::hint::black_box(d.complete(p, 9, &ctx));
                t.elapsed()
            })
            .collect();
        times.sort();
        let pct = |q: f64| times[((times.len() as f64 - 1.0) * q) as usize];
        println!(
            "words {} prefixes {} p50 {:?} p99 {:?} max {:?}",
            d.len(),
            times.len(),
            pct(0.5),
            pct(0.99),
            pct(1.0)
        );
        assert!(pct(0.99) < std::time::Duration::from_micros(500));
    }

    /// Flip bytes in the body and fix the crc so the inner validation is exercised.
    #[test]
    fn mutated_bodies_never_panic() {
        let d = build("می\u{200C}خواهم\t4.8\nمیز\t4.5\nسلام\t5\nسلامت\t4\n");
        let good = d.buf.as_ref().clone();
        let mut seed = 0x9E37_79B9u32;
        for _ in 0..3000 {
            let mut v = good.clone();
            for _ in 0..3 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let at = 24 + (seed as usize >> 8) % (v.len() - 24);
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                v[at] = (seed >> 24) as u8;
            }
            let crc = crc32fast::hash(&v[64..]);
            v[16..20].copy_from_slice(&crc.to_le_bytes());
            if let Ok(d) = Dictionary::from_bytes(v) {
                for pre in ["م", "می", "س", "سلا", "x"] {
                    let _ = d.complete(pre, 9, &plain());
                    let _ = d.prior(pre);
                }
                let _ = d.info();
            }
        }
    }
}
