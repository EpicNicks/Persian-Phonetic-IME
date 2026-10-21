# Autocomplete: interface-first contract

Status: contract for implementation (decisions D5, D15, D18, D20-D24 in `PLAN.md` section 7). Where this file and `PLAN.md` 2.3 / WS3 disagree, the decisions D18/D20-D24 and this file win (PLAN 2.3 still describes Latin-side, vowel-optional matching, which D18/D20 dropped). Anything not decided here is listed in section 11 (open questions); do not decide it silently.

Stream letters below are `AC-A`..`AC-H` (local to autocomplete; unrelated to the stream A/B/C names already used in `contracts.md`).

## 1. User-visible behaviour (summary)

- `[autocomplete] enabled = false` by default. Off means exactly the current behaviour (alternates of the last ambiguous letter, PLAN 2.2/2.3). **Why off by default:** it is the only feature that writes data derived from typing to disk (D24), so turning it on is the consent moment; it changes Down/Up/Space semantics for existing users; and the dictionary has not passed the evaluation gate (section 9). Revisit after the eval (Q1). `learning` defaults to `true` but only matters when `enabled`.
- When on, and the preedit (the Persian text of the word being typed) has at least `predict_min_chars` letters, the popup shows up to `max_completions` dictionary words that **start with the preedit** (plain prefix match on the Persian output, D20; no Latin-side matching, no vowel skipping, D18). They appear in the same popup, below the alternates, after a divider.
- Ranking is adaptive (D21): frequency prior + boost for words you accepted - penalty for words you passed over or dismissed, decaying over time.
- ZWNJ (D22): the dictionary's ZWNJ form is offered unless you already typed past that position without a ZWNJ (section 2.4).
- Never emits Arabic ي ك ة (D23): the dictionary is normalised at build time.
- Learning is local only (`%LOCALAPPDATA%\PersianPhoneticIme\history.bin`), toggleable, clearable, never recorded in disabled/password contexts, never logged (D24).
- Missing or corrupt `dict.bin`: autocomplete silently does nothing; typing is unaffected.

## 2. pime-core contract (owner AC-C)

### 2.1 Dependency direction

`pime-core` defines the completer **trait** and pure helpers; it never depends on `pime-dict`. `pime-dict` depends on `pime-core` and implements the trait. `pime-tip` and `pime-settings` depend on both. No cycle. `pime-core` goldens use the in-core `ListCompleter` fixture; ranking with a real dictionary is tested in `pime-dict`.

### 2.2 New module `pime_core::complete` (re-exported from the crate root)

AC-C lands this module first, exactly as below (bodies may be `todo!()` for a few hours), so AC-A and AC-E compile against it.

```rust
/// Implemented by pime-dict. Called on the key path: no I/O, no blocking beyond short
/// in-memory locks, target < 0.5 ms. Must not panic on any input.
pub trait Completer: Send + Sync {
    /// At most `limit` completions of `prefix` (the preedit), best first. Every `text`
    /// starts with `prefix` verbatim, differs from it, and is unique in the result.
    fn complete(&self, prefix: &str, limit: usize, ctx: &CompleteCtx) -> Vec<Completion>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteCtx { pub use_history: bool }          // = cfg.autocomplete.learning
#[derive(Debug, Clone, PartialEq)]
pub struct Completion { pub text: String, pub score: f32 }

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionList {
    pub items: Vec<String>,            // display order; row number = alternates.len() + i + 1
    pub highlighted: Option<usize>,    // None = the typed text is "selected"
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnEvent { pub word: String, pub kind: LearnKind }
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LearnKind { Skipped, Ignored, Dismissed, Accepted }   // ascending priority
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextPolicy { pub completions: bool, pub learning: bool }  // Default: both true

pub const ZWNJ: char = '\u{200C}';
/// Matching key: drop ZWNJ, U+0640 tatweel and U+064B..=U+065F, U+0670; map ي U+064A and
/// ى U+0649 -> ی U+06CC, ك U+0643 -> ک U+06A9, ة U+0629 -> ه U+0647. Nothing else.
pub fn match_key(s: &str) -> String;
/// The ZWNJ rule (2.4). None if `display` does not extend `prefix`.
pub fn complete_with_zwnj_rule(prefix: &str, display: &str) -> Option<String>;
/// True if `preedit` may be completed: non-empty, only U+0600..=U+06FF and ZWNJ,
/// at least one non-ZWNJ char.
pub fn completable(preedit: &str) -> bool;

/// Fixture completer for goldens, fuzz, demo, settings "try typing": linear scan, match by
/// `match_key` prefix, output via `complete_with_zwnj_rule`, order by score desc then text.
pub struct ListCompleter { /* private */ }
impl ListCompleter { pub fn new(words: impl IntoIterator<Item = (String, f32)>) -> Self; }
impl Completer for ListCompleter { /* ignores ctx.use_history */ }
```

### 2.3 Session and Output changes (additive)

```rust
pub struct Output { /* existing fields */
    pub completions: Option<CompletionList>,   // None = no completion section
    pub learn: Vec<LearnEvent>,                 // non-empty only on the Output that ends an episode
}
impl Session {
    pub fn set_completer(&mut self, c: Option<Arc<dyn Completer>>); // None = v1 behaviour
    pub fn set_context_policy(&mut self, p: ContextPolicy);         // per focused context
}
```

`Session::new(cfg, Arc<Dictionary>)` keeps its signature; the placeholder `Dictionary` stays a no-op (remove later). `CandidateKind::Completions` stays unused: completions travel in `Output.completions`, `Output.candidates` keeps meaning "alternates". Consumers that build `Output` literals must use `..Default::default()`. `Session` stays `Clone + Debug` (wrap the `Arc<dyn Completer>` in a newtype with a manual `Debug`).

