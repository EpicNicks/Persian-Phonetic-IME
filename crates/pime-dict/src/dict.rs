//! The loaded dictionary and ranked prefix completion (docs/autocomplete.md 3.1).

use std::cmp::Ordering;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use fst::{IntoStreamer, Streamer};
use pime_core::{complete_with_zwnj_rule, match_key, CompleteCtx, Completion};

use crate::format::{read_u32, validate, DictError, Layout, FLAGGED, MAX_FILE, TOP_THRESHOLD};
use crate::fuzzy::{max_edits, walk, Found, FuzzyPrefix, MAX_WORK};
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
    /// Fill slots the exact path left free with edit-distance matches (section 13).
    pub fuzzy: bool,
    /// Key chars needed before fuzzy matching is tried (values below 2 count as 2).
    pub fuzzy_min_chars: usize,
    /// Skip words flagged as vulgar, in both paths.
    pub hide_flagged: bool,
}

impl<'a> RankCtx<'a> {
    /// Exact prefix matches only, by frequency only, flagged words shown.
    pub fn new(today: Day, params: &'a ScoreParams) -> Self {
        RankCtx {
            history: None,
            today,
            params,
            fuzzy: false,
            fuzzy_min_chars: 3,
            hide_flagged: false,
        }
    }

    /// The ranking a [`pime_core::Completer`] query asks for: `history` is used only if
    /// `ctx.use_history`.
    pub fn from_ctx(
        ctx: &CompleteCtx,
        history: Option<&'a History>,
        today: Day,
        params: &'a ScoreParams,
    ) -> Self {
        RankCtx {
            history: history.filter(|_| ctx.use_history),
            today,
            params,
            fuzzy: ctx.fuzzy,
            fuzzy_min_chars: ctx.fuzzy_min_chars,
            hide_flagged: ctx.hide_flagged,
        }
    }
}

