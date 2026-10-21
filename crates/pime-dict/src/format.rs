//! `dict.bin` format version 1: layout constants and one-time validation
//! (docs/autocomplete.md 3.1). After [`validate`] succeeds no lookup can fail.

use std::fmt;
use std::ops::Range;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use fst::{Map, Streamer};

pub(crate) const MAGIC: &[u8; 8] = b"PIMEDICT";
pub(crate) const VERSION: u16 = 1;
pub(crate) const HEADER_LEN: usize = 64;
/// Largest `dict.bin` accepted.
pub(crate) const MAX_FILE: usize = 32 << 20;
pub(crate) const SECTIONS: usize = 5;
pub(crate) const SEC_FST: usize = 0;
pub(crate) const SEC_PRIOR: usize = 1;
pub(crate) const SEC_DISPLAY: usize = 2;
pub(crate) const SEC_TOP: usize = 3;
pub(crate) const SEC_INFO: usize = 4;
/// Most ids in one TOP entry.
pub(crate) const TOP_COUNT: usize = 64;
/// Prefixes whose range holds more keys than this get a TOP entry.
pub(crate) const TOP_THRESHOLD: usize = 256;
/// Largest PRIOR byte (Zipf 9.0).
pub(crate) const MAX_PRIOR: u8 = 90;

/// Why a dictionary could not be used. Never contains words or paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictError {
    /// The file does not exist.
    Missing,
    /// Reading failed.
    Io(std::io::ErrorKind),
    /// Larger than 32 MiB.
    TooLarge,
    /// Not a dictionary file.
    BadMagic,
    /// A format version this build does not read (treat as missing).
    UnsupportedVersion(u16),
    /// Structurally invalid; the text names the check that failed.
    Corrupt(&'static str),
}

impl fmt::Display for DictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DictError::Missing => f.write_str("dictionary file missing"),
            DictError::Io(kind) => write!(f, "dictionary file unreadable ({kind})"),
            DictError::TooLarge => f.write_str("dictionary file too large"),
            DictError::BadMagic => f.write_str("not a dictionary file"),
            DictError::UnsupportedVersion(v) => write!(f, "unsupported dictionary version {v}"),
            DictError::Corrupt(what) => write!(f, "dictionary file damaged ({what})"),
        }
    }
}

impl std::error::Error for DictError {}

/// A sub-range of the shared file buffer, usable as `fst` backing storage.
#[derive(Clone)]
pub(crate) struct Slice {
    buf: Arc<Vec<u8>>,
    range: Range<usize>,
}

impl AsRef<[u8]> for Slice {
    fn as_ref(&self) -> &[u8] {
        self.buf.get(self.range.clone()).unwrap_or(&[])
    }
}

/// Validated views into a `dict.bin` buffer.
pub(crate) struct Layout {
    pub n: usize,
    pub fst: Map<Slice>,
    pub top: Map<Slice>,
    pub prior: Range<usize>,
    /// Start of the `n + 1` u32 display offsets.
    pub display_offsets: usize,
    /// The display UTF-8 bytes.
    pub display_data: Range<usize>,
    /// The TOP u32 id array.
    pub top_ids: Range<usize>,
    pub info: Range<usize>,
}

pub(crate) fn read_u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

const fn corrupt(what: &'static str) -> DictError {
    DictError::Corrupt(what)
}

