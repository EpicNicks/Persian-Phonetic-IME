//! Learned accept/reject history and `history.bin` v1 (docs/autocomplete.md 3.4).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Bound;

use pime_core::{match_key, LearnEvent, LearnKind};

use crate::score::{decay_factor, learned_term, Day, ScoreParams};

const MAGIC: &[u8; 8] = b"PIMEHIST";
const VERSION: u16 = 1;
/// Most entries kept (and allowed in a file).
pub(crate) const MAX_ENTRIES: usize = 10_000;
/// In memory the map may grow this far past [`MAX_ENTRIES`] before it is trimmed.
const SLACK: usize = 512;
/// Longest key in bytes.
pub(crate) const MAX_KEY_BYTES: usize = 128;
/// Largest `history.bin`.
pub(crate) const MAX_FILE: usize = 2 << 20;
/// Largest stored accepts/rejects (the upper bound of `ScoreParams::cap`).
const MAX_VALUE: f32 = 1000.0;
/// Entries with both decayed values below this are pruned.
const PRUNE_BELOW: f32 = 0.05;

/// Learned counts of one word, decayed lazily from `day`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    /// Accept count as of `day`.
    pub accepts: f32,
    /// Weighted rejection count as of `day`.
    pub rejects: f32,
    /// Day of the last update.
    pub day: Day,
}

impl Entry {
    fn decayed(&self, today: Day, p: &ScoreParams) -> (f32, f32) {
        let f = decay_factor(self.day, today, p);
        (self.accepts * f, self.rejects * f)
    }
}

/// How a shown completion was passed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// A row above the accepted one.
    Skipped,
    /// Visible while the typed text was committed as is.
    Ignored,
    /// Visible when Esc closed the popup.
    Dismissed,
}

/// Why `history.bin` was rejected. Never contains words or paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// Larger than 2 MiB.
    TooLarge,
    /// Not a history file.
    BadMagic,
    /// A format version this build does not read.
    UnsupportedVersion(u16),
    /// Structurally invalid; the text names the check that failed.
    Corrupt(&'static str),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistoryError::TooLarge => f.write_str("history file too large"),
            HistoryError::BadMagic => f.write_str("not a history file"),
            HistoryError::UnsupportedVersion(v) => write!(f, "unsupported history version {v}"),
            HistoryError::Corrupt(what) => write!(f, "history file damaged ({what})"),
        }
    }
}

impl std::error::Error for HistoryError {}

/// Per-word learned history, keyed by `match_key`. `Debug` prints only the entry count.
#[derive(Clone, Default, PartialEq)]
pub struct History {
    epoch: u64,
    entries: BTreeMap<String, Entry>,
}

impl fmt::Debug for History {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("History")
            .field("entries", &self.entries.len())
            .finish()
    }
}

fn finite_value(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, MAX_VALUE)
    }
}

impl History {
    /// Empty history with epoch 0 (never persisted).
    pub fn new() -> Self {
        History::default()
    }

    /// Apply one learn event.
    pub fn record(&mut self, ev: &LearnEvent, today: Day, p: &ScoreParams) {
        match ev.kind {
            LearnKind::Accepted => self.record_accept(&ev.word, today, p),
            LearnKind::Skipped => self.record_reject(&ev.word, Rejection::Skipped, today, p),
            LearnKind::Ignored => self.record_reject(&ev.word, Rejection::Ignored, today, p),
            LearnKind::Dismissed => self.record_reject(&ev.word, Rejection::Dismissed, today, p),
        }
    }

    /// `word` was accepted: accepts + 1 (capped), rejects halved.
    pub fn record_accept(&mut self, word: &str, today: Day, p: &ScoreParams) {
        let p = p.clamped();
        self.update(word, today, &p, |a, r| ((a + 1.0).min(p.cap), r / 2.0));
    }

    /// `word` was shown and passed over.
    pub fn record_reject(&mut self, word: &str, how: Rejection, today: Day, p: &ScoreParams) {
        let p = p.clamped();
        let w = match how {
            Rejection::Skipped => p.skip,
            Rejection::Ignored => p.ignore,
            Rejection::Dismissed => p.dismiss,
        };
        self.update(word, today, &p, |a, r| (a, (r + w).min(p.cap)));
    }

    fn update(
        &mut self,
        word: &str,
        today: Day,
        p: &ScoreParams,
        f: impl Fn(f32, f32) -> (f32, f32),
    ) {
        let key = match_key(word);
        if key.is_empty() || key.len() > MAX_KEY_BYTES {
            return;
        }
        let (a, r) = self
            .entries
            .get(&key)
            .map_or((0.0, 0.0), |e| e.decayed(today, p));
        let (a, r) = f(a, r);
        let day = self.entries.get(&key).map_or(today, |e| e.day.max(today));
        let entry = Entry {
            accepts: finite_value(a),
            rejects: finite_value(r),
            day,
        };
        self.entries.insert(key, entry);
        if self.entries.len() > MAX_ENTRIES + SLACK {
            self.trim(today, p);
        }
    }