Completions are queried in `on_key`/`select_candidate`/`set_config`/`set_completer` after the buffer changes (never in `snapshot`), and cached in the session with the prefix they belong to, so `would_eat` (which runs `on_key` on a clone) only re-queries for letter keys. They are shown only if all hold: `cfg.autocomplete.enabled`, a completer is set, `policy.completions`, the popup is not closed by Esc, `preedit_pending` is `None`, `completable(preedit)`, and the number of non-ZWNJ chars in the preedit >= `predict_min_chars`. Query `limit = min(max_completions, 9 - alternates.len())`; 0 means no section. The session drops any returned item that equals the preedit or does not start with it (defence in depth).

**Rows.** The popup has one row index space: rows `0..n` are the alternates (`Output.candidates.items`, unchanged numbering), rows `n..n+k` the completions. `1-9` and `select_candidate(row)` use this index.

### 2.4 ZWNJ rule and matching (D22)

Matching is ZWNJ-insensitive: `match_key(display)` must start with `match_key(prefix)`. Output never rewrites what was typed:

```
p   = match_key(prefix); n = p.chars().count()
D's first n non-ZWNJ chars (after the per-char mapping of match_key) must equal p, else None
tail = D after its n-th non-ZWNJ char            (may start with ZWNJ)
if tail has no non-ZWNJ char: None               (nothing to complete)
out  = prefix + (if prefix ends with ZWNJ { tail without leading ZWNJ } else { tail })
```

So `D = می‌خواهم`: `می` -> `می‌خواهم` (ZWNJ offered at the junction); `میخ` -> `میخواهم` (typed past the ZWNJ position without one, so none); `می‌خ` -> `می‌خواهم`. A ZWNJ the user typed where `D` has none is kept (`می‌` + `میخک` -> `می‌خک`). Arabic-only forms can appear in `out` only inside `prefix`, i.e. only if the user's own map allows them.

### 2.5 Keys while autocomplete is on

Only rows marked "(new)" differ from v1, and only while a completion section is visible.

| Key | Behaviour |
|---|---|
| letters, `'`, Backspace, Shift+Space | v1. Completion highlight resets to `None`; the list is re-queried. No learn event. |
| Tab / Shift+Tab | v1: cycle the alternates (changes the preedit, so completions re-query). With no alternates: v1 (commit, pass Tab through), even if completions are visible (Q2). |
| Down / Up (new) | Move the completion highlight: Down `None->0->1->..->k-1->None`, Up the reverse. Eaten. With no completions visible: v1 (alternates). After Esc closed the popup: Down/Up/Tab reopen it (v1), highlight `None`. |
| Enter (new) | Completion highlighted: **accept** it, key eaten whatever `enter` says (Q3). Otherwise v1. |
| Space (new) | Completion highlighted: accept + `" "`. No highlight and `space = "top-candidate"`: accept row `n` (first completion) + `" "`. Otherwise v1. |
| `1-9` (new) | `number_select` and popup visible: row `< n` is v1; row in `n..n+k` accepts that completion; other digits are eaten (v1). |
| Mouse click | `select_candidate(row)`: same mapping as digits. |
| Esc (new) | Popup visible: close the whole popup (v1 `list_closed`) and record **Dismissed** for every visible completion row. Popup closed: v1 cancel. |
| Right, Left, Home, End, Ctrl/Alt chords, focus loss | v1 (commit the preedit as typed). |

**Accept** = commit the completion text exactly (it already starts with the preedit), end the composition, reset the buffer, eat the key. Accepted text is never put back into the Latin buffer (there is no reverse mapping); a suffix such as ها is typed as the next composition, which is adjacent because accept inserts no space unless the key was Space.

### 2.6 Accept / reject / ignore (episodes, D21)

An **episode** is one composition: from the first key until it is committed, cancelled or terminated. The session collects events per episode and emits them in `Output.learn` on the Output that ends the episode, deduplicated by `match_key(word)` keeping the highest `LearnKind` (Accepted > Dismissed > Ignored > Skipped). "Visible list" = the completion rows in the most recent Output the session produced.

| Event | When |
|---|---|
| `Accepted(w)` | `w` was accepted (Enter, Space, digit, click, top-candidate Space). |
| `Skipped(v)` | On accept of row `j`, each completion row above it (`0..j`) in the visible list. |
| `Dismissed(v)` | Esc closed the popup while completions were visible: every visible completion row. |
| `Ignored(v)` | The preedit is committed as typed by Space, Enter, a digit or punctuation while completions are visible: the first `min(3, k)` rows of the visible list, except a row equal to the committed text. |
| none (neutral) | Committing text equal to a visible completion (typed through, Q4); lists shown earlier in the episode and typed past; Backspace; focus loss, navigation keys, Ctrl/Alt chords (Dismissed events already collected are still emitted); Esc cancel (collected events emitted); `on_composition_terminated` (events dropped); the `MAX_PREEDIT` flush. |

Events are produced only if `cfg.autocomplete.learning && policy.learning`. `would_eat` produces them on a discarded clone, so it stays side-effect free. Only words from completion rows (dictionary words) ever appear in events; free-typed text never does.

## 3. pime-dict (owner AC-A; no Windows deps)

Deps: `pime-core`, `fst = "0.4"`, `crc32fast`, `unicode-normalization` (build path only). Std `std::os::windows` is allowed behind `cfg(windows)` (file share modes); no `windows` crate.

