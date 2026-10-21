# Cross-workstream contracts (wave 1)

Source of truth for behaviour is `PLAN.md` (read it first). This file pins the interfaces parallel workstreams code against. If you must change a contract, edit this file and say so in your final report; do not silently diverge.

## Ground rules for every agent

- Work only inside the files you own (table below). Do not edit another workstream's files, `PLAN.md`, or the root `Cargo.toml` (ask in your report if the workspace needs changing).
- Use your own cargo target dir to avoid build-lock contention: set `CARGO_TARGET_DIR` to `E:\Code\Projects\persian phonetic ime\target-<yourid>` (e.g. `target-ws2`). Never run plain `cargo` that writes to `target\`.
- **Never register the IME with Windows, never run regsvr32, never call `InstallLayoutOrTip`/`RegisterProfile` on this machine.** An unstable text service breaks typing in every app. Building is fine; registration is tested later in a VM/Sandbox by the user.
- Read-only on the Qamooscheh repos (`E:\Code\Projects\Qamooscheh-*`, `C:\Users\epicn\source\repos\Qamooscheh`).
- No network access to the IME at runtime, no telemetry, no logging of typed content.
- No git commits (the repo is not initialised and the user has not asked for commits).
- Match the surrounding style; keep comments to the non-obvious "why".
- Final report: what you built, how to run its tests, what is unfinished or uncertain, and any contract changes.

## Ownership

| ID | Owns |
|---|---|
| WS0 | `data/**`, `docs/config-schema.md` |
| WS1 | `spike/**`, `docs/spike.md` |
| WS2 | `crates/pime-core/**` |
| WS6a | `crates/pime-candidate-ui/**` |
| WS7 | `crates/pime-settings/**` |
| WS8 | `crates/pime-setup/**`, `installer/**`, `crates/pime-ids/**` (frozen GUIDs; edit only the doc comments) |
| WS4+5 (stream A) | `crates/pime-tip/src/{lib.rs (except mod lines),exports.rs,service.rs,keysink.rs,composition.rs,presenter.rs,...}` + `crates/pime-tip/Cargo.toml` + `.cargo/config.toml` |
| WS6 (stream B) | `crates/pime-tip/src/candidates.rs`, `crates/pime-candidate-ui/**` |
| WS7 rest (stream C) | `crates/pime-tip/src/{config,langbar,reload}.rs` |
| WS10 | `README.md`, `CLAUDE.md`, `.gitignore`, `.github/**`, `rustfmt.toml`, `docs/DESIGN.md`, `docs/contracts.md` |

## Config schema

PLAN.md section 3.3 is the schema. A preset file (`data/presets/<name>.toml`) uses the same TOML shape as a user config minus the `preset` key (it is a base). WS0 writes `docs/config-schema.md` as the precise reference (every option, allowed values, the map patch each `[options]` value expands to); WS2 implements it. If WS0's reference and PLAN.md disagree, PLAN.md wins and WS0 notes the discrepancy.

## Golden test format

Files: `data/golden/*.json`. Each: `{ "version": 1, "cases": [ ... ] }`. A case has `name` (unique string) and `type`:

```jsonc
// typing: feed Latin chars one at a time through Session, check the preedit
{ "type": "typing", "name": "digraph sh", "config": "qamooscheh",
  "input": "sh", "preedit_after_each": ["س", "ش"], "final_preedit": "ش",
  "candidates_after_last": null }          // or ["ش"] / ["غ","ق"] etc; optional

// session: scripted keys, check committed text and final state
{ "type": "session", "name": "space commits", "config": "qamooscheh",
  "keys": ["s","h","a","b","Space"], "committed": "شاب ", "preedit": "" }

// config_error: load a user config and check fallback + messages
{ "type": "config_error", "name": "bad value skips one option",
  "user_config_toml": "[options]\ngh = \"kaf\"\ndigits = \"ascii\"\n",
  "expect": { "fallback": "entry",            // "entry" | "whole_file" | "none"
              "applied": { "digits": "ascii" },
              "messages_contain": ["line 2", "kaf", "ghain", "qaf"] } }
```

`config` is a built-in preset name, or an object `{ "preset": "qamooscheh", "toml": "<user config toml>" }`.

Key names in `keys`: a single printable character for letters, digits and punctuation (`"s"`, `"1"`, `","`, `"'"`); named keys `"Space"`, `"Enter"`, `"Escape"`, `"Backspace"`, `"Tab"`, `"Up"`, `"Down"`, `"Left"`, `"Right"`, `"ShiftSpace"`, `"ShiftTab"`, `"Delete"`, `"Home"`, `"End"`, `"CapsLock"`, `"Ctrl+x"`, `"Alt+x"` (modifier plus key `x`) and `"Numpad0"`..`"Numpad9"`. A session case may also give `last_eaten`. Uppercase letters (`"S"`) mean Shift+letter.

## Session API (pime-core, public)

Declared in PLAN.md section 5.1. Concrete contract for consumers (WS6a mirrors the candidate shape; the future TIP crate consumes all of it):

```rust
pub struct KeyEvent { pub vk: u32, pub shift: bool, pub ctrl: bool, pub alt: bool }
pub struct Output {
    pub commit: String,
    pub preedit: String,
    pub preedit_pending: Option<std::ops::Range<usize>>, // UTF-16 code-unit range inside `preedit`
    pub candidates: Option<CandidateList>,
    pub eaten: bool,
    pub zwnj_toggle: bool, // (WS2 addition) idle Shift+Space: commit is U+200C; if the char
                           // before the caret is already U+200C, delete it instead of inserting
}
pub struct CandidateList { pub items: Vec<String>, pub highlighted: usize, pub kind: CandidateKind }
pub enum CandidateKind { Alternates, Completions }   // Completions is phase 2, unused in v1
```

`pime-core` has no Windows dependencies and must build and test on any host.

Semantics pinned by WS2: apply an `Output` as commit `commit` (ends any composition), then show `preedit` ("" = none), then pass the key on if `!eaten`. `preedit_pending` covers only the trailing run of unmapped Latin letters (a lone `c` still waiting for `h`); an unmapped letter mid-word stays visible as Latin but is not pending. `Session::new` takes `Arc<Dictionary>`, a phase-2 placeholder (`Dictionary::empty()`). Extras: `Session::snapshot()` (redraw after `set_config`), `Session::is_composing()`, `KeyEvent::plain/shifted`. Virtual-key constants and the US-QWERTY table are in `pime_core::keys`.

Config helpers for hosts and the settings app (pime-core, public):

```rust
pub fn option_specs() -> &'static [OptionSpec];        // every [options] key, schema order
pub fn autocomplete_specs() -> &'static [OptionSpec];  // [autocomplete]
pub struct OptionSpec { pub name: &'static str, pub kind: OptionKind, /* private default */ }
impl OptionSpec { pub fn default_value(&self) -> Option<OptionValue>; pub fn allowed_values(&self) -> &'static [&'static str]; }
pub enum OptionKind { Choice(&'static [&'static str]), Bool, List, Integer { min: i64 } }
pub fn format_issues_for(file_name: &str, issues: &[ConfigIssue]) -> String; // format_issues = "config.toml"
pub fn decode_config_bytes(bytes: &[u8]) -> Result<DecodedConfig, ConfigIssue>; // Err = whole-file issue
pub struct DecodedConfig { pub text: String, pub encoding: TextEncoding }
pub enum TextEncoding { Utf8, Utf8Bom, Utf16Le, Utf16Be }
pub fn load_config_bytes(bytes: &[u8]) -> LoadedConfig; // decode + load_config; not text -> defaults + 1 issue
```

`[options] candidate_position = "below" | "above"` (default `"below"`) is exposed as `options().candidate_position: pime_core::config::CandidatePosition { Below, Above }`; the candidate window opens on that side of the preedit and still flips when there is no room.

`option_specs()` is the table the loader validates against (the "Use ..." hints are generated from it), so it is the single source of truth for option names, kinds, allowed values and defaults. Decoding: UTF-8 with or without BOM, UTF-16 LE/BE with BOM; anything else is one `IssueKind::Syntax` issue with line 0 (`fallback_of` = `WholeFile`). Both the TIP and the settings app must read the file with `decode_config_bytes` / `load_config_bytes`.

## Candidate window input (pime-candidate-ui)

The library does not depend on `pime-core` (only its demo does, as a dev-dependency); it defines its own plain input struct, and `pime_tip::candidates::to_view` is the adapter:

```rust
pub struct CandidateView { pub items: Vec<String>, pub highlighted: usize, pub show_numbers: bool,
                           pub labels: Option<Vec<String>>, pub show_footer: bool /* "2/3" */,
                           pub size_percent: u32 /* 0 means 100; multiplies the DPI scale */ }
