//! Fuzzy prefix matching (docs/autocomplete.md 13): an `fst::Automaton` over the
//! Damerau-Levenshtein (optimal string alignment) distance between the typed key and the
//! closest prefix of a dictionary key, and a bounded walk of the dictionary FST.

use std::ops::Range;

use fst::raw::{CompiledAddr, Fst, Output};
use fst::Automaton;

/// Longest typed key (chars) that can still reach a 32-char word within 2 edits.
pub(crate) const MAX_KEY_CHARS: usize = 34;
const ROW: usize = MAX_KEY_CHARS + 1;
/// Work bound per query: FST nodes entered plus candidate ids examined.
pub(crate) const MAX_WORK: usize = 4096;
/// Share of [`MAX_WORK`] the walk may use; the rest is kept for collecting candidates.
pub(crate) const MAX_WALK: usize = 3072;

/// Edit budget for a typed key of `n` chars.
pub(crate) fn max_edits(n: usize) -> u8 {
    if n <= 5 {
        1
    } else {
        2
    }
}

/// Matches keys having a prefix within `1..=maxd` edits of `key` (and nothing that
/// extends `key` exactly: that is the exact path).
#[derive(Debug, Clone)]
pub(crate) struct FuzzyPrefix {
    key: Vec<char>,
    maxd: u8,
}

/// DP state after some bytes of a dictionary key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FuzzyState {
    /// `row[i]` = distance(consumed chars, `key[..i]`), saturated at `maxd + 1`.
    row: [u8; ROW],
    /// The row before the last char (for transpositions).
    prev: [u8; ROW],
    last: Option<char>,
    /// Chars consumed (saturating).
    depth: u8,
    /// Smallest cell of `row`.
    min: u8,
    /// Smallest `row[n]` seen so far: the distance to the closest prefix.
    best: u8,
    /// A partially read UTF-8 char.
    cp: u32,
    pending: u8,
    dead: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// No extension can match.
    Dead,
    /// An exact prefix match: left to the exact path.
    Exact,
    /// Keep walking.
    Alive,
    /// Every extension matches with this many edits (it can no longer improve).
    Sticky(u8),
}

impl FuzzyPrefix {
    /// `None` if `key` is empty or longer than [`MAX_KEY_CHARS`].
    pub(crate) fn new(key: &str, maxd: u8) -> Option<Self> {
        let key: Vec<char> = key.chars().collect();
        if key.is_empty() || key.len() > MAX_KEY_CHARS || maxd == 0 {
            return None;
        }
        Some(FuzzyPrefix { key, maxd })
    }

    fn cap(&self) -> u8 {
        self.maxd + 1
    }

    pub(crate) fn status(&self, s: &FuzzyState) -> Status {
        if s.dead {
            return Status::Dead;
        }
        if s.best == 0 {
            return Status::Exact;
        }
        if s.best <= self.maxd && s.best <= s.min {
            Status::Sticky(s.best)
        } else if s.min > self.maxd {
            Status::Dead
        } else {
            Status::Alive
        }
    }

    fn feed(&self, s: &FuzzyState, c: char) -> FuzzyState {
        let n = self.key.len();
        let cap = self.cap();
        let depth = s.depth.saturating_add(1);
        let mut row = [cap; ROW];
        row[0] = depth.min(cap);
        let mut min = row[0];
        // Cell i is at least |i - depth|, so only a band of 2 * maxd + 1 cells can be within
        // budget; the rest stays at `cap`.
        let lo = usize::from(depth.saturating_sub(self.maxd)).max(1);
        let hi = n.min(usize::from(depth) + usize::from(self.maxd));
        for i in lo..=hi {
            let kc = self.key[i - 1];
            let mut v = (s.row[i] + 1)
                .min(row[i - 1] + 1)
                .min(s.row[i - 1] + u8::from(c != kc));
            if i >= 2 && c == self.key[i - 2] && s.last == Some(kc) {
                v = v.min(s.prev[i - 2] + 1);
            }
            row[i] = v.min(cap);
            min = min.min(row[i]);
        }
        FuzzyState {
            row,
            prev: s.row,
            last: Some(c),
            depth,
            min,
            best: s.best.min(row[n]),
            cp: 0,
            pending: 0,
            dead: false,
        }
    }