**Why `fst`:** prefix ranges are the core query and `fst::Map` gives them directly (`range().ge(k).lt(succ(k))`), stores ~200k keys in a fraction of a sorted string array, needs no parse step (validated once, then used in place), and is mature pure Rust (MIT/Unlicense). A sorted array + binary search was the alternative: simpler, but about 2-3x larger in every process that loads it.

Modules: `format.rs` (dict.bin read/validate), `build.rs`, `dict.rs` (`Dictionary`, completion), `history.rs`, `score.rs`, `runtime.rs` (load/save threads, `LiveCompleter`), `eval.rs` (owned by AC-H).

### 3.1 dict.bin (format version 1, little-endian, arch-independent)

```
0   [u8;8]  magic b"PIMEDICT"
8   u16     format_version = 1        (other values: treat as missing)
10  u16     flags = 0                 (non-zero: reject)
12  u32     entry_count n
16  u32     crc32 of bytes 64..end
20  u32     reserved = 0
24  5 x (u32 offset, u32 len): FST, PRIOR, DISPLAY, TOP, INFO   (8-byte aligned, in bounds,
64  sections...                                                     non-overlapping)
```

- **FST**: `fst::Map`, key = `match_key(word)` (UTF-8), value = entry id; ids are `0..n` in key order.
- **PRIOR**: `n` bytes, Zipf frequency x 10, rounded, `0..=90`.
- **DISPLAY**: `n+1` u32 offsets then UTF-8 bytes; entry `i` = canonical display form (NFC, normalised, with ZWNJ).
- **TOP**: u32 fst length, `fst::Map` bytes, pad to 4, then a u32 id array. Key = a prefix key, value = `(start << 8) | count`, `count <= 64`: the 64 best ids by prior under that prefix. The builder emits an entry for **every** char-prefix whose FST range holds more than 256 keys, so any prefix without a TOP entry scans at most 256 keys.
- **INFO**: UTF-8 `key=value` lines (`source`, `wordfreq_version`, `wordlist`, `licence=CC-BY-SA-4.0`, `entries`, `builder`). No timestamps (reproducible).

Load validates everything once (magic, version, flags, bounds, crc, both fsts, PRIOR length, DISPLAY offsets monotonic and valid UTF-8, TOP ids `< n`, `fst.len() == n`, file <= 32 MiB). After that, no lookup can panic. The file is read fully into memory and closed: it is a few MB, it is loaded only in processes where autocomplete is enabled, and never holding it open means upgrades can replace it in place (memory mapping to share pages is Q5).

```rust
pub struct Dictionary { /* Vec<u8> + validated views */ }
pub enum DictError { Missing, Io(std::io::ErrorKind), TooLarge, BadMagic,
                     UnsupportedVersion(u16), Corrupt(&'static str) }  // never contains words or paths
impl Dictionary {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, DictError>;
    pub fn load(path: &Path) -> Result<Self, DictError>;    // file I/O: never on the key path
    pub fn len(&self) -> usize;
    pub fn info(&self) -> &str;
    pub fn prior(&self, word: &str) -> Option<f32>;          // Zipf, by match_key
    pub fn complete(&self, prefix: &str, limit: usize, rank: &RankCtx) -> Vec<pime_core::Completion>;
}
pub struct RankCtx<'a> { pub history: Option<&'a History>, pub today: Day, pub params: &'a ScoreParams }
```

`complete`: `k = match_key(prefix)`; empty `k` -> `[]`. Pool = TOP ids for `k` if present, else the FST range of `k`, plus (with history) every history key starting with `k` that exists in the FST. Score each (3.3), sort by score desc, prior desc, shorter display, display; map through `complete_with_zwnj_rule(prefix, display)` (drops the exact word), dedupe, truncate to `limit`. Exact unless more than 55 of a prefix's top-64 words carry net penalties (accepted approximation).

### 3.2 Build (library entry; AC-B's `xtask build-dict` is a thin wrapper)

```rust
pub struct BuildOptions { pub min_zipf: f32 /*1.0*/, pub max_entries: usize /*200_000*/,
                          pub max_bytes: usize /*16 MiB*/, pub info: Vec<(String, String)> }
pub struct BuildReport { pub read: usize, pub kept: usize, pub dropped_chars: usize,
                         pub merged: usize, pub with_zwnj: usize, pub bytes: usize }
pub fn build_from_tsv(inputs: &mut [&mut dyn BufRead], opts: &BuildOptions)
    -> Result<(Vec<u8>, BuildReport), BuildError>;   // BuildError carries line numbers
```

