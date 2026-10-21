//! Build `dict.bin` from word-frequency TSV (docs/autocomplete.md 3.2). Build path only.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{self, BufRead};

use fst::MapBuilder;
use pime_core::{match_key, ZWNJ};
use unicode_normalization::UnicodeNormalization;

use crate::dict::top_order;
use crate::format::{
    HEADER_LEN, MAGIC, MAX_FILE, MAX_PRIOR, SECTIONS, TOP_COUNT, TOP_THRESHOLD, VERSION,
};

/// Build limits and INFO metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildOptions {
    /// Words below this Zipf frequency (after merging) are dropped. Default 1.0.
    pub min_zipf: f32,
    /// Keep at most this many words, most frequent first. Default 200 000.
    pub max_entries: usize,
    /// Fail if the output is larger. Default 16 MiB.
    pub max_bytes: usize,
    /// Extra INFO lines (`source`, `wordfreq_version`, `wordlist`, ...). `entries` and
    /// `builder` are always written by the builder; `licence` defaults to CC-BY-SA-4.0.
    pub info: Vec<(String, String)>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            min_zipf: 1.0,
            max_entries: 200_000,
            max_bytes: 16 << 20,
            info: Vec::new(),
        }
    }
}

/// What the build did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildReport {
    /// Data lines read (comments and blank lines excluded).
    pub read: usize,
    /// Words in the output.
    pub kept: usize,
    /// Lines dropped by normalisation: a char outside the Persian set, or a key length
    /// outside 2..=32.
    pub dropped_chars: usize,
    /// Lines whose key was already seen (merged into an earlier line).
    pub merged: usize,
    /// Output words whose display form contains ZWNJ.
    pub with_zwnj: usize,
    /// Output size in bytes.
    pub bytes: usize,
}

/// Why a build failed. Line errors carry the 1-based input and line number, never text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// Reading an input failed.
    Io {
        /// 1-based input index.
        input: usize,
        /// 1-based line number.
        line: usize,
        /// The I/O error kind.
        kind: io::ErrorKind,
    },
    /// A malformed line.
    Line {
        /// 1-based input index.
        input: usize,
        /// 1-based line number.
        line: usize,
        /// What is wrong.
        what: &'static str,
    },
    /// An INFO key or value contains a newline, or a key contains `=` or is empty.
    BadInfo,
    /// The output would exceed `max_bytes` (or the 32 MiB load limit).
    TooLarge {
        /// Size the output would have.
        bytes: usize,
    },
    /// The fst builder failed (should not happen with sorted unique keys).
    Fst,
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Io { input, line, kind } => {
                write!(f, "input {input}, line {line}: read error ({kind})")
            }
            BuildError::Line { input, line, what } => {
                write!(f, "input {input}, line {line}: {what}")
            }
            BuildError::BadInfo => f.write_str("invalid INFO key or value"),
            BuildError::TooLarge { bytes } => write!(f, "dictionary would be {bytes} bytes"),
            BuildError::Fst => f.write_str("fst construction failed"),
        }
    }
}

impl std::error::Error for BuildError {}

/// Shortest and longest key, in chars.
const MIN_KEY: usize = 2;
const MAX_KEY: usize = 32;

fn allowed(c: char) -> bool {
    matches!(c,
        '\u{0621}'..='\u{063A}'
        | '\u{0641}'..='\u{0648}'
        | '\u{067E}' | '\u{0686}' | '\u{0698}' | '\u{06A9}' | '\u{06AF}' | '\u{06CC}'
        | ZWNJ)
}

/// Normalise one word (3.2): `Some((display, key))` or `None` if it must be dropped.
pub(crate) fn normalise(word: &str) -> Option<(String, String)> {
    let mut out = String::with_capacity(word.len());
    for c in word.nfc() {
        let c = match c {
            '\u{064A}' | '\u{0649}' => '\u{06CC}',
            '\u{0643}' => '\u{06A9}',
            '\u{0629}' => '\u{0647}',
            c => c,
        };
        if matches!(c, '\u{0640}' | '\u{064B}'..='\u{065F}' | '\u{0670}') {
            continue;
        }
        if c == ZWNJ && (out.is_empty() || out.ends_with(ZWNJ)) {
            continue;
        }
        if !allowed(c) {
            return None;
        }
        out.push(c);
    }
    while out.ends_with(ZWNJ) {
        out.pop();
    }
    let key = match_key(&out);
    (MIN_KEY..=MAX_KEY)
        .contains(&key.chars().count())
        .then_some((out, key))
}