    fn step(&self, s: &FuzzyState, byte: u8) -> FuzzyState {
        if s.dead || matches!(self.status(s), Status::Sticky(_)) {
            return *s;
        }
        let dead = FuzzyState { dead: true, ..*s };
        let (cp, pending) = if s.pending == 0 {
            match byte {
                0x00..=0x7F => (u32::from(byte), 0),
                0xC2..=0xDF => (u32::from(byte & 0x1F), 1),
                0xE0..=0xEF => (u32::from(byte & 0x0F), 2),
                0xF0..=0xF4 => (u32::from(byte & 0x07), 3),
                _ => return dead,
            }
        } else if byte & 0xC0 == 0x80 {
            ((s.cp << 6) | u32::from(byte & 0x3F), s.pending - 1)
        } else {
            return dead;
        };
        if pending > 0 {
            return FuzzyState { cp, pending, ..*s };
        }
        match char::from_u32(cp) {
            Some(c) => self.feed(s, c),
            None => dead,
        }
    }
}

impl Automaton for FuzzyPrefix {
    type State = FuzzyState;

    fn start(&self) -> FuzzyState {
        let n = self.key.len();
        let cap = self.cap();
        let mut row = [cap; ROW];
        for (i, r) in row.iter_mut().enumerate().take(n + 1) {
            *r = (i as u8).min(cap);
        }
        FuzzyState {
            row,
            prev: [cap; ROW],
            last: None,
            depth: 0,
            min: 0,
            best: row[n],
            cp: 0,
            pending: 0,
            dead: false,
        }
    }

    fn is_match(&self, s: &FuzzyState) -> bool {
        !s.dead && s.pending == 0 && (1..=self.maxd).contains(&s.best)
    }

    fn can_match(&self, s: &FuzzyState) -> bool {
        matches!(self.status(s), Status::Alive | Status::Sticky(_))
    }

    fn will_always_match(&self, s: &FuzzyState) -> bool {
        matches!(self.status(s), Status::Sticky(_))
    }

    fn accept(&self, s: &FuzzyState, byte: u8) -> FuzzyState {
        self.step(s, byte)
    }
}

/// What the walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    /// One key (its id) at a node that was still being walked.
    Word { id: u64, edits: u8 },
    /// Every key under a prefix (bytes `prefix` of the walk's path buffer): ids `ids`.
    Subtree {
        prefix: Range<usize>,
        ids: Range<u64>,
        edits: u8,
    },
}

/// Id of the first (`first`) or last key under `addr`. `None` on a malformed path.
fn extreme<D: AsRef<[u8]>>(
    fst: &Fst<D>,
    mut addr: CompiledAddr,
    mut acc: Output,
    first: bool,
) -> Option<u64> {
    // Keys are at most 32 chars of up to 4 bytes.
    for _ in 0..=128 {
        let node = fst.node(addr);
        if node.is_empty() || (first && node.is_final()) {
            return node
                .is_final()
                .then(|| acc.cat(node.final_output()).value());
        }
        let t = node.transition(if first { 0 } else { node.len() - 1 });
        acc = acc.cat(t.out);
        addr = t.addr;
    }
    None
}