/// Defensive bound on keys scanned without a TOP entry (a valid file never needs > 256).
const MAX_SCAN: usize = 1024;
/// Most history words with a positive learned term added to the pool.
const MAX_LEARNED_POOL: usize = 256;
/// Fuzzy candidates (best by edits, then prior) that are scored with history.
const FUZZY_POOL: usize = 64;

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
    /// Validate `bytes` as `dict.bin` v2.
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

    /// True if `word` (by `match_key`) is in the dictionary and flagged as vulgar.
    pub fn is_flagged(&self, word: &str) -> bool {
        self.layout
            .fst
            .get(match_key(word))
            .is_some_and(|id| self.raw_prior(id) & FLAGGED != 0)
    }

    /// The PRIOR byte: flag bit 7, value bits 0..=6.
    fn raw_prior(&self, id: u64) -> u8 {
        match usize::try_from(id) {
            Ok(i) if i < self.layout.n => self
                .buf
                .get(self.layout.prior.start + i)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn prior_byte(&self, id: u64) -> u8 {
        self.raw_prior(id) & !FLAGGED
    }

    /// `(prior, flagged)` of an id, `None` if out of range.
    fn prior_flag(&self, id: u64) -> Option<(u8, bool)> {
        let i = usize::try_from(id).ok().filter(|&i| i < self.layout.n)?;
        let b = *self.buf.get(self.layout.prior.start + i)?;
        Some((b & !FLAGGED, b & FLAGGED != 0))
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
            .filter_map(|id| {
                let (prior, flagged) = self.prior_flag(id)?;
                if flagged && rank.hide_flagged {
                    return None;
                }
                let display = self.display(id);
                let mut score = f32::from(prior) / 10.0;
                if !learned.is_empty() {
                    let key = match_key(display);
                    if let Ok(i) = learned.binary_search_by(|(hk, _)| (*hk).cmp(key.as_str())) {
                        score += learned[i].1;
                    }
                }
                Some((score, prior, display.chars().count(), display))
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
                out.push(Completion {
                    text,
                    score,
                    edits: 0,
                });
            }
        }

        if out.len() < limit && rank.fuzzy {
            let n = k.chars().count();
            if n >= rank.fuzzy_min_chars.max(2) {
                self.fuzzy(prefix, &k, n, limit, rank, &p, &mut out);
            }
        }
        out
    }

    /// Append fuzzy matches (section 13) until `out` holds `limit` items.
    #[allow(clippy::too_many_arguments)]
    fn fuzzy(
        &self,
        prefix: &str,
        k: &str,
        n: usize,
        limit: usize,
        rank: &RankCtx,
        p: &ScoreParams,
        out: &mut Vec<Completion>,
    ) {
        let Some(aut) = FuzzyPrefix::new(k, max_edits(n)) else {
            return;
        };
        let mut paths = Vec::new();
        let mut found = Vec::new();
        let walked = walk(self.layout.fst.as_fst(), &aut, &mut paths, &mut found);
        let mut budget = MAX_WORK.saturating_sub(walked);

        // Fewer edits first; within that, big subtrees (cheap TOP lists of frequent words),
        // then single words, then small subtrees (scanned).
        let class = |f: &Found| match f {
            Found::Subtree { ids, edits, .. } if ids.end - ids.start > TOP_THRESHOLD as u64 => {
                (*edits, 0u8)
            }
            Found::Word { edits, .. } => (*edits, 1),
            Found::Subtree { edits, .. } => (*edits, 2),
        };
        found.sort_by_key(class);

        let per_prefix = limit - out.len();
        let mut hits: Vec<(u8, u8, u64)> = Vec::new();
        let push = |hits: &mut Vec<(u8, u8, u64)>, id: u64, edits: u8| -> bool {
            match self.prior_flag(id) {
                Some((_, true)) if rank.hide_flagged => false,
                Some((prior, _)) => {
                    hits.push((edits, prior, id));
                    true
                }
                None => false,
            }
        };
        for f in &found {
            if budget == 0 {
                break;
            }
            match f {
                Found::Word { id, edits } => {
                    budget -= 1;
                    push(&mut hits, *id, *edits);
                }
                Found::Subtree { prefix, ids, edits } => {
                    let top = (ids.end - ids.start > TOP_THRESHOLD as u64)
                        .then(|| paths.get(prefix.clone()))
                        .flatten()
                        .and_then(|key| self.layout.top.get(key));
                    if let Some(v) = top {
                        let (start, count) = ((v >> 8) as usize, (v & 0xFF) as usize);
                        let mut taken = 0;
                        for j in start..start.saturating_add(count) {
                            if budget == 0 || taken >= per_prefix {
                                break;
                            }
                            budget -= 1;
                            let at = self
                                .layout
                                .top_ids
                                .start
                                .saturating_add(j.saturating_mul(4));
                            if at >= self.layout.top_ids.end {
                                break;
                            }
                            if let Some(id) = read_u32(&self.buf, at) {
                                taken += usize::from(push(&mut hits, u64::from(id), *edits));
                            }
                        }
                    } else {
                        let end = ids.end.min(ids.start.saturating_add(budget as u64));
                        for id in ids.start..end {
                            push(&mut hits, id, *edits);
                        }
                        budget -= (end - ids.start) as usize;
                    }
                }
            }
        }

        hits.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        hits.dedup_by_key(|h| h.2);
        hits.truncate(FUZZY_POOL.max(per_prefix * 4));

        // History words with a positive learned term anywhere in what the walk found join
        // the pool even if their prior (or the budget) left them out above. History keys are
        // match keys, so a found prefix is a key range of the history too.
        if let Some(h) = rank.history.filter(|h| !h.is_empty()) {
            let mut boosted = 0usize;
            let mut add = |key: &str, e: &crate::history::Entry, edits: u8| {
                if boosted < MAX_LEARNED_POOL && learned_for(e, rank.today, p) > 0.0 {
                    if let Some(id) = self.layout.fst.get(key) {
                        boosted += usize::from(push(&mut hits, id, edits));
                    }
                }
            };
            for f in &found {
                match f {
                    Found::Word { id, edits } => {
                        let key = match_key(self.display(*id));
                        if let Some((k, e)) = h.prefix(&key).next().filter(|(k, _)| *k == key) {
                            add(k, e, *edits);
                        };
                    }
                    Found::Subtree { prefix, edits, .. } => {
                        let Some(key) = paths
                            .get(prefix.clone())
                            .and_then(|b| std::str::from_utf8(b).ok())
                        else {
                            continue;
                        };
                        for (k, e) in h.prefix(key) {
                            add(k, e, *edits);
                        }
                    }
                }
            }
        }

        let mut scored: Vec<(u8, f32, usize, &str)> = hits
            .into_iter()
            .filter_map(|(edits, prior, id)| {
                let display = self.display(id);
                if display.is_empty() || display == prefix {
                    return None;
                }
                let mut score = f32::from(prior) / 10.0;
                if let Some(h) = rank.history {
                    score += h.learned(display, rank.today, p);
                }
                Some((edits, score, display.chars().count(), display))
            })
            .collect();
        scored.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(b.1.total_cmp(&a.1))
                .then(a.2.cmp(&b.2))
                .then_with(|| a.3.cmp(b.3))
        });
        for (edits, score, _, display) in scored {
            if out.len() >= limit {
                break;
            }
            if !out.iter().any(|c| c.text == display) {
                out.push(Completion {
                    text: display.to_string(),
                    score,
                    edits,
                });
            }
        }
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
        RankCtx::new(Day(20_000), &P)
    }

    fn fuzzy() -> RankCtx<'static> {
        RankCtx {
            fuzzy: true,
            ..plain()
        }
    }

    fn with_edits(v: Vec<Completion>) -> Vec<(String, u8)> {
        v.into_iter().map(|c| (c.text, c.edits)).collect()
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
    fn fuzzy_edit_kinds() {
        let d = build("سلام\t5\nکتاب\t5\nکتابخانه\t4\nسالم\t3\nسلطان\t2\n");
        let has = |pre: &str, w: &str| {
            with_edits(d.complete(pre, 9, &fuzzy())).contains(&(w.to_string(), 1))
        };
        assert!(has("سلم", "سلام"), "insertion");
        assert!(has("کتب", "کتاب"), "insertion");
        assert!(has("سلاام", "سلام"), "deletion");
        assert!(has("کاتب", "کتاب"), "transposition");
        assert!(has("سلان", "سلام"), "substitution");
        // Fuzzy rows are the display form, replacing the preedit.
        let got = d.complete("کتب", 9, &fuzzy());
        assert!(got.iter().all(|c| c.edits == 1));
        // Off by default.
        assert!(d.complete("سلم", 9, &plain()).is_empty());
    }

    #[test]
    fn exact_before_fuzzy() {
        let d = build("سلام\t5\nسلامت\t3\nسلاح\t6\nسلا\t2\n");
        // "سلا" exact: سلاح, سلام, سلامت; fuzzy then fills (سل... words within 1 edit).
        let got = with_edits(d.complete("سلا", 9, &fuzzy()));
        assert_eq!(
            &got[..3],
            &[("سلاح".into(), 0), ("سلام".into(), 0), ("سلامت".into(), 0)]
        );
        assert!(got[3..].iter().all(|(_, e)| *e > 0));
        // A full exact list leaves no room for fuzzy.
        let got = with_edits(d.complete("سلا", 2, &fuzzy()));
        assert_eq!(got, [("سلاح".into(), 0), ("سلام".into(), 0)]);
        // Fuzzy order: edits, then score.
        let got = with_edits(d.complete("سلن", 9, &fuzzy()));
        assert_eq!(
            got,
            [
                ("سلاح".into(), 1),
                ("سلام".into(), 1),
                ("سلامت".into(), 1),
                ("سلا".into(), 1)
            ]
        );
        // The preedit's own word is never offered.
        assert!(!d
            .complete("سلام", 9, &fuzzy())
            .iter()
            .any(|c| c.text == "سلام"));
    }

    #[test]
    fn fuzzy_min_chars_and_two_edits() {
        let d = build("کتابخانه\t4\nسلام\t5\n");
        let ctx = |min| RankCtx {
            fuzzy_min_chars: min,
            ..fuzzy()
        };
        assert!(d.complete("سلم", 9, &ctx(4)).is_empty(), "below min chars");
        assert_eq!(texts(d.complete("سلم", 9, &ctx(3))), ["سلام"]);
        assert_eq!(
            texts(d.complete("سلم", 9, &ctx(0))),
            ["سلام"],
            "0 counts as 2"
        );
        // Two edits only from 6 chars: "کتبخنه" (7 -> 2 deletions) vs "کتبخانه" (1).
        assert_eq!(
            with_edits(d.complete("کتبخنه", 9, &fuzzy())),
            [("کتابخانه".into(), 2)]
        );
        assert_eq!(
            with_edits(d.complete("کتبخانه", 9, &fuzzy())),
            [("کتابخانه".into(), 1)]
        );
        assert!(
            d.complete("کتبخن", 9, &fuzzy()).is_empty(),
            "5 chars: 1 edit only"
        );
    }

    #[test]
    fn flagged_hidden_or_shown() {
        let tsv = "سگ\t3\tp\nسگی\t2\nسگها\t2.5\t\nسگدان\t1.5\tp\n";
        let mut r: &[u8] = tsv.as_bytes();
        let mut inputs: [&mut dyn BufRead; 1] = [&mut r];
        let (bytes, rep) = build_from_tsv(&mut inputs, &BuildOptions::default()).unwrap();
        assert_eq!(rep.flagged, 2);
        let d = Dictionary::from_bytes(bytes).unwrap();
        assert!(d.is_flagged("سگ") && d.is_flagged("سگدان") && !d.is_flagged("سگی"));
        let hide = RankCtx {
            hide_flagged: true,
            ..fuzzy()
        };
        assert_eq!(
            texts(d.complete("سگ", 9, &plain())),
            ["سگها", "سگی", "سگدان"]
        );
        assert_eq!(texts(d.complete("سگ", 9, &hide)), ["سگها", "سگی"]);
        // Fuzzy path: "سگد" -> 1 edit from سگ (deletion) and سگی / سگها (substitution).
        let shown = texts(d.complete("سگد", 9, &fuzzy()));
        assert!(shown.contains(&"سگ".to_string()));
        let hidden = texts(d.complete("سگد", 9, &hide));
        assert!(!hidden.iter().any(|w| w == "سگ" || w.starts_with("سگد")));
        assert!(hidden.contains(&"سگی".to_string()));
    }

    #[test]
    fn fuzzy_history_boost() {
        let d = build("سلام\t5\nسالم\t4\n");
        let mut h = History::new();
        for _ in 0..3 {
            h.record_accept("سالم", Day(20_000), &P);
        }
        let ctx = RankCtx {
            history: Some(&h),
            ..fuzzy()
        };
        assert_eq!(texts(d.complete("سلم", 9, &fuzzy())), ["سلام", "سالم"]);
        assert_eq!(texts(d.complete("سلم", 9, &ctx)), ["سالم", "سلام"]);
    }

    #[test]
    fn fuzzy_work_is_bounded() {
        // Dense: every 3-letter word over 24 letters (13 824 keys), all within 1 edit.
        let letters: Vec<char> = "ابپتثجچحخدذرزژسشصضطظعغفق".chars().collect();
        let mut tsv = String::new();
        for (i, a) in letters.iter().enumerate() {
            for b in &letters {
                for c in &letters {
                    tsv.push_str(&format!("{a}{b}{c}\t{}\n", 1.0 + (i % 9) as f32 / 2.0));
                }
            }
        }
        let d = build(&tsv);
        let t = std::time::Instant::now();
        let got = d.complete("ابپ", 9, &fuzzy());
        assert!(t.elapsed() < std::time::Duration::from_millis(200));
        assert_eq!(got.len(), 9);
        assert!(got.iter().all(|c| c.edits == 1 || c.edits == 0));
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
        for version in [1u8, 3] {
            let mut v = good.clone();
            v[8] = version;
            assert_eq!(
                Dictionary::from_bytes(v).unwrap_err(),
                DictError::UnsupportedVersion(u16::from(version)),
                "version 1 files are rejected"
            );
        }
        // A PRIOR value above 90 is corrupt; bit 7 alone is the flag.
        let prior_at = read_u32(&good, 24 + 8).unwrap() as usize;
        let mut v = good.clone();
        v[prior_at] = 91;
        let crc = crc32fast::hash(&v[64..]);
        v[16..20].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            Dictionary::from_bytes(v).unwrap_err(),
            DictError::Corrupt("prior value")
        );
        let mut v = good.clone();
        v[prior_at] |= FLAGGED;
        let crc = crc32fast::hash(&v[64..]);
        v[16..20].copy_from_slice(&crc.to_le_bytes());
        let d = Dictionary::from_bytes(v).unwrap();
        assert!(d.is_flagged("سلام"));
        assert_eq!(d.prior("سلام"), Some(5.0));
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

    /// Fuzzy queries: prefixes of 3..=8 chars of real words with one seeded edit.
    /// `cargo test -p pime-dict --release --lib -- --ignored fuzzy_latency --nocapture`
    #[test]
    #[ignore]
    fn fuzzy_latency_on_wordfreq() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../data/dict/fa-wordfreq.tsv"
        );
        let Ok(tsv) = std::fs::read_to_string(path) else {
            return;
        };
        let d = build(&tsv);
        let mut h = History::new();
        for line in tsv.lines().take(2000) {
            h.record_accept(line.split('\t').next().unwrap_or(""), Day(20_000), &P);
        }
        let ctx = RankCtx {
            history: Some(&h),
            hide_flagged: true,
            ..fuzzy()
        };
        let letters: Vec<char> = "ابپتجچحخدرزسشصطعفقکگلمنوهی".chars().collect();
        let mut seed = 0x2545_F491u32;
        let mut rnd = |m: usize| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as usize % m.max(1)
        };
        let mut queries: Vec<(String, &str)> = Vec::new();
        for w in tsv.lines().filter_map(|l| l.split('\t').next()) {
            let cs: Vec<char> = w.chars().filter(|&c| c != '\u{200C}').collect();
            for n in 3..=cs.len().min(8) {
                let mut p: Vec<char> = cs[..n].to_vec();
                let at = rnd(n);
                match rnd(4) {
                    0 => p[at] = letters[rnd(letters.len())],
                    1 => {
                        p.remove(at);
                    }
                    2 => p.insert(at, letters[rnd(letters.len())]),
                    _ if at + 1 < n => p.swap(at, at + 1),
                    _ => p[at] = letters[rnd(letters.len())],
                }
                queries.push((p.into_iter().collect(), w));
            }
            if queries.len() >= 20_000 {
                break;
            }
        }
        let (mut fuzzy_runs, mut hits) = (0usize, 0usize);
        let mut times: Vec<std::time::Duration> = queries
            .iter()
            .map(|(q, w)| {
                let t = std::time::Instant::now();
                let got = std::hint::black_box(d.complete(q, 9, &ctx));
                let e = t.elapsed();
                fuzzy_runs += usize::from(got.iter().any(|c| c.edits > 0));
                hits += usize::from(got.iter().any(|c| c.text == *w));
                e
            })
            .collect();
        let mut walks: Vec<(std::time::Duration, usize)> = queries
            .iter()
            .filter_map(|(q, _)| {
                let k = match_key(q);
                let aut = FuzzyPrefix::new(&k, max_edits(k.chars().count()))?;
                let (mut paths, mut found) = (Vec::new(), Vec::new());
                let t = std::time::Instant::now();
                let work = walk(d.layout.fst.as_fst(), &aut, &mut paths, &mut found);
                Some((t.elapsed(), work))
            })
            .collect();
        walks.sort();
        let capped = walks
            .iter()
            .filter(|w| w.1 >= crate::fuzzy::MAX_WALK)
            .count();
        println!(
            "walk only: p99 {:?} max {:?}, walks at the cap {capped}",
            walks[(walks.len() - 1) * 99 / 100].0,
            walks[walks.len() - 1].0
        );
        times.sort();
        let pct = |q: f64| times[((times.len() as f64 - 1.0) * q) as usize];
        println!(
            "fuzzy: words {} queries {} with fuzzy rows {} source word shown {hits} p50 {:?} p99 {:?} max {:?}",
            d.len(),
            times.len(),
            fuzzy_runs,
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
                let hide = RankCtx {
                    hide_flagged: true,
                    ..fuzzy()
                };
                for pre in ["م", "می", "س", "سلا", "x", "سلم", "میخواهد"] {
                    let _ = d.complete(pre, 9, &plain());
                    let _ = d.complete(pre, 9, &hide);
                    let _ = d.prior(pre);
                    let _ = d.is_flagged(pre);
                }
                let _ = d.info();
            }
        }
    }
}