#[derive(Default)]
struct Acc {
    total: f64,
    /// Display variants with their summed frequency.
    variants: Vec<(String, f64)>,
}

struct Word {
    key: String,
    display: String,
    prior: u8,
}

/// Build `dict.bin` v1 from one or more TSV inputs (`word<TAB>zipf`), merged in order.
pub fn build_from_tsv(
    inputs: &mut [&mut dyn BufRead],
    opts: &BuildOptions,
) -> Result<(Vec<u8>, BuildReport), BuildError> {
    let mut report = BuildReport::default();
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let mut raw = Vec::new();
    for (idx, input) in inputs.iter_mut().enumerate() {
        let input_no = idx + 1;
        let mut line_no = 0usize;
        loop {
            raw.clear();
            line_no += 1;
            let got = input
                .read_until(b'\n', &mut raw)
                .map_err(|e| BuildError::Io {
                    input: input_no,
                    line: line_no,
                    kind: e.kind(),
                })?;
            if got == 0 {
                break;
            }
            let err = |what| BuildError::Line {
                input: input_no,
                line: line_no,
                what,
            };
            let text = std::str::from_utf8(&raw).map_err(|_| err("not UTF-8"))?;
            let text = if line_no == 1 {
                text.trim_start_matches('\u{FEFF}')
            } else {
                text
            };
            let text = text.trim_end_matches(['\n', '\r']);
            if text.trim().is_empty() || text.starts_with('#') {
                continue;
            }
            report.read += 1;
            let mut fields = text.split('\t');
            let word = fields.next().unwrap_or("").trim_matches(' ');
            let zipf = fields.next().ok_or(err("missing tab"))?.trim();
            if fields.next().is_some_and(|f| !f.trim().is_empty()) {
                return Err(err("more than two fields"));
            }
            if word.is_empty() {
                return Err(err("empty word"));
            }
            let zipf: f64 = zipf.parse().map_err(|_| err("zipf is not a number"))?;
            if !(0.0..=9.0).contains(&zipf) {
                return Err(err("zipf outside 0..=9"));
            }
            let Some((display, key)) = normalise(word) else {
                report.dropped_chars += 1;
                continue;
            };
            let freq = 10f64.powf(zipf);
            let a = acc.entry(key).or_default();
            if !a.variants.is_empty() {
                report.merged += 1;
            }
            a.total += freq;
            match a.variants.iter_mut().find(|(d, _)| *d == display) {
                Some(v) => v.1 += freq,
                None => a.variants.push((display, freq)),
            }
        }
    }

    // Merge variants, filter, keep the most frequent.
    let min_zipf = f64::from(opts.min_zipf);
    let mut words: Vec<(f64, Word)> = acc
        .into_iter()
        .filter_map(|(key, a)| {
            let zipf = a.total.log10();
            if zipf.is_nan() || zipf < min_zipf {
                return None;
            }
            let has_zwnj = a.variants.iter().any(|(d, _)| d.contains(ZWNJ));
            let display = a
                .variants
                .into_iter()
                .filter(|(d, _)| !has_zwnj || d.contains(ZWNJ))
                .max_by(|x, y| x.1.total_cmp(&y.1).then_with(|| y.0.cmp(&x.0)))
                .map(|(d, _)| d)
                .unwrap_or_default();
            let prior = (zipf * 10.0).round().clamp(0.0, f64::from(MAX_PRIOR)) as u8;
            Some((
                zipf,
                Word {
                    key,
                    display,
                    prior,
                },
            ))
        })
        .collect();
    words.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
    words.truncate(opts.max_entries);
    let mut words: Vec<Word> = words.into_iter().map(|(_, w)| w).collect();
    words.sort_by(|a, b| a.key.cmp(&b.key));
    report.kept = words.len();
    report.with_zwnj = words.iter().filter(|w| w.display.contains(ZWNJ)).count();

    let bytes = encode(&words, opts)?;
    report.bytes = bytes.len();
    Ok((bytes, report))
}