/// Walk `fst` under `aut`, entering at most [`MAX_WALK`] nodes. Subtree prefixes are
/// appended to `paths`. Returns the number of nodes entered.
///
/// Nodes are taken depth-first from the lowest non-empty bucket of "smallest DP cell", so
/// when the cap cuts a walk short the budget went to the most promising branches rather
/// than to the alphabetically first ones.
pub(crate) fn walk<D: AsRef<[u8]>>(
    fst: &Fst<D>,
    aut: &FuzzyPrefix,
    paths: &mut Vec<u8>,
    found: &mut Vec<Found>,
) -> usize {
    type Entry = (CompiledAddr, Output, FuzzyState, u32);
    let mut work = 0usize;
    // Path arena: (parent index, byte into this node); index 0 is the root.
    let mut arena: Vec<(u32, u8)> = Vec::with_capacity(256);
    arena.push((0, 0));
    let mut buckets: [Vec<Entry>; 3] = Default::default();
    buckets[0].push((fst.root().addr(), Output::zero(), aut.start(), 0));
    let mut path: Vec<u8> = Vec::with_capacity(64);
    loop {
        let Some((addr, acc, st, at)) = buckets.iter_mut().find_map(|b| b.pop()) else {
            break;
        };
        let node = fst.node(addr);
        if node.is_final() && aut.is_match(&st) {
            found.push(Found::Word {
                id: acc.cat(node.final_output()).value(),
                edits: st.best,
            });
        }
        for i in 0..node.len() {
            let t = node.transition(i);
            let ns = aut.accept(&st, t.inp);
            let status = aut.status(&ns);
            if matches!(status, Status::Dead | Status::Exact) {
                continue;
            }
            work += 1;
            if work > MAX_WALK {
                return work - 1;
            }
            let out = acc.cat(t.out);
            match status {
                Status::Sticky(edits) => {
                    let lo = extreme(fst, t.addr, out, true);
                    let hi = extreme(fst, t.addr, out, false);
                    if let (Some(lo), Some(hi)) = (lo, hi) {
                        if lo <= hi {
                            path.clear();
                            path.push(t.inp);
                            let mut i = at as usize;
                            while i != 0 {
                                let (parent, byte) = arena[i];
                                path.push(byte);
                                i = parent as usize;
                            }
                            let start = paths.len();
                            paths.extend(path.iter().rev());
                            found.push(Found::Subtree {
                                prefix: start..paths.len(),
                                ids: lo..hi + 1,
                                edits,
                            });
                        }
                    }
                }
                _ => {
                    arena.push((at, t.inp));
                    let b = usize::from(ns.min).min(buckets.len() - 1);
                    buckets[b].push((t.addr, out, ns, (arena.len() - 1) as u32));
                }
            }
        }
    }
    work
}

#[cfg(test)]
mod tests {
    use super::*;
    use fst::{IntoStreamer, Map, Streamer};

    /// Reference: OSA distance from `k` to the closest prefix of `w`.
    fn reference(k: &[char], w: &[char]) -> usize {
        let n = k.len();
        let mut d = vec![vec![0usize; n + 1]; w.len() + 1];
        for (i, cell) in d[0].iter_mut().enumerate() {
            *cell = i;
        }
        let mut best = d[0][n];
        for j in 1..=w.len() {
            d[j][0] = j;
            for i in 1..=n {
                let mut v = (d[j - 1][i] + 1)
                    .min(d[j][i - 1] + 1)
                    .min(d[j - 1][i - 1] + usize::from(w[j - 1] != k[i - 1]));
                if i >= 2 && j >= 2 && w[j - 1] == k[i - 2] && w[j - 2] == k[i - 1] {
                    v = v.min(d[j - 2][i - 2] + 1);
                }
                d[j][i] = v;
            }
            best = best.min(d[j][n]);
        }
        best
    }

    fn map(words: &[&str]) -> Map<Vec<u8>> {
        let mut v: Vec<&str> = words.to_vec();
        v.sort_unstable();
        v.dedup();
        Map::from_iter(v.into_iter().enumerate().map(|(i, w)| (w, i as u64))).unwrap()
    }

    fn by_walk(m: &Map<Vec<u8>>, key: &str) -> Vec<(String, u8)> {
        let aut = FuzzyPrefix::new(key, max_edits(key.chars().count())).unwrap();
        let mut paths = Vec::new();
        let mut found = Vec::new();
        walk(m.as_fst(), &aut, &mut paths, &mut found);
        let keys: Vec<String> = {
            let mut s = m.stream();
            let mut v = Vec::new();
            while let Some((k, _)) = s.next() {
                v.push(String::from_utf8(k.to_vec()).unwrap());
            }
            v
        };
        let mut out = Vec::new();
        for f in found {
            match f {
                Found::Word { id, edits } => out.push((keys[id as usize].clone(), edits)),
                Found::Subtree { prefix, ids, edits } => {
                    for id in ids {
                        let k = &keys[id as usize];
                        assert!(k.as_bytes().starts_with(&paths[prefix.clone()]));
                        out.push((k.clone(), edits));
                    }
                }
            }
        }
        out.sort();
        out
    }