/// Validate everything once (magic, version, flags, bounds, crc, both fsts, PRIOR,
/// DISPLAY, TOP, INFO).
pub(crate) fn validate(buf: &Arc<Vec<u8>>) -> Result<Layout, DictError> {
    let b: &[u8] = buf;
    if b.len() > MAX_FILE {
        return Err(DictError::TooLarge);
    }
    if b.len() < 8 || &b[..8] != MAGIC {
        return Err(DictError::BadMagic);
    }
    if b.len() < HEADER_LEN {
        return Err(corrupt("truncated header"));
    }
    let version = read_u16(b, 8);
    if version != VERSION {
        return Err(DictError::UnsupportedVersion(version));
    }
    if read_u16(b, 10) != 0 {
        return Err(corrupt("flags"));
    }
    let n = read_u32(b, 12).unwrap_or(0) as usize;
    let crc = read_u32(b, 16).unwrap_or(0);
    if read_u32(b, 20) != Some(0) {
        return Err(corrupt("reserved field"));
    }
    if crc32fast::hash(&b[HEADER_LEN..]) != crc {
        return Err(corrupt("checksum"));
    }
    let mut secs: [Range<usize>; SECTIONS] = std::array::from_fn(|_| 0..0);
    for (i, sec) in secs.iter_mut().enumerate() {
        let off = read_u32(b, 24 + 8 * i).unwrap_or(0) as usize;
        let len = read_u32(b, 28 + 8 * i).unwrap_or(0) as usize;
        let end = off.checked_add(len).ok_or(corrupt("section bounds"))?;
        if off < HEADER_LEN || !off.is_multiple_of(8) || end > b.len() {
            return Err(corrupt("section bounds"));
        }
        *sec = off..end;
    }
    let mut sorted = secs.clone();
    sorted.sort_by_key(|r| r.start);
    if sorted.windows(2).any(|w| w[0].end > w[1].start) {
        return Err(corrupt("sections overlap"));
    }

    let prior = secs[SEC_PRIOR].clone();
    if prior.len() != n {
        return Err(corrupt("prior length"));
    }
    if b[prior.clone()].iter().any(|&p| p > MAX_PRIOR) {
        return Err(corrupt("prior value"));
    }

    let disp = secs[SEC_DISPLAY].clone();
    let table = n
        .checked_add(1)
        .and_then(|x| x.checked_mul(4))
        .filter(|&t| t <= disp.len())
        .ok_or(corrupt("display table"))?;
    let display_offsets = disp.start;
    let display_data = disp.start + table..disp.end;
    let data = &b[display_data.clone()];
    let mut prev = 0usize;
    for i in 0..=n {
        let off = read_u32(b, display_offsets + 4 * i).unwrap_or(u32::MAX) as usize;
        if (i == 0 && off != 0) || off < prev || off > data.len() {
            return Err(corrupt("display offsets"));
        }
        if i > 0 && std::str::from_utf8(&data[prev..off]).is_err() {
            return Err(corrupt("display encoding"));
        }
        prev = off;
    }
    if prev != data.len() {
        return Err(corrupt("display offsets"));
    }

    let top = secs[SEC_TOP].clone();
    if top.len() < 4 {
        return Err(corrupt("top section"));
    }
    let fst_len = read_u32(b, top.start)
        .map(|x| x as usize)
        .filter(|&l| l <= top.len() - 4)
        .ok_or(corrupt("top section"))?;
    let top_fst = top.start + 4..top.start + 4 + fst_len;
    let ids_start = top_fst.end.next_multiple_of(4);
    if ids_start > top.end || !(top.end - ids_start).is_multiple_of(4) {
        return Err(corrupt("top ids"));
    }
    let top_ids = ids_start..top.end;
    let id_count = top_ids.len() / 4;
    if (0..id_count).any(|i| read_u32(b, ids_start + 4 * i).is_none_or(|id| id as usize >= n)) {
        return Err(corrupt("top id"));
    }

    let info = secs[SEC_INFO].clone();
    if std::str::from_utf8(&b[info.clone()]).is_err() {
        return Err(corrupt("info encoding"));
    }

    let slice = |range: Range<usize>| Slice {
        buf: Arc::clone(buf),
        range,
    };
    let fst = check_fst(slice(secs[SEC_FST].clone()), |i, _key, v| v == i as u64, n)?;
    let top = check_fst(
        slice(top_fst),
        |_, _key, v| {
            let start = (v >> 8) as usize;
            let count = (v & 0xFF) as usize;
            (1..=TOP_COUNT).contains(&count)
                && start.checked_add(count).is_some_and(|e| e <= id_count)
        },
        usize::MAX,
    )?;

    Ok(Layout {
        n,
        fst,
        top,
        prior,
        display_offsets,
        display_data,
        top_ids,
        info,
    })
}

/// Open an fst and walk every entry once: checksum, UTF-8 keys, values per `ok`, and
/// (unless `want` is `usize::MAX`) exactly `want` entries. A panic inside `fst` on
/// adversarial bytes is reported as corruption.
fn check_fst(
    data: Slice,
    ok: impl Fn(usize, &[u8], u64) -> bool,
    want: usize,
) -> Result<Map<Slice>, DictError> {
    let walk = || -> Result<Map<Slice>, DictError> {
        let map = Map::new(data).map_err(|_| corrupt("fst"))?;
        map.as_fst().verify().map_err(|_| corrupt("fst checksum"))?;
        let mut stream = map.stream();
        let mut i = 0usize;
        while let Some((k, v)) = stream.next() {
            if std::str::from_utf8(k).is_err() || !ok(i, k, v) {
                return Err(corrupt("fst entry"));
            }
            i += 1;
        }
        if (want != usize::MAX && i != want) || map.len() != i {
            return Err(corrupt("fst length"));
        }
        drop(stream);
        Ok(map)
    };
    catch_unwind(AssertUnwindSafe(walk)).unwrap_or(Err(corrupt("fst")))
}