    /// The clamped learned term for `key` (a `match_key`; any word is accepted and keyed).
    pub fn learned(&self, key: &str, today: Day, p: &ScoreParams) -> f32 {
        let p = p.clamped();
        let found = self.entries.get(key).or_else(|| {
            let k = match_key(key);
            if k == key {
                None
            } else {
                self.entries.get(&k)
            }
        });
        found.map_or(0.0, |e| learned_for(e, today, &p))
    }

    /// Apply decay up to `today`, prune near-zero entries and cap the size.
    pub fn decay(&mut self, today: Day, p: &ScoreParams) {
        let p = p.clamped();
        self.entries.retain(|_, e| {
            let (a, r) = e.decayed(today, &p);
            *e = Entry {
                accepts: finite_value(a),
                rejects: finite_value(r),
                day: e.day.max(today),
            };
            e.accepts >= PRUNE_BELOW || e.rejects >= PRUNE_BELOW
        });
        if self.entries.len() > MAX_ENTRIES {
            self.trim(today, &p);
        }
    }

    /// Evict down to [`MAX_ENTRIES`]: smallest decayed `a + r` first, then oldest.
    fn trim(&mut self, today: Day, p: &ScoreParams) {
        let excess = self.entries.len().saturating_sub(MAX_ENTRIES);
        if excess == 0 {
            return;
        }
        let order = self.eviction_order(Some((today, p)));
        let doomed: Vec<String> = order.into_iter().take(excess).map(str::to_owned).collect();
        for k in doomed {
            self.entries.remove(&k);
        }
    }

    /// Keys, first to evict first. Without `today`, values are compared undecayed.
    fn eviction_order(&self, decay: Option<(Day, &ScoreParams)>) -> Vec<&str> {
        let mut v: Vec<(f32, Day, &str)> = self
            .entries
            .iter()
            .map(|(k, e)| {
                let (a, r) = match decay {
                    Some((today, p)) => e.decayed(today, p),
                    None => (e.accepts, e.rejects),
                };
                (a + r, e.day, k.as_str())
            })
            .collect();
        v.sort_by(|x, y| {
            x.0.total_cmp(&y.0)
                .then(x.1.cmp(&y.1))
                .then_with(|| x.2.cmp(y.2))
        });
        v.into_iter().map(|(_, _, k)| k).collect()
    }