    fn by_reference(words: &[&str], key: &str) -> Vec<(String, u8)> {
        let k: Vec<char> = key.chars().collect();
        let maxd = usize::from(max_edits(k.len()));
        let mut v: Vec<(String, u8)> = words
            .iter()
            .filter_map(|w| {
                let d = reference(&k, &w.chars().collect::<Vec<_>>());
                (1..=maxd).contains(&d).then(|| (w.to_string(), d as u8))
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    const WORDS: &[&str] = &[
        "سلام",
        "سلامت",
        "سلطان",
        "سم",
        "سمت",
        "لم",
        "سالم",
        "کتاب",
        "کتابخانه",
        "کتب",
        "کاتب",
        "تکاب",
        "کباب",
        "سل",
        "مسلم",
        "اسلام",
        "سلاام",
        "سلم",
        "خانه",
        "خوانه",
        "خانواده",
        "کتابها",
        "کتابخوان",
    ];

    #[test]
    fn walk_agrees_with_reference() {
        let m = map(WORDS);
        for key in [
            "سلم",
            "کتب",
            "سلاام",
            "سلا",
            "کتاب",
            "کاتب",
            "خانواد",
            "کتابخانا",
            "کتبخانه",
            "لس",
            "خوانواده",
            "اسلم",
        ] {
            assert_eq!(by_walk(&m, key), by_reference(WORDS, key), "key {key}");
        }
    }

    #[test]
    fn automaton_stream_agrees() {
        let m = map(WORDS);
        for key in ["سلم", "کتب", "سلاام", "خانواد"] {
            let aut = FuzzyPrefix::new(key, max_edits(key.chars().count())).unwrap();
            let mut s = m.search(aut).into_stream();
            let mut got = Vec::new();
            while let Some((k, _)) = s.next() {
                got.push(String::from_utf8(k.to_vec()).unwrap());
            }
            let want: Vec<String> = by_reference(WORDS, key).into_iter().map(|x| x.0).collect();
            assert_eq!(got, want, "key {key}");
        }
    }

    #[test]
    fn edit_kinds() {
        let m = map(WORDS);
        let has = |key: &str, w: &str, d: u8| by_walk(&m, key).contains(&(w.to_string(), d));
        assert!(has("سلم", "سلام", 1), "insertion");
        assert!(has("کتب", "کتاب", 1), "insertion");
        assert!(has("سلاام", "سلام", 1), "deletion");
        assert!(has("کاتب", "کتاب", 1), "transposition");
        assert!(has("کتاب", "کباب", 1), "substitution");
        // Exact prefix extensions are left to the exact path.
        assert!(!by_walk(&m, "سلا").iter().any(|(w, _)| w == "سلام"));
        // Two edits only from 6 chars on.
        assert!(!has("خوانه", "خانواده", 2));
        assert!(has("خوانواده", "خانواده", 1));
        assert!(has("کتبخانه", "کتابخانه", 1));
    }

    #[test]
    fn bounded_walk() {
        // A dense dictionary: every 3-letter word over 20 letters (8000 keys).
        let letters: Vec<char> = "ابپتثجچحخدذرزسشصطعفق".chars().collect();
        let mut words = Vec::new();
        for a in &letters {
            for b in &letters {
                for c in &letters {
                    words.push(format!("{a}{b}{c}"));
                }
            }
        }
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let m = map(&refs);
        // Two edits on three letters: most of the 8000 keys match.
        let aut = FuzzyPrefix::new("ابپ", 2).unwrap();
        let mut paths = Vec::new();
        let mut found = Vec::new();
        let work = walk(m.as_fst(), &aut, &mut paths, &mut found);
        assert!(work <= MAX_WALK);
        assert!(!found.is_empty());
    }

    #[test]
    fn rejects_unusable_keys() {
        assert!(FuzzyPrefix::new("", 1).is_none());
        assert!(FuzzyPrefix::new(&"ب".repeat(MAX_KEY_CHARS + 1), 2).is_none());
        let aut = FuzzyPrefix::new("سلم", 1).unwrap();
        // Invalid UTF-8 is dead.
        let s = aut.accept(&aut.start(), 0xFF);
        assert_eq!(aut.status(&s), Status::Dead);
        let s = aut.accept(&aut.accept(&aut.start(), 0xD8), b'a');
        assert_eq!(aut.status(&s), Status::Dead);
    }
}