fn encode(words: &[Word], opts: &BuildOptions) -> Result<Vec<u8>, BuildError> {
    let n = u32::try_from(words.len()).map_err(|_| BuildError::TooLarge { bytes: usize::MAX })?;

    let mut fst = MapBuilder::memory();
    for (i, w) in words.iter().enumerate() {
        fst.insert(&w.key, i as u64).map_err(|_| BuildError::Fst)?;
    }
    let fst = fst.into_inner().map_err(|_| BuildError::Fst)?;

    let prior: Vec<u8> = words.iter().map(|w| w.prior).collect();

    let mut display = Vec::new();
    let mut text = Vec::new();
    for w in words {
        display.extend_from_slice(&(text.len() as u32).to_le_bytes());
        text.extend_from_slice(w.display.as_bytes());
    }
    display.extend_from_slice(&(text.len() as u32).to_le_bytes());
    display.extend_from_slice(&text);

    let top = encode_top(words)?;
    let info = encode_info(words.len(), opts)?;

    let mut out = vec![0u8; HEADER_LEN];
    let mut table = [(0u32, 0u32); SECTIONS];
    for (i, sec) in [&fst, &prior, &display, &top, &info]
        .into_iter()
        .enumerate()
    {
        while !out.len().is_multiple_of(8) {
            out.push(0);
        }
        table[i] = (out.len() as u32, sec.len() as u32);
        out.extend_from_slice(sec);
    }
    if out.len() > opts.max_bytes || out.len() > MAX_FILE {
        return Err(BuildError::TooLarge { bytes: out.len() });
    }
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&VERSION.to_le_bytes());
    out[12..16].copy_from_slice(&n.to_le_bytes());
    for (i, (off, len)) in table.iter().enumerate() {
        out[24 + 8 * i..28 + 8 * i].copy_from_slice(&off.to_le_bytes());
        out[28 + 8 * i..32 + 8 * i].copy_from_slice(&len.to_le_bytes());
    }
    let crc = crc32fast::hash(&out[HEADER_LEN..]);
    out[16..20].copy_from_slice(&crc.to_le_bytes());
    Ok(out)
}

/// TOP: for every char-prefix whose range holds more than 256 keys, the 64 best ids.
fn encode_top(words: &[Word]) -> Result<Vec<u8>, BuildError> {
    // Keys are sorted, so each prefix's range is contiguous: (start, count).
    let mut ranges: HashMap<&str, (usize, usize)> = HashMap::new();
    for (i, w) in words.iter().enumerate() {
        for (end, c) in w.key.char_indices() {
            let r = ranges.entry(&w.key[..end + c.len_utf8()]).or_insert((i, 0));
            r.1 += 1;
        }
    }
    let mut big: Vec<(&str, (usize, usize))> = ranges
        .into_iter()
        .filter(|(_, (_, count))| *count > TOP_THRESHOLD)
        .collect();
    big.sort_unstable_by(|a, b| a.0.cmp(b.0));

    let mut map = MapBuilder::memory();
    let mut ids: Vec<u32> = Vec::new();
    for (prefix, (start, count)) in big {
        let mut best: Vec<usize> = (start..start + count).collect();
        best.sort_by(|&a, &b| {
            top_order(
                (words[a].prior, &words[a].display),
                (words[b].prior, &words[b].display),
            )
            .then(a.cmp(&b))
        });
        best.truncate(TOP_COUNT);
        let value = ((ids.len() as u64) << 8) | best.len() as u64;
        map.insert(prefix, value).map_err(|_| BuildError::Fst)?;
        ids.extend(best.into_iter().map(|i| i as u32));
    }
    let map = map.into_inner().map_err(|_| BuildError::Fst)?;

    let mut out = Vec::with_capacity(8 + map.len() + ids.len() * 4);
    out.extend_from_slice(&(map.len() as u32).to_le_bytes());
    out.extend_from_slice(&map);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    for id in ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    Ok(out)
}