    /// Entries whose key starts with `key_prefix`, in key order.
    pub fn prefix<'a>(
        &'a self,
        key_prefix: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a Entry)> + 'a {
        self.entries
            .range::<str, _>((Bound::Included(key_prefix), Bound::Unbounded))
            .take_while(move |(k, _)| k.starts_with(key_prefix))
            .map(|(k, e)| (k.as_str(), e))
    }

    /// Merge `other` in: per key the later day wins; on the same day, per-field max.
    pub fn merge(&mut self, other: &History) {
        for (k, e) in &other.entries {
            let e = Entry {
                accepts: finite_value(e.accepts),
                rejects: finite_value(e.rejects),
                day: e.day,
            };
            match self.entries.get_mut(k) {
                None => {
                    self.entries.insert(k.clone(), e);
                }
                Some(s) => match e.day.cmp(&s.day) {
                    Ordering::Greater => *s = e,
                    Ordering::Equal => {
                        s.accepts = s.accepts.max(e.accepts);
                        s.rejects = s.rejects.max(e.rejects);
                    }
                    Ordering::Less => {}
                },
            }
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove every entry (the epoch is kept).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Identity of the file this history belongs to; 0 = never persisted.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn set_epoch(&mut self, epoch: u64) {
        self.epoch = epoch;
    }

    /// Serialise as `history.bin` v1. More than 10 000 entries: the strongest are written.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut keep: Vec<&str> = if self.entries.len() > MAX_ENTRIES {
            let order = self.eviction_order(None);
            let skip = order.len() - MAX_ENTRIES;
            order.into_iter().skip(skip).collect()
        } else {
            self.entries.keys().map(String::as_str).collect()
        };
        keep.retain(|k| !k.is_empty() && k.len() <= MAX_KEY_BYTES);
        keep.sort_unstable();
        let mut out = Vec::with_capacity(28 + keep.len() * 24);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&(keep.len() as u32).to_le_bytes());
        for k in keep {
            let e = &self.entries[k];
            out.push(k.len() as u8);
            out.extend_from_slice(k.as_bytes());
            out.extend_from_slice(&finite_value(e.accepts).to_le_bytes());
            out.extend_from_slice(&finite_value(e.rejects).to_le_bytes());
            out.extend_from_slice(&e.day.0.to_le_bytes());
        }
        let crc = crc32fast::hash(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse and validate `history.bin` v1.
    pub fn from_bytes(b: &[u8]) -> Result<History, HistoryError> {
        if b.len() > MAX_FILE {
            return Err(HistoryError::TooLarge);
        }
        if b.len() < 8 || &b[..8] != MAGIC {
            return Err(HistoryError::BadMagic);
        }
        if b.len() < 28 {
            return Err(HistoryError::Corrupt("truncated header"));
        }
        let mut r = Reader { b, pos: 8 };
        let version = r.u16()?;
        if version != VERSION {
            return Err(HistoryError::UnsupportedVersion(version));
        }
        let (body, crc) = b.split_at(b.len() - 4);
        let crc = u32::from_le_bytes([crc[0], crc[1], crc[2], crc[3]]);
        if crc32fast::hash(body) != crc {
            return Err(HistoryError::Corrupt("checksum"));
        }
        let mut r = Reader { b: body, pos: 10 };
        if r.u16()? != 0 {
            return Err(HistoryError::Corrupt("reserved field"));
        }
        let epoch = r.u64()?;
        let count = r.u32()? as usize;
        if count > MAX_ENTRIES {
            return Err(HistoryError::Corrupt("too many entries"));
        }
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let len = r.u8()? as usize;
            if len == 0 || len > MAX_KEY_BYTES {
                return Err(HistoryError::Corrupt("key length"));
            }
            let key = std::str::from_utf8(r.take(len)?)
                .map_err(|_| HistoryError::Corrupt("key encoding"))?;
            let accepts = f32::from_le_bytes(r.array()?);
            let rejects = f32::from_le_bytes(r.array()?);
            let day = Day(r.u32()?);
            let valid = |v: f32| v.is_finite() && (0.0..=MAX_VALUE).contains(&v);
            if !valid(accepts) || !valid(rejects) {
                return Err(HistoryError::Corrupt("value out of range"));
            }
            let e = Entry {
                accepts,
                rejects,
                day,
            };
            if entries.insert(key.to_owned(), e).is_some() {
                return Err(HistoryError::Corrupt("duplicate key"));
            }
        }
        if r.pos != body.len() {
            return Err(HistoryError::Corrupt("trailing bytes"));
        }
        Ok(History { epoch, entries })
    }
}