impl CandidateWindow {
    pub fn new(on_event: impl FnMut(CandidateEvent) + 'static) -> Result<Self>; // Hover(i) | Click(i)
    pub fn update(&self, view: &CandidateView);           // relayout; empty list hides
    pub fn set_highlight(&self, index: usize);            // no relayout
    pub fn show(&self, anchor: Rect);                     // = show_at(anchor, false)
    pub fn show_at(&self, anchor: Rect, prefer_above: bool); // physical px; flips when no room
    pub fn hide(&self); pub fn is_visible(&self) -> bool; pub fn highlight(&self) -> usize;
}
pub fn unregister_class() -> bool; // optional tidy-up before DLL unload
```

`size_percent` comes from `[options] candidate_size` (25-300, clamped by the config loader, default 50); `CandidateView::size_factor()` returns `size_percent / 100` (1.0 when 0).

Hover only tints the row under the pointer; the highlight (selection) is always the session's.

## Candidate presenter (pime-tip, stream B -> stream A)

`presenter.rs` (`CandidatePresenter`, `ScreenRect`, `PreferredSide`) is unchanged. `candidates.rs` adds, without touching the pinned trait:

```rust
pub fn create_presenter() -> Box<dyn CandidatePresenter>;                 // popup only
pub fn create_presenter_with(opts: PresenterOptions) -> Box<dyn CandidatePresenter>;
pub fn register_ui_element(thread_mgr: &ITfThreadMgr, candidate_labels: bool, size_percent: u32) -> Box<dyn CandidatePresenter>;
pub fn to_view(list: &CandidateList, candidate_labels: bool, footer: bool, size_percent: u32) -> CandidateView;
pub struct PresenterOptions { pub candidate_labels: bool, pub hide_footer: bool, pub size_percent: u32 /* 0 = 100 */, pub thread_mgr: Option<ITfThreadMgr> }
pub struct PopupPresenter; // impl CandidatePresenter; new(opts), set_candidate_labels(bool)
pub fn to_physical(hwnd: HWND, r: ScreenRect) -> ScreenRect;  // GetTextExt (logical) -> physical
pub fn caret_fallback_anchor() -> Option<ScreenRect>;         // GetGUIThreadInfo caret, physical
pub fn is_degenerate(r: ScreenRect) -> bool;                  // all-zero / inverted rect
```

With `thread_mgr` set, each list is also published as an `ITfCandidateListUIElement` (+ Behavior) via `ITfUIElementMgr::Begin/Update/EndUIElement` from inside `show`/`set_highlight`/`hide`; our window is drawn only when the host allows (`pbShow`, `ITfUIElement::Show`, not `TF_TMF_UIELEMENTENABLEDONLY`). Behavior `SetSelection` moves the popup/element highlight only; `Finalize` calls the `set_on_click` callback with the element's selection; `Abort` is a no-op. A degenerate anchor reuses the last anchor, then the caret fallback, then the mouse position.

## Autocomplete (phase 2, D5/D20-D24)

Full contract, rationale, key table and work breakdown: [`autocomplete.md`](autocomplete.md). Pinned here so consumers can code against it; `autocomplete.md` wins on detail.

Ownership (streams AC-A..AC-H, exclusive; hook edits limited to the lines listed in `autocomplete.md` section 10):

| ID | Owns |
|---|---|
| AC-A | `crates/pime-dict/**` except `src/eval.rs`, `tests/**` |
| AC-B | `tools/wordfreq/**`, `data/dict/*` (not `eval/`), `xtask/**` except `src/eval.rs` |
| AC-C | `crates/pime-core/**`, `data/golden/autocomplete.json` |
| AC-D | `crates/pime-candidate-ui/**`, `crates/pime-tip/src/candidates.rs` |
| AC-E | `crates/pime-tip/src/autocomplete.rs` + hooks in `service.rs`, `exports.rs`, `reload.rs`, `lib.rs`, `Cargo.toml` |
| AC-F | `crates/pime-settings/**` |
| AC-G | `installer/**` |
| AC-H | `data/golden/dict.json`, `data/dict/eval/**`, `crates/pime-dict/src/eval.rs`, `crates/pime-dict/tests/**`, `xtask/src/eval.rs`, new `fuzz/fuzz_targets/dict_*.rs` |

Dependency direction: `pime-dict -> pime-core`; `pime-tip`, `pime-settings -> pime-core, pime-dict`. `pime-core` never depends on `pime-dict`.

pime-core (`pime_core::complete`, re-exported at the root):

```rust
pub trait Completer: Send + Sync {
    fn complete(&self, prefix: &str, limit: usize, ctx: &CompleteCtx) -> Vec<Completion>;
}
pub struct CompleteCtx { pub use_history: bool }
pub struct Completion { pub text: String, pub score: f32 }
pub struct CompletionList { pub items: Vec<String>, pub highlighted: Option<usize> }
pub struct LearnEvent { pub word: String, pub kind: LearnKind }
pub enum LearnKind { Skipped, Ignored, Dismissed, Accepted }      // ascending priority
pub struct ContextPolicy { pub completions: bool, pub learning: bool }
pub fn match_key(s: &str) -> String;
pub fn complete_with_zwnj_rule(prefix: &str, display: &str) -> Option<String>;
pub fn completable(preedit: &str) -> bool;
pub struct ListCompleter; impl ListCompleter { pub fn new(words: impl IntoIterator<Item = (String, f32)>) -> Self; }
// Output gains: pub completions: Option<CompletionList>, pub learn: Vec<LearnEvent>
// Session gains: set_completer(Option<Arc<dyn Completer>>), set_context_policy(ContextPolicy)
```

Semantics: `Output.candidates` still means alternates. Popup rows are one index space: alternates `0..n`, completions `n..n+k` (`k <= 9 - n`); `1-9` and `select_candidate(row)` use it. `Output.learn` is non-empty only on the Output that ends a composition and must be forwarded by the host to the learner (never for `would_eat`). Build `Output` literals with `..Default::default()`.

pime-dict: `Dictionary::{from_bytes, load, len, info, prior, complete}`, `History::{record, record_accept, record_reject, learned, decay, merge, to_bytes, from_bytes, clear}`, `ScoreParams`, `Day`, `Runtime::{start, completer, record, status, request_save, on_clear_signal, shutdown, is_running}`, `build_from_tsv`. Errors never contain words or paths.

pime-candidate-ui: `CandidateView` gains `completions: Vec<String>`, `completion_highlight: Option<usize>`, `completion_prefix_chars: usize` and `selected_row() -> Option<usize>`; `Hover/Click(i)` report the combined row. `pime_tip::candidates::to_view_full(alts, comps, preedit, candidate_labels, footer, size_percent)`; `to_view` unchanged.

Files: `dict.bin` (format v1, `autocomplete.md` 3.1) next to the TIP DLLs; `%LOCALAPPDATA%\PersianPhoneticIme\history.bin` (v1, 3.4; `PIME_CONFIG_DIR` overrides). Clearing = deleting `history.bin*` and signalling `Local\PersianPhoneticIme.ConfigChanged`.

Golden additions: session cases may carry `completer` (`{"words": [[text, score], ...]}` -> `ListCompleter`), `policy`, `completions_after_last`, `completion_highlight_after_last`, `learn` (`[[kind, word], ...]`, kinds `accepted|skipped|ignored|dismissed`) in `data/golden/autocomplete.json`; dictionary cases of type `complete` and `build` live in `data/golden/dict.json`. Formats: `autocomplete.md` section 8.