fn encode_info(entries: usize, opts: &BuildOptions) -> Result<Vec<u8>, BuildError> {
    let mut lines: Vec<(String, String)> = Vec::new();
    for (k, v) in &opts.info {
        let bad = |s: &str| s.contains(['\n', '\r']);
        if k.is_empty() || k.contains('=') || bad(k) || bad(v) {
            return Err(BuildError::BadInfo);
        }
        if k != "entries" && k != "builder" {
            lines.push((k.clone(), v.clone()));
        }
    }
    if !lines.iter().any(|(k, _)| k == "licence") {
        lines.push(("licence".into(), "CC-BY-SA-4.0".into()));
    }
    lines.push(("entries".into(), entries.to_string()));
    lines.push((
        "builder".into(),
        format!("pime-dict {}", env!("CARGO_PKG_VERSION")),
    ));
    let mut out = String::new();
    for (k, v) in lines {
        out.push_str(&k);
        out.push('=');
        out.push_str(&v);
        out.push('\n');
    }
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::{Dictionary, RankCtx};
    use crate::score::{Day, ScoreParams};

    fn run(tsv: &str, opts: &BuildOptions) -> Result<(Vec<u8>, BuildReport), BuildError> {
        let mut r: &[u8] = tsv.as_bytes();
        let mut inputs: [&mut dyn BufRead; 1] = [&mut r];
        build_from_tsv(&mut inputs, opts)
    }

    fn displays(bytes: Vec<u8>) -> Vec<String> {
        let d = Dictionary::from_bytes(bytes).unwrap();
        (0..d.len() as u64).map(|i| d.display_for_test(i)).collect()
    }

    #[test]
    fn normalisation() {
        let n = |w: &str| normalise(w).map(|(d, _)| d);
        assert_eq!(n("كتاب").as_deref(), Some("کتاب"));
        assert_eq!(n("خانة").as_deref(), Some("خانه"));
        assert_eq!(n("مـَن").as_deref(), Some("من"));
        assert_eq!(
            n("\u{200C}می\u{200C}\u{200C}خواهم\u{200C}").as_deref(),
            Some("می\u{200C}خواهم")
        );
        // NFC composes alef + madda above into آ.
        assert_eq!(n("\u{0627}\u{0653}ب").as_deref(), Some("آب"));
        assert_eq!(n("abc"), None);
        assert_eq!(n("ب"), None, "one-letter keys are dropped");
        assert_eq!(n(&"ب".repeat(33)), None);
        assert!(n(&"ب".repeat(32)).is_some());
        assert_eq!(n("ۀ"), None);
        assert_eq!(n("سلام!"), None);
    }

    #[test]
    fn arabic_forms_merge() {
        let (bytes, rep) = run("كتاب\t5\nكتاب\t4\n", &BuildOptions::default()).unwrap();
        assert_eq!(rep.kept, 1);
        assert_eq!(rep.merged, 1);
        assert_eq!(rep.read, 2);
        let d = Dictionary::from_bytes(bytes.clone()).unwrap();
        let expected = (1e5f64 + 1e4).log10();
        assert_eq!(
            d.prior("کتاب"),
            Some(((expected * 10.0).round() / 10.0) as f32)
        );
        assert_eq!(displays(bytes), ["کتاب"]);
    }

    #[test]
    fn zwnj_display_preferred() {
        let tsv = "# comment\n\nمیخواهم\t5\nمی\u{200C}خواهم\t3\nمی خواهم\t6\n";
        let (bytes, rep) = run(tsv, &BuildOptions::default()).unwrap();
        assert_eq!(rep.read, 3);
        assert_eq!(rep.dropped_chars, 1);
        assert_eq!(rep.with_zwnj, 1);
        assert_eq!(displays(bytes), ["می\u{200C}خواهم"]);
    }

    #[test]
    fn filters_and_limits() {
        let tsv = "سلام\t5\nسلامت\t4\nسرد\t0.5\nبد\t3\n";
        let opts = BuildOptions {
            max_entries: 2,
            ..BuildOptions::default()
        };
        let (bytes, rep) = run(tsv, &opts).unwrap();
        assert_eq!(rep.kept, 2);
        assert_eq!(displays(bytes), ["سلام", "سلامت"]);
        let opts = BuildOptions {
            max_bytes: 100,
            ..BuildOptions::default()
        };
        assert!(matches!(run(tsv, &opts), Err(BuildError::TooLarge { .. })));
    }

    #[test]
    fn line_errors() {
        let o = BuildOptions::default();
        let e = |what| {
            Err(BuildError::Line {
                input: 1,
                line: 2,
                what,
            })
        };
        assert_eq!(run("سلام\t5\nسلام\n", &o), e("missing tab"));
        assert_eq!(run("سلام\t5\nسلام\t12\n", &o), e("zipf outside 0..=9"));
        assert_eq!(run("سلام\t5\nسلام\tx\n", &o), e("zipf is not a number"));
        assert_eq!(run("سلام\t5\nسلام\t1\t2\n", &o), e("more than two fields"));
        assert_eq!(run("سلام\t5\n\t1\n", &o), e("empty word"));
        let mut a: &[u8] = "سلام\t5\n".as_bytes();
        let mut b: &[u8] = b"x\t1\n\xff\t1\n";
        let mut inputs: [&mut dyn BufRead; 2] = [&mut a, &mut b];
        assert_eq!(
            build_from_tsv(&mut inputs, &o),
            Err(BuildError::Line {
                input: 2,
                line: 2,
                what: "not UTF-8"
            })
        );
        // CRLF, BOM and trailing whitespace are fine.
        let (_, rep) = run("\u{FEFF}سلام\t5 \r\nبد\t3\t\r\n", &o).unwrap();
        assert_eq!(rep.kept, 2);
    }

    #[test]
    fn info_and_determinism() {
        let opts = BuildOptions {
            info: vec![("source".into(), "wordfreq".into())],
            ..BuildOptions::default()
        };
        let tsv = "سلام\t5\nمی\u{200C}خواهم\t4\nكتاب\t3\n";
        let (a, _) = run(tsv, &opts).unwrap();
        let (b, _) = run(tsv, &opts).unwrap();
        assert_eq!(a, b);
        let d = Dictionary::from_bytes(a).unwrap();
        assert!(d
            .info()
            .starts_with("source=wordfreq\nlicence=CC-BY-SA-4.0\nentries=3\n"));
        let bad = BuildOptions {
            info: vec![("a=b".into(), "c".into())],
            ..BuildOptions::default()
        };
        assert_eq!(run(tsv, &bad), Err(BuildError::BadInfo));
    }

    #[test]
    fn empty_input_builds_empty_dictionary() {
        let (bytes, rep) = run("", &BuildOptions::default()).unwrap();
        assert_eq!(rep.kept, 0);
        let d = Dictionary::from_bytes(bytes).unwrap();
        assert!(d.is_empty());
        let ctx = RankCtx {
            history: None,
            today: Day(1),
            params: &ScoreParams::default(),
        };
        assert!(d.complete("سل", 5, &ctx).is_empty());
    }

    /// `data/dict/fixture.tsv` holds `word<TAB>count per billion`; Zipf = log10(count).
    #[test]
    fn fixture_tsv() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/dict/fixture.tsv");
        let Ok(raw) = std::fs::read_to_string(path) else {
            return; // the data stream may be regenerating it
        };
        let tsv: String = raw
            .lines()
            .filter_map(|l| {
                let (w, f) = l.split_once('\t')?;
                let f: f64 = f.trim().parse().ok()?;
                let zipf = if f > 9.0 { f.log10() } else { f };
                Some(format!("{w}\t{:.2}\n", zipf.clamp(0.0, 9.0)))
            })
            .collect();
        let (bytes, rep) = run(&tsv, &BuildOptions::default()).unwrap();
        assert!(rep.kept > 100, "{rep:?}");
        let d = Dictionary::from_bytes(bytes).unwrap();
        let ctx = RankCtx {
            history: None,
            today: Day(1),
            params: &ScoreParams::default(),
        };
        let got = d.complete("ب", 9, &ctx);
        assert!(!got.is_empty());
        assert!(got.iter().all(|c| c.text.starts_with('ب') && c.text != "ب"));
        assert!(got.windows(2).all(|w| w[0].score >= w[1].score));
    }
}