/// The learned term of one entry, `p` already clamped.
pub(crate) fn learned_for(e: &Entry, today: Day, p: &ScoreParams) -> f32 {
    let (a, r) = e.decayed(today, p);
    learned_term(a, r, p)
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], HistoryError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or(HistoryError::Corrupt("truncated"))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], HistoryError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, HistoryError> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, HistoryError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, HistoryError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, HistoryError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn ev(word: &str, kind: LearnKind) -> LearnEvent {
        LearnEvent {
            word: word.to_string(),
            kind,
        }
    }

    #[test]
    fn accept_and_reject() {
        let mut h = History::new();
        h.record(&ev("می\u{200C}خواهم", LearnKind::Accepted), Day(100), &P);
        assert_eq!(h.len(), 1);
        assert!((h.learned("میخواهم", Day(100), &P) - 1.5).abs() < 1e-6);
        // Lookups by a non-key form are keyed too.
        assert!((h.learned("می\u{200C}خواهم", Day(100), &P) - 1.5).abs() < 1e-6);
        h.record(&ev("میز", LearnKind::Dismissed), Day(100), &P);
        assert!((h.learned("میز", Day(100), &P) + 1.0).abs() < 1e-6);
        h.record(&ev("میز", LearnKind::Ignored), Day(100), &P);
        h.record(&ev("میز", LearnKind::Skipped), Day(100), &P);
        let e = h.prefix("میز").next().unwrap().1;
        assert!((e.rejects - 1.75).abs() < 1e-6);
        h.record(&ev("میز", LearnKind::Accepted), Day(100), &P);
        let e = *h.prefix("میز").next().unwrap().1;
        assert_eq!(e.accepts, 1.0);
        assert!((e.rejects - 0.875).abs() < 1e-6);
        // Decay: one half-life later the accept counts half.
        let l = h.learned("میخواهم", Day(130), &P);
        assert!((l - 1.5 * 1.5f32.log2()).abs() < 1e-5);
        // Clock going backwards counts as no decay.
        assert!((h.learned("میخواهم", Day(50), &P) - 1.5).abs() < 1e-6);
        assert_eq!(h.learned("نیست", Day(100), &P), 0.0);
    }

    #[test]
    fn cap_and_empty_keys() {
        let mut h = History::new();
        for _ in 0..100 {
            h.record_accept("سلام", Day(1), &P);
        }
        assert_eq!(h.prefix("سل").next().unwrap().1.accepts, 64.0);
        h.record_accept("\u{200C}", Day(1), &P);
        h.record_accept(&"س".repeat(65), Day(1), &P);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn decay_prunes_and_caps() {
        let mut h = History::new();
        h.record_accept("سلام", Day(0), &P);
        h.record_reject("سلامت", Rejection::Skipped, Day(0), &P);
        h.decay(Day(30 * 5), &P); // 1/32: accept 0.031 pruned, so is the 0.25 reject
        assert!(h.is_empty());
        let mut h = History::new();
        for i in 0..(MAX_ENTRIES + SLACK + 1) {
            h.record_reject(&format!("س{i}"), Rejection::Skipped, Day(1), &P);
        }
        // Digits are kept in keys; the point here is the size bound.
        assert_eq!(h.len(), MAX_ENTRIES);
        h.record_accept("سلام", Day(1), &P);
        h.decay(Day(1), &P);
        assert_eq!(h.len(), MAX_ENTRIES);
        assert!(
            h.learned("سلام", Day(1), &P) > 0.0,
            "strong entry survives the cap"
        );
    }

    #[test]
    fn prefix_iter() {
        let mut h = History::new();
        for w in ["سلام", "سلامت", "سرد", "سل"] {
            h.record_accept(w, Day(1), &P);
        }
        let keys: Vec<&str> = h.prefix("سل").map(|(k, _)| k).collect();
        assert_eq!(keys, ["سل", "سلام", "سلامت"]);
        assert_eq!(h.prefix("ب").count(), 0);
        assert_eq!(h.prefix("").count(), 4);
    }

    #[test]
    fn merge_rules() {
        let mut a = History::new();
        a.record_accept("الف", Day(5), &P);
        a.record_accept("ب", Day(5), &P);
        let mut b = History::new();
        b.record_reject("الف", Rejection::Dismissed, Day(6), &P);
        b.record_reject("ب", Rejection::Dismissed, Day(5), &P);
        b.record_accept("پ", Day(1), &P);
        a.merge(&b);
        let get = |h: &History, k: &str| *h.prefix(k).next().unwrap().1;
        assert_eq!(get(&a, "الف"), get(&b, "الف"));
        let e = get(&a, "ب");
        assert_eq!((e.accepts, e.rejects), (1.0, 1.0));
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn bytes_roundtrip_and_rejects() {
        let mut h = History::new();
        h.set_epoch(0xDEAD_BEEF);
        h.record_accept("سلام", Day(20_000), &P);
        h.record_reject("می\u{200C}خواهم", Rejection::Ignored, Day(20_001), &P);
        let b = h.to_bytes();
        let back = History::from_bytes(&b).unwrap();
        assert_eq!(back, h);
        assert_eq!(back.epoch(), 0xDEAD_BEEF);
        assert_eq!(format!("{back:?}"), "History { entries: 2 }");

        assert_eq!(History::from_bytes(b"nope"), Err(HistoryError::BadMagic));
        let mut bad = b.clone();
        bad[30] ^= 1;
        assert_eq!(
            History::from_bytes(&bad),
            Err(HistoryError::Corrupt("checksum"))
        );
        let mut v2 = b.clone();
        v2[8] = 2;
        assert_eq!(
            History::from_bytes(&v2),
            Err(HistoryError::UnsupportedVersion(2))
        );
        assert_eq!(
            History::from_bytes(&vec![0; MAX_FILE + 1]),
            Err(HistoryError::TooLarge)
        );
        // Every truncation fails cleanly.
        for n in 0..b.len() {
            assert!(History::from_bytes(&b[..n]).is_err());
        }
        assert_eq!(
            History::from_bytes(&History::new().to_bytes())
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn from_bytes_never_panics_on_noise() {
        let mut seed = 0x1234_5678u32;
        for len in 0..300usize {
            let mut b: Vec<u8> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect();
            if len >= 8 {
                b[..8].copy_from_slice(MAGIC);
            }
            if len >= 10 {
                b[8] = 1;
                b[9] = 0;
            }
            if len >= 12 {
                let n = b.len() - 4;
                let crc = crc32fast::hash(&b[..n]);
                b[n..].copy_from_slice(&crc.to_le_bytes());
            }
            let _ = History::from_bytes(&b);
        }
    }

    #[test]
    fn to_bytes_caps_entries() {
        let mut h = History::new();
        for i in 0..(MAX_ENTRIES + 10) {
            h.entries.insert(
                format!("k{i}"),
                Entry {
                    accepts: i as f32,
                    rejects: 0.0,
                    day: Day(1),
                },
            );
        }
        let back = History::from_bytes(&h.to_bytes()).unwrap();
        assert_eq!(back.len(), MAX_ENTRIES);
        assert!(
            back.prefix("k0").all(|(k, _)| k != "k0"),
            "weakest entry dropped"
        );
    }
}