TSV: UTF-8, one `word<TAB>zipf` per line, `#` comments and blank lines skipped; `zipf` is a decimal in `0..=9` (wordfreq's Zipf scale). Several inputs (wordfreq export, then hand-curated extras) are merged. Per word: NFC; map ي ى -> ی, ك -> ک, ة -> ه; strip tatweel and U+064B..=U+065F, U+0670; trim ZWNJ at the ends and collapse ZWNJ runs; keep only if every char is in U+0621..=U+063A, U+0641..=U+0648, U+067E, U+0686, U+0698, U+06A9, U+06AF, U+06CC, ZWNJ (so U+0643/U+064A/U+0629 can never survive, D23) and the key has 2..=32 chars. Same key from several lines: frequency = `log10(sum 10^zipf)`, display = the most frequent variant **containing ZWNJ** if any, else the most frequent. Then drop `zipf < min_zipf`, keep the top `max_entries`, fail if output exceeds `max_bytes`.

### 3.3 Scoring (D21)

`Day(pub u32)` = days since 1970-01-01 UTC. History entry for a key: `accepts a`, `rejects r` (f32), `day`. Decay to `today`: `x' = x * 2^(-max(0, today - day) / H)` (clock going backwards counts as 0).

```
score(w) = prior(w) + clamp( alpha * log2(1 + a') - beta * log2(1 + r'), learned_min, learned_max )
Accepted:  a = min(a' + 1, cap); r = r' / 2; day = today
Skipped / Ignored / Dismissed:  a = a'; r = min(r' + skip | ignore | dismiss, cap); day = today
```

```rust
pub struct ScoreParams {            // default   bounds (clamped by ScoreParams::clamped)
    pub alpha: f32,                 // 1.5       0.0..=3.0
    pub beta: f32,                  // 1.0       0.0..=3.0
    pub half_life_days: f32,        // 30        7..=365
    pub skip: f32,                  // 0.25      0.0..=1.0
    pub ignore: f32,                // 0.5       0.0..=2.0
    pub dismiss: f32,               // 1.0       0.0..=4.0
    pub learned_min: f32,           // -4.0      -8.0..=0.0
    pub learned_max: f32,           // 6.0       0.0..=8.0
    pub cap: f32,                   // 64        1..=1000
}
```

Reading: one accept is +1.5 Zipf (about 30x frequency), enough to beat a word 1 Zipf ahead; one dismissal is -1; a word ignored twice is -1. Parameters are constants, not user config; AC-H tunes them with the eval (section 9).

### 3.4 History

```rust
pub struct History { /* epoch: u64, BTreeMap<String /*match_key*/, Entry> */ }
pub struct Entry { pub accepts: f32, pub rejects: f32, pub day: Day }
pub enum Rejection { Skipped, Ignored, Dismissed }
impl History {
    pub fn new() -> Self;                                   // epoch 0 = never persisted
    pub fn record(&mut self, ev: &pime_core::LearnEvent, today: Day, p: &ScoreParams);
    pub fn record_accept(&mut self, word: &str, today: Day, p: &ScoreParams);
    pub fn record_reject(&mut self, word: &str, how: Rejection, today: Day, p: &ScoreParams);
    pub fn learned(&self, key: &str, today: Day, p: &ScoreParams) -> f32;   // the clamped term
    pub fn decay(&mut self, today: Day, p: &ScoreParams);  // apply decay, prune a'<0.05 && r'<0.05,
                                                           // cap 10_000 entries (evict smallest a'+r', then oldest)
    pub fn prefix(&self, key_prefix: &str) -> impl Iterator<Item = (&str, &Entry)>;
    pub fn merge(&mut self, other: &History);  // per key: later day wins; same day: per-field max
    pub fn len(&self) -> usize; pub fn clear(&mut self); pub fn epoch(&self) -> u64;
    pub fn to_bytes(&self) -> Vec<u8>;
    pub fn from_bytes(b: &[u8]) -> Result<History, HistoryError>;
}
```

`history.bin` v1: `b"PIMEHIST"`, `u16 version = 1`, `u16 0`, `u64 epoch` (non-zero once written), `u32 count <= 10_000`, then per entry `u8 len (<=128), key UTF-8, f32 accepts, f32 rejects, u32 day`, then `u32 crc32` of everything before it; file <= 2 MiB. Any violation -> `HistoryError`, start empty, and keep the bad file once as `history.bin.bad` (overwritten next time). `Debug` for `History` prints only the entry count; no error type contains words.

### 3.5 Runtime: loading, key path, debounced save

```rust
pub struct RuntimePaths { pub dict: PathBuf, pub history: PathBuf }
pub struct RuntimeOptions { pub persist: bool, pub debounce: Duration /*10 s*/,
                            pub max_delay: Duration /*60 s*/, pub params: ScoreParams }
pub enum Status { Loading, Ready { words: usize }, Unavailable(DictError) }
impl Runtime {
    pub fn start(paths: RuntimePaths, opts: RuntimeOptions) -> Arc<Runtime>; // spawns, returns at once
    pub fn completer(self: &Arc<Self>) -> Arc<dyn pime_core::Completer>;   // LiveCompleter
    pub fn record(&self, events: &[pime_core::LearnEvent]);  // key path: in-memory, then notify saver
    pub fn status(&self) -> Status;
    pub fn request_save(&self);        // non-blocking
    pub fn on_clear_signal(&self);     // non-blocking; saver re-checks the file
    pub fn shutdown(&self);            // non-blocking: final save, then threads exit
    pub fn is_running(&self) -> bool;  // any runtime thread alive
}
```

- **Load** (thread `pime-dict-load`, exits after): `Dictionary::load`, then read `history.bin`. Until it finishes, `LiveCompleter::complete` returns `[]`. A dictionary error leaves `Status::Unavailable` forever (until restart) and the completer keeps returning `[]`.
- **Key path**: `complete` and `record` take short in-memory locks only (no I/O, no allocation beyond the result). `LiveCompleter` computes `Day` from `SystemTime` (a `fn() -> Day` clock is injectable for tests). `record` with `persist = false` still updates memory (learning lasts for the process).
- **Save** (thread `pime-dict-save`, waits on a condvar): fires `debounce` after the last change, at most `max_delay` after the first unsaved change, or on `request_save`/`shutdown`. It clones the history under the lock, then without the lock: take `history.lock` (exclusive open, 5 tries x 50 ms, else skip this round), read the file, apply the clear rule, `merge`, `decay`, write `history.bin.tmp`, rename over `history.bin`. Writes nothing while history is empty and no file exists.
- **Clear rule** (cross-process): clearing = deleting the file. If this runtime has a non-zero epoch and the file is missing or has a different epoch, someone cleared it: drop the in-memory history and adopt the file (or start a new epoch). A runtime with epoch 0 merges and adopts. New epochs: non-zero mix of time and process id.
- Thread bodies run under `catch_unwind`; a panic disables persistence for the process, never typing.

## 4. Candidate UI (owner AC-D: `crates/pime-candidate-ui/**`, `crates/pime-tip/src/candidates.rs`)

Same popup, a second section; no ghost text in the document (it would need fake composition text, confuses hosts, caret placement in RTL text and screen readers). Additive `CandidateView` fields:

```rust
pub completions: Vec<String>,          // rows after `items`, below a 1 px divider (theme border colour)
pub completion_highlight: Option<usize>,
pub completion_prefix_chars: usize,    // leading chars of each completion that were typed; drawn in the
                                       // secondary text colour, the rest in primary (0 = no dimming)
```

- Rows = `items ++ completions`, truncated to `MAX_ROWS` (9); `visible_len()` counts both; numbers continue across the divider (`row_number(n + i)`); `labels` apply to alternate rows only. New `selected_row() -> Option<usize>`: `n + j` if `completion_highlight = Some(j)`, else `highlighted` if `items` is non-empty, else `None` (no highlight drawn). `CandidateEvent::Hover/Click(i)` carry the combined row. Footer `"r/total"` over combined rows.
- RTL: completion rows right-aligned like alternates, logical-order text only, no bidi marks; ZWNJ shaped by DirectWrite. Row width = widest row of either section.
- UIA: one List, ListItems for all rows; completion items' `HelpText` "completion"; the highlight notification adds ", completion" for completion rows. UI-less `ITfCandidateListUIElement`: `GetCount`/`GetString`/`GetSelection` over combined rows (TSF has no sections).
- Adapter: `pub fn to_view_full(alts: Option<&CandidateList>, comps: Option<&CompletionList>, preedit: &str, candidate_labels: bool, footer: bool, size_percent: u32) -> CandidateView` (existing `to_view` unchanged). The popup is shown when either section is present.

## 5. TIP wiring (owner AC-E)

New `crates/pime-tip/src/autocomplete.rs`; hook edits listed in section 10.

- **Process-global** `Runtime` (one per host process, shared by all threads/contexts), started on the first activation whose config has `enabled = true` (not in `DllMain`). Ref-counted like `reload::start/stop`; the last deactivation calls `request_save` + `shutdown`. `DllCanUnloadNow` returns `S_FALSE` while `autocomplete::is_running()`.
- **Paths**: dictionary = `dict.bin` in the directory of this DLL (`GetModuleFileNameW` of our module; `PIME_DICT_PATH` overrides for development). History = `%LOCALAPPDATA%\PersianPhoneticIme\history.bin`, or `PIME_CONFIG_DIR\history.bin` (same rules as `pime_settings::paths`; a test checks they agree).
- `persist = false` in AppContainer processes (they cannot write the user's LocalAppData) and when the folder cannot be created.
- **Session**: after activation and every config reload, `session.set_completer(enabled.then(|| runtime.completer()))`; disabling also drops the process's runtime when no session uses it.
- **Learning**: after an `Output` from a real key/click/focus event has been handed to the edit session, call `runtime.record(&out.learn)` (never for `would_eat`).
- **Policy** per focused context (`OnSetFocus`/context push): `GUID_COMPARTMENT_KEYBOARD_DISABLED` / `EMPTYCONTEXT` already bypass the session; additionally InputScope `IS_PASSWORD`, `IS_NUMERIC_PASSWORD`, `IS_NUMERIC_PIN`, `IS_PRIVATE` (if present, Q6) -> `ContextPolicy { completions: false, learning: false }`; otherwise both true.
- **Clear**: on every `ConfigChanged` wake (existing reload watcher thread, off the key path) call `runtime.on_clear_signal()`.
- ETW: load time and word count, `Unavailable` kind, save success/failure counts. Never words, prefixes or paths containing the user name.

## 6. Config and settings

**Config (AC-C, pime-core).** `[autocomplete]` keys, validated by the existing `autocomplete_specs()` mechanism, existing friendly message kinds (full reference in `docs/config-schema.md` section 8):

| Key | Type | Default | Notes |
|---|---|---|---|
| `enabled` | bool | `false` | master switch (section 1) |
| `predict_min_chars` | integer >= 1 | `2` | counted in Persian letters of the preedit, ZWNJ not counted |
| `learning` | bool | `true` | off: no events, history not used for ranking, file untouched |
| `max_completions` | integer, 1-9, clamped | `5` | new; values above 9 become 9 silently (like `candidate_size`); 0 or less gets the existing "whole number of at least 1" error and the default is kept |

**Settings (AC-F, `crates/pime-settings/**`).**
- Options form: section "Word completion": "Suggest word completions" (`enabled`), "Learn from my choices" (`learning`), "Start suggesting after N letters" (1-10 spin), "Number of suggestions" (1-9 spin), plus one privacy line: "Learned words stay on this computer. Nothing is sent anywhere."
- "Clear learned words" button (confirm dialog) and CLI `pime-settings --clear-history`: delete `history.bin`, `history.bin.tmp`, `history.bin.bad`, then signal `Local\PersianPhoneticIme.ConfigChanged`. Exit 0 also when nothing existed; prints "Learned words cleared." Shows "N learned words" (count from `History::from_bytes`, never the words).
- `--check` adds one line: `Word completion dictionary: OK (N words)` / `missing or damaged; word completion is off.` (uses `pime_dict::Dictionary::load` on the installed `dict.bin`).
- "Try typing" box: real dictionary through `Runtime::start` with `persist: false` and `ContextPolicy { completions: true, learning: false }`; it never records or writes history.
- `default_config.toml` gains `max_completions = 5` with a comment.

## 7. Data pipeline and installer

**AC-B (data).** `tools/wordfreq/export.py` (offline, developer machine, `python -I`): `wordfreq.get_frequency_dict("fa", wordlist="best")` -> `data/dict/wordfreq-fa.tsv` (`word<TAB>zipf`, 2 decimals, sorted by zipf desc). `data/dict/SOURCE.txt` records wordfreq version, wordlist name, date, row count. `data/dict/extra.tsv` holds hand-curated additions (ZWNJ forms, Q7). `xtask build-dict --input data/dict/wordfreq-fa.tsv --input data/dict/extra.tsv --out <path>` prints the `BuildReport`. Licence: wordfreq code is Apache-2.0 and its data CC BY-SA 4.0 (verify, D15); ship `data/dict/NOTICE.txt` (attribution to wordfreq, its upstream sources as listed in its README, the CC BY-SA 4.0 notice, and that `dict.bin` is a derived database under CC BY-SA 4.0) and `data/dict/CC-BY-SA-4.0.txt`. The code licence (MIT) is unaffected: `dict.bin` is a separate data file.

**AC-G (installer).** `installer/build.ps1` runs `cargo run -p xtask --release -- build-dict ...` into the stage dir (fails the build if missing). `Package.wxs`: component `Dictionary` with `dict.bin`, `THIRD-PARTY-NOTICES.txt` (from `NOTICE.txt`) and `licenses\CC-BY-SA-4.0.txt` in `INSTALLFOLDER`, next to both DLLs (one file serves x64 and x86). `INSTALLFOLDER` is versioned, so an upgrade installs the new `dict.bin` beside the old one; the old copy goes with the old version (at reboot if a host still maps it). Uninstall removes it; per-user `history.bin` is user data and is not removed (Q8). `installer/README.md` checklist: install without `dict.bin` -> typing works, completion silent.

## 8. Golden formats (additions to `contracts.md` "Golden test format")

`data/golden/autocomplete.json` (AC-C; run by `crates/pime-core/tests/golden.rs`). Session cases gain optional fields:

```jsonc
{ "type": "session", "name": "down enter accepts",
  "config": { "preset": "qamooscheh", "toml": "[autocomplete]\nenabled = true\n" },
  "completer": { "words": [["سلام", 5.0], ["سلامت", 4.2], ["سلطان", 4.0]] },  // ListCompleter
  "policy": { "completions": true, "learning": true },                          // optional
  "keys": ["s", "l", "Down", "Enter"],
  "committed": "سلام", "preedit": "",
  "completions_after_last": null,            // optional: visible rows after the last key, or null
  "completion_highlight_after_last": null,   // optional
  "learn": [["accepted", "سلام"]] }          // optional: Output.learn of all keys, concatenated, in order
```

`data/golden/dict.json` (AC-H; run by `crates/pime-dict/tests/golden.rs`, dictionary built in memory with `build_from_tsv`):

```jsonc
{ "type": "complete", "name": "zwnj offered", "tsv": "می‌خواهم\t4.8\nمیز\t4.5\n",
  "history": [{ "word": "میز", "events": ["dismissed"], "day": 20000 }],   // optional, replayed in order
  "today": 20000, "use_history": true, "prefix": "می", "limit": 5,
  "expect": ["می‌خواهم", "میز"] }                                         // exact, ordered
{ "type": "build", "name": "arabic forms normalised", "tsv": "كتاب\t5\nكتاب\t4\n",
  "expect_display": ["کتاب"], "expect_report": { "kept": 1, "merged": 1 } }
```

## 9. Evaluation (AC-H)

- **Set**: `data/dict/eval/words.tsv`, `word<TAB>note`, 300-500 common words and ZWNJ forms, curated independently of wordfreq's ranking (source: Q9). Optional `data/dict/eval/corpus.txt` (one sentence per line) for the adaptive run.
- **Static metrics** (`cargo xtask eval-dict --dict <path> --set <tsv> [--json]`, logic in `pime_dict::eval`): for each word with key length `m >= 3` and each `L` in `predict_min_chars..m`, prefix = the word up to its `L`-th letter (ZWNJs before it included, as a correct typist would), empty history, `limit = 9`. Report hit@1 and hit@5 per `L` = 2, 3, 4 and overall, MRR, the share of words that are hit@1 at some `L`, and mean letters saved `(m - L* - 1) / m` where `L*` is the first `L` with hit@1.
- **Adaptive metric**: replay the corpus words through a real `Session` + `LiveCompleter` (persist off, fixed clock advancing one day per 500 words); the simulated user accepts when the target is visible (Down presses as needed), else types it out. Report hit@1 over the second half with learning on vs off.
- **Performance**: p99 `complete()` latency over all eval prefixes, release build (target < 0.5 ms).
- Gates (thresholds for turning `enabled` on by default, or accepting `ScoreParams` changes): Q1.

## 10. Work breakdown

Prerequisite P0 (coordinator, before AC-A/AC-B start, about 15 minutes): add workspace members `crates/pime-dict` and `xtask` to the root `Cargo.toml` with compiling skeletons (`lib.rs` declaring the modules of section 3, `xtask/src/main.rs` dispatching `build-dict` and `eval-dict`). Hook edits in files owned by other streams are limited to the lines listed and must be sequenced after those files' current owners land their work.

| Stream | Scope | Owns (exclusive) | Depends on | Start |
|---|---|---|---|---|
| AC-A pime-dict | format, build fn, `Dictionary::complete`, History, scoring, Runtime/LiveCompleter, unit tests | `crates/pime-dict/**` except `src/eval.rs`, `tests/**` | P0; AC-C's `complete.rs` signatures (2.2) | immediately (format/build/history first) |
| AC-B data pipeline | wordfreq export, licence verification, extras, `xtask build-dict` | `tools/wordfreq/**`, `data/dict/*` (not `eval/`), `xtask/**` except `src/eval.rs` | P0; AC-A `build_from_tsv` for the xtask wrapper | immediately |
| AC-C core | `complete` module, Session/Output changes, key table 2.5, episodes 2.6, `max_completions`, `ListCompleter`, autocomplete goldens + runner | `crates/pime-core/**`, `data/golden/autocomplete.json` | none | immediately; land 2.2 signatures first |
| AC-D candidate UI | CandidateView fields, two-section layout, UIA, `to_view_full`, UI element over combined rows | `crates/pime-candidate-ui/**`, `crates/pime-tip/src/candidates.rs` | AC-C types for `to_view_full` only | immediately (library part) |
| AC-E TIP wiring | runtime lifecycle, paths, policy, record, clear signal, ETW | `crates/pime-tip/src/autocomplete.rs`; hooks: `lib.rs` (mod line), `Cargo.toml` (pime-dict dep), `service.rs` (activate/deactivate, set_completer, record after apply, policy on focus, show popup when either section present), `exports.rs` (`DllCanUnloadNow`), `reload.rs` (one call) | AC-A Runtime API, AC-C Session API, AC-D `to_view_full` | after AC-A/AC-C land signatures |
| AC-F settings | form section, clear-history button + CLI, `--check` line, try-typing completer, default config | `crates/pime-settings/**` | AC-C config key; AC-A `Dictionary::load`, `History::from_bytes` | immediately (clear-history, form); dict lines later |
| AC-G installer | build step, WiX component, notices, checklist | `installer/**` | AC-B (xtask + notices); placeholder `dict.bin` meanwhile | immediately |
| AC-H tests/eval | dict goldens + runner, `pime_dict::eval`, `xtask eval-dict`, eval set, fuzz targets (`dict_from_bytes`, `history_from_bytes`, session with `ListCompleter`), latency bench | `data/golden/dict.json`, `data/dict/eval/**`, `crates/pime-dict/src/eval.rs`, `crates/pime-dict/tests/**`, `xtask/src/eval.rs`, new files under `fuzz/fuzz_targets/` (+ their `[[bin]]` entries, coordinated with the fuzz owner) | AC-A for running; none for authoring | immediately (data and goldens) |

Docs: this file, `contracts.md` "Autocomplete" and `config-schema.md` section 8 are the contract; streams report needed changes instead of editing them. Each stream adds its own row to `PLAN.md` section 9 only if the user asks (WS3 row covers AC-A/B/H).

## 11. Open questions (not decided here)

- **Q1** Eval gates: which hit@5 / letters-saved thresholds turn `enabled` on by default, and which gate a `ScoreParams` change.
- **Q2** Should Tab accept the first completion when there are no alternates (IDE habit), instead of v1's commit-and-pass?
- **Q3** Enter accepting a highlighted completion is always eaten, even with `enter = "commit-and-pass"` (so chat apps do not send). Confirm.
- **Q4** Typing a visible completion out in full and committing it is neutral; should it count as Accepted?
- **Q5** Memory-map `dict.bin` to share pages across host processes (needs versioned dictionary file names like the DLLs) instead of reading it into each process.
- **Q6** Exact InputScope list for "no completions, no learning" (is there a usable in-private signal from browsers?); should email/URL scopes also disable completions?
- **Q7** Does the wordfreq `fa` list contain ZWNJ words, or did its tokeniser split them (می / خواهم)? If split, ZWNJ forms must come from `extra.tsv` or a rule; AC-B reports the count (`BuildReport.with_zwnj`).
- **Q8** Should uninstall offer to delete per-user `history.bin`?
- **Q9** Source of the evaluation word list and corpus (must be independent of wordfreq).
- **Q10** Should a missing/damaged dictionary while `enabled = true` show a language-bar badge or settings-page notice, or stay silent beyond `--check`?
- **Q11** `learning = false` currently also stops using existing history for ranking; confirm (alternative: read-only use).

## 12. Implementation notes (as built)

Golden format additions: session cases accept `candidates_after_last`; the runner also understands the steps `"Click<row>"` (0-based combined row, calls `select_candidate`), `"FocusLost"` and `"Terminated"`, and checks `would_eat` against `on_key` for every key. The pime-core runner skips the `complete` and `build` cases of `dict.json`.
Deviations accepted: the session also drops completions whose added part contains U+064A, U+0649, U+0643 or U+0629; the ZWNJ rule returns None when the tail has no letters after normalisation; picking an alternate by digit or click while completions are visible records Ignored (Tab committing is neutral); backspacing to an empty word ends the episode; the adaptive eval replays `Dictionary` + `History` with the 2.6 rules rather than a `Session`. Callers must call `snapshot()` after `set_completer`, `set_context_policy` or `set_config` to redraw an open word. Defaults chosen for open questions: Q2 Tab does not accept; Q3 Enter accepting is always eaten; Q4 neutral; Q8 uninstall keeps history; Q11 `learning = false` stops using history for ranking. Still open: Q1 (eval gates), Q5, Q6, Q9, Q10; whether `BuildReport.merged` counts rows or keys; whether zipf outside 0..=9 is an error or a dropped row.

## 13. Fuzzy (edit-distance) matching and the profanity filter (user-approved 2026-10-21)

Supersedes the pure-prefix rule of D20 **only as a fallback**: exact prefix matches are unchanged and always rank first. Defaults chosen from the user's "all sound good" to the recommendations: both options are on when `enabled = true`; autocomplete itself stays off by default.

**Config** (`[autocomplete]`, AC-C validates; AC-F exposes toggles): `fuzzy = true` (bool; typo-tolerant suggestions), `fuzzy_min_chars = 3` (integer >= 2; preedit letters needed before fuzzy matches are tried; ZWNJ not counted), `filter_profanity = true` (bool; hide dictionary words flagged as vulgar). `CompleteCtx` gains `pub fuzzy: bool, pub fuzzy_min_chars: usize, pub hide_flagged: bool` (Copy; constructed from cfg; `use_history` stays).

**Fuzzy match definition** (AC-A): `k = match_key(preedit)`, `n = k.chars().count() >= fuzzy_min_chars`. A dictionary key `w` fuzzy-matches if the Damerau-Levenshtein distance (insert, delete, substitute, adjacent transposition each cost 1, over `match_key` chars, ZWNJ already removed) between `k` and some **prefix** of `w` is `d` with `1 <= d <= maxd(n)`, where `maxd(n) = 1` for `n <= 5` and `2` for `n >= 6`. (So a typo anywhere in the typed prefix still completes: `سلم` -> `سلام`, `کتب` -> `کتاب`; `d = 0` is the exact path.) Implement as a custom `fst::Automaton` (DP row state, sticky once the whole input is aligned within budget, like `starts_with`), not the whole-word `fst` Levenshtein. Bounded work: visit at most 4096 FST nodes/keys per query and stop; this must keep p99 < 0.5 ms on the real list (measure). Fuzzy is only run when the exact path returned fewer than `limit` results, and fills the remaining slots. Fuzzy results are ordered by `(edits asc, score desc, shorter display, display)`; exact results are never displaced. History boosts and penalties apply to the score as usual. Words equal to the preedit's own key are excluded.

**Type changes (additive, AC-C lands first)**: `Completion` gains `pub edits: u8` (0 = exact prefix completion; `Completion::new`/struct literals elsewhere must set it). `CompletionList` gains `pub edits: Vec<u8>` parallel to `items`. Contract text "every `text` starts with `prefix` verbatim" now holds only for `edits == 0`; for `edits > 0` the text replaces the whole preedit (no ZWNJ-tail rule; output = the dictionary display form). Session's defence-in-depth drop applies only to `edits == 0` items; `edits > 0` items are still dropped if they equal the preedit or contain U+064A/U+0649/U+0643/U+0629. `ListCompleter` implements the same fuzzy definition by linear scan (so session goldens can cover it) honouring `ctx.fuzzy`/`fuzzy_min_chars`. Accepting a fuzzy row commits its text exactly (replacing the preedit), like any accept. Learning: fuzzy rows only ever produce `Accepted` events; they never produce Skipped, Ignored or Dismissed (an unrelated-looking suggestion must not be penalised). The popup draws fuzzy rows with no prefix dimming (`completion_prefix_chars` applies to exact rows only) and AC-D adds a per-row marker: `CandidateView.completion_fuzzy: Vec<bool>` (additive; UIA HelpText "completion, approximate match"). `Output.completions` rows keep the single combined index space.

**Profanity flag** (AC-A/AC-B): the build TSV gains an optional third column `flags`; the letter `p` marks a word as vulgar. Flagged words stay in `dict.bin` (they must remain matchable only when `filter_profanity = false`). Format bump: `dict.bin` format_version = 2; PRIOR byte bit 7 = flagged (value bits 0..=90 unchanged); loaders reject version 1. `build_from_tsv` merges flags with OR. `RankCtx` gains `hide_flagged: bool`; `Dictionary::complete` skips flagged entries when set (exact and fuzzy). `BuildReport` gains `flagged: usize`. AC-B: `tools/wordlist/build_wordlist.py` emits blocklisted words with the `p` flag instead of dropping them, `xtask build-dict` passes them through, the blocklist stays in `tools/wordlist/blocklist.txt` and the user may extend it.

**Settings (AC-F)**: toggles "Suggest corrections for typos" (`fuzzy`), "Hide vulgar words" (`filter_profanity`), plus `fuzzy_min_chars` spin (2-10) under the word-completion section; Try typing honours them.

**Evaluation (AC-H)**: add typo robustness metrics to `eval-dict`: for each eval word and prefix length `L` in 3..=5, apply one random-but-seeded edit (substitution/deletion/insertion/transposition) inside the prefix and report hit@5 for fuzzy on vs off; report p99 latency of fuzzy queries; goldens for fuzzy ordering (exact first), `edits` values, flagged filtering both ways, fuzzy disabled below `fuzzy_min_chars`, history boost on fuzzy words.

### 13.1 As built

An empty `flags` column means not flagged; letters other than `p` are ignored and non-letters are a line error. `fuzzy_min_chars` below 2 is treated as 2. The fuzzy walk is capped (3072 nodes walked, 4096 total); about 18% of walks hit the cap, mostly 2-edit ones, and the cut-off branches are the least promising. Fuzzy ranking scores the best 64 candidates by prior plus up to 256 history-boosted words from the matched ranges. Measured on the 27k-word list (release): fuzzy p99 about 260-330 us, exact p99 about 50-200 us. Typo eval (789 queries, seeded edits at L = 3..5): hit@5 40.2% with fuzzy against 0% without; deletions do worst (6.2%) because a deletion at L = 3 falls below `fuzzy_min_chars`. `کس` is flagged and so stops completing while `filter_profanity` is on (tell the maintainer to unflag if too aggressive).
