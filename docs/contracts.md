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
| WS10 | `README.md`, `CLAUDE.md`, `.gitignore`, `.github/**`, `rustfmt.toml`, `docs/DESIGN.md` |

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

Key names in `keys`: a single printable character for letters, digits and punctuation (`"s"`, `"1"`, `","`, `"'"`); named keys `"Space"`, `"Enter"`, `"Escape"`, `"Backspace"`, `"Tab"`, `"Up"`, `"Down"`, `"Left"`, `"Right"`, `"ShiftSpace"`. Uppercase letters (`"S"`) mean Shift+letter.

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
                           pub labels: Option<Vec<String>>, pub show_footer: bool /* "2/3" */ }
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

Hover only tints the row under the pointer; the highlight (selection) is always the session's.

## Candidate presenter (pime-tip, stream B -> stream A)

`presenter.rs` (`CandidatePresenter`, `ScreenRect`, `PreferredSide`) is unchanged. `candidates.rs` adds, without touching the pinned trait:

```rust
pub fn create_presenter() -> Box<dyn CandidatePresenter>;                 // popup only
pub fn create_presenter_with(opts: PresenterOptions) -> Box<dyn CandidatePresenter>;
pub fn register_ui_element(thread_mgr: &ITfThreadMgr, candidate_labels: bool) -> Box<dyn CandidatePresenter>;
pub struct PresenterOptions { pub candidate_labels: bool, pub hide_footer: bool, pub thread_mgr: Option<ITfThreadMgr> }
pub struct PopupPresenter; // impl CandidatePresenter; new(opts), set_candidate_labels(bool)
pub fn to_physical(hwnd: HWND, r: ScreenRect) -> ScreenRect;  // GetTextExt (logical) -> physical
pub fn caret_fallback_anchor() -> Option<ScreenRect>;         // GetGUIThreadInfo caret, physical
pub fn is_degenerate(r: ScreenRect) -> bool;                  // all-zero / inverted rect
```

With `thread_mgr` set, each list is also published as an `ITfCandidateListUIElement` (+ Behavior) via `ITfUIElementMgr::Begin/Update/EndUIElement` from inside `show`/`set_highlight`/`hide`; our window is drawn only when the host allows (`pbShow`, `ITfUIElement::Show`, not `TF_TMF_UIELEMENTENABLEDONLY`). Behavior `SetSelection` moves the popup/element highlight only; `Finalize` calls the `set_on_click` callback with the element's selection; `Abort` is a no-op. A degenerate anchor reuses the last anchor, then the caret fallback, then the mouse position.
