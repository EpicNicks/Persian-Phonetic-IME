# Persian Phonetic IME for Windows: working plan

**Scope and home (decided by the user):** a general-purpose phonetic Persian IME for Windows, NOT specific to any project. It lives in this folder (`E:\Code\Projects\persian phonetic ime`), is a standalone repo (run `git init` here), and has no build-time or runtime dependency on Qamooscheh. The Qamooscheh web/Android/iOS keyboards are the *origin and reference* of the default mapping only. Consequences:
- Neutral branding: working name "Persian Phonetic IME" (final name TBD, D8), code prefix `PersianPhoneticIme`, registry `Software\PersianPhoneticIme`.
- This repo owns its map, presets and golden tests. The default preset is imported once from the Qamooscheh web map; no ongoing sync, no edits to those repos, no feature-ledger work.

Audience: Claude sessions (and humans) implementing this in parallel. Read sections 0-5, then pick a workstream from section 8. Update the status table in section 9 when you start or finish something.

## 0. Goal and ground rules

A native Windows text input processor (TSF) for typing Persian from a Latin keyboard (`salam` -> سلام), with:
1. **Google-Japanese-IME-style behaviour**: whole-word underlined composition, a numbered candidate list for ambiguous letters with number/Tab/arrow selection (section 2). Dictionary-backed autocomplete (predictions, learning, user dictionary) is an **optional later phase** (2.3), not part of v1.
2. **A configuration layer**: a built-in default preset based on the Qamooscheh keyboard, a user TOML config for options and raw per-key map overrides (section 3).

Ground rules:
- Target Windows 10 22H2 (build 19045) and Windows 11. Language fa-IR (0x0429).
- Stack (leaning, D1): **Rust** with `windows-rs`, `x86_64-pc-windows-msvc` + `i686-pc-windows-msvc`, static CRT. Plan B: Rust core + thin C++ TIP shell forked from Microsoft SampleIME (section 5.1).
- Privacy: no network, ever. Typing history is local, opt-out, clearable, never recorded in password/disabled contexts, never logged.
- **An IME crash breaks typing in every app.** Never register a development build on the host's only keyboard. Use Windows Sandbox or a Hyper-V VM, keep another keyboard (Win+Space), honour the kill switch `HKCU\Software\PersianPhoneticIme\Disabled` (also check HKLM).
- Every COM entry point must be panic-safe (`catch_unwind`, returns `E_FAIL`); no work in `DllMain`; no file I/O or allocation-heavy work on the per-key path beyond what the design allows; ETW TraceLogging with no typed content.
- Never emit Arabic-only code points ي U+064A, ك U+0643, ة U+0629 by default (map uses ی U+06CC, ک U+06A9); config validation warns if a user map contains them (opt-in `allow_arabic_forms`).

## 1. Language decision: Rust vs C++ (D1)

Answer to "is Rust painful on Windows?":
- **Not painful for most of this project.** The bulk of the code is engine, config parsing (serde + TOML), dictionary (FST) and ranking: pure logic where Rust is clearly nicer than C++ and trivially unit-testable/fuzzable.
- **Moderately more painful for the TSF shell.** `windows-rs` (Microsoft-maintained) exposes `Win32::UI::TextServices` and `#[implement]` creates COM objects implementing several interfaces. Cost: no ready SampleIME-style reference (we must port its patterns by hand), heavy `unsafe` at COM edges, a panic crossing an `extern "system"` boundary is fatal to the host (wrap every entry in `catch_unwind`; prefer `panic = "unwind"`, never rely on abort), and the i686 target must be installed (`rustup target add i686-pc-windows-msvc`; only x86_64 is installed on this machine now).
- I believe a few open-source Rust TSF IMEs already exist; verify and read one during the spike rather than trusting this.
- **Decision:** Rust, gated by the spike (WS1). If after ~5 working days the TSF shell is still fighting us, switch the shell to C++ (SampleIME fork) and keep `pime-core`/`pime-dict` in Rust behind a small C ABI. The core crate stays free of Windows types so plan B is cheap.
- C#/.NET is out (loads a CLR into every process).
- Toolchain here: VS 2022 + "18" Community (MSVC linker), Windows SDK 10.0.19041/22621/26100, CMake, Rust 1.92, dotnet. WiX v5 needed for the installer.

## 2. Product behaviour (modelled on Google Japanese IME)

### 2.1 Composition model: whole-word preedit

Replaces the earlier single-letter model. The composition is the **entire current word**, shown underlined, exactly like kana/romaji preedit in Google IME.

- The session keeps a raw `latin` buffer (e.g. `s`,`h`,`a`,`b`). The preedit text is `render(latin, config, overrides)`: tokenise (longest match over digraphs/trigraphs/singles per the resolved map), take each token's chosen candidate (default = index 0). The whole preedit is recomputed on every key, so digraph retroactive fix-ups (`s`+`h` -> ش) and Backspace are automatic: Backspace removes the last **Latin** char and re-renders.
- Lone `c` (when the config gives it no sound) renders as a visible Latin `c` in the preedit with a "pending" attribute, never a silently swallowed key.
- ZWNJ (U+200C) is a token inside the buffer (Shift+Space), so words like می‌خواهم compose in one go.
- The preedit is committed on Space, Enter, punctuation, caret/mouse/focus change, or by picking a candidate; Escape cancels (section 2.2).
- Whole-word composition is the standard IME model, so it should be *more* compatible with weak hosts (Chromium, CUAS/IMM32 apps) than the web's insert-then-fix-up model.

### 2.2 Key behaviour (all configurable where noted, section 3)

| Key | Behaviour |
|---|---|
| `a-z`, `'` (virtual keys, not characters) | Append to `latin`, re-render preedit, refresh candidates. Uppercase folds to lowercase unless `shift_letters = "alternate"` (Shift+letter picks that token's second candidate: Shift+A -> آ, Shift+S -> ص). Eaten. |
| Backspace | Composition open: delete last Latin char, re-render; when empty, end composition. Idle: pass through (app deletes; never delete committed text via TSF). |
| Space | Composition open: commit per `space` option (default `preedit` = commit exactly what is shown; alt `top-candidate`), then insert a space in the same edit session. Idle: insert space. |
| Enter | Commit per `enter` option. Default `commit` (Google IME: commit only, key eaten). Alt `commit-and-pass` (commit then pass Enter through, needs `TF_ES_SYNC`; useful for chat apps). If a candidate is highlighted, Enter commits it. |
| Tab / Down | Open the candidate list / move highlight down (Google IME). Up moves up. |
| `1-9` while list open | Select that candidate and commit (config `number_select`, default on). Digits when no list is open produce Persian/ASCII digits per config. |
| Escape | List open: close list (keep preedit). Preedit open and list closed: cancel composition (clear preedit, discard buffer). Differs from the Qamooscheh web (Esc commits); Google-IME-style per user direction. |
| Left/Right/Home/End/PgUp/PgDn, Ctrl/Alt/Win chords, mouse click, focus change | Commit, then pass the key through. (Editing inside the preedit is a later feature, D19.) |
| Shift+Space | Insert ZWNJ token into the buffer (composing) or insert a U+200C directly (idle). Tapping again right after removes it (web parity: read back one char before the caret; if the read fails, always insert). |
| Punctuation / digits | Commit preedit, then insert converted punctuation (`,`->`،` `;`->`؛` `?`->`؟` `%`->`٪`, optional `«»`) per config; digits `0-9` -> `۰-۹` by default. |

### 2.3 Candidates (v1) and autocomplete (optional later phase)

**v1 (no dictionary).** The numbered, RTL candidate list (Google-IME-style popup below the preedit) shows the alternates of the **last ambiguous token** the learner typed: `s` -> س ص ث, `z` -> ز ذ ض ظ, `a` -> ا آ, `gh` -> غ ق, and so on, in the config's order (index 0 = what the preedit already shows). Tab/Down enters the list, `1-9` picks, typing on commits the highlighted choice and continues. There is no prediction and no word completion in v1, and the IME needs no dictionary, no learning and no user history.

**Phase 2: autocomplete (only if we decide to ship it; D5).** Everything below is behind `[autocomplete] enabled = false` and the `pime-dict` crate; v1 must not depend on it.
- *Word-level alternates*: dictionary words matching the typed Latin phonetically, ranked by frequency, so `sabz` offers سبز before صبز.
- *Completions*: longer words/phrases extending the typed prefix, shown after `predict_min_chars` letters.
- Matching: each token maps to its candidate set from the resolved map and the FST dictionary is searched with those alternatives. **Short vowels must be optional in matching** (Persian script mostly omits a/e/o), otherwise `salam` never finds سلام. Skipping/insertion rules are tuning-heavy: build an evaluation set (WS3) before trusting them (D18).
- Local learning: committed (latin, word) counts in `%LOCALAPPDATA%\PersianPhoneticIme\history.bin`, toggle `learning`, clear-history command, never recorded in disabled/password contexts. User dictionary in `%APPDATA%\PersianPhoneticIme\user_dict.txt`.
- System dictionary: shipped `dict.bin` (FST + frequencies) built by a tool from a word-frequency list; **source and licence unresolved (D15)**. A missing dictionary must degrade to v1 behaviour.

### 2.4 Persian specifics

- RTL belongs to the host: emit logical-order code points only, no RLM/LRM. Candidate window lays out RTL.
- Latin pass-through toggle: `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE` (open = Persian), input-indicator button via `ITfLangBarItemButton` + `GUID_LBI_INPUTMODE` ("ف"/"A"). Optional hotkey via `ITfKeystrokeMgr::PreserveKey`, configurable, **unbound by default** (Ctrl+Space collides with IDEs). Win+Space / Alt+Shift keep working.
- Respect `GUID_COMPARTMENT_KEYBOARD_DISABLED` and `GUID_COMPARTMENT_EMPTYCONTEXT` (password/read-only fields).
- Pass-through pitfall: under fa-IR the underlying layout is Persian (00000429), so un-eaten printable keys would produce Persian-layout characters. Mitigation: while active, eat every printable key without Ctrl/Alt and insert it ourselves from a fixed US-QWERTY table; also test `hklSubstitute = 0x04090409` in `RegisterProfile`. Verify in the spike.
- Out of scope v1: ezafe and short-vowel marks (kasra U+0650, hamza above U+0654).

## 3. Configuration layer

Goal: everything about the mapping and behaviour is data, with a default preset (Qamooscheh-derived) and a user config that overrides it.

### 3.1 Resolution order

`built-in preset (e.g. "qamooscheh")` -> `[options]` (friendly switches, each expands to map patches) -> `[map.*]` raw overrides -> validation -> `ResolvedConfig` (immutable, shared with the session). On any parse/validation problem: never fail activation and never break typing; fall back as described in 3.4 and show the user a friendly message.

### 3.2 Files

- Built-in presets: `data/presets/*.toml`, compiled into the DLL (`include_str!`), so the DLL never needs them on disk. Initial: `qamooscheh` (default), `minimal` (letters only, no ambiguity candidates).
- User config: `%APPDATA%\PersianPhoneticIme\config.toml`. Written/edited by the settings app (which must preserve comments; use `toml_edit`) or by hand.
- Reload: on focus change and via a named event the settings app signals; never poll on the key path. A parsed config is held behind an `Arc` swapped atomically.

### 3.3 Schema sketch

```toml
preset = "qamooscheh"            # built-in base

[options]
gh = "ghain"                     # "ghain" (غ default, ق alternate) | "qaf" (ق default)
q  = "qaf"                       # "qaf" | "ghain"
c  = "none"                      # "none" (visible placeholder) | "k" (ک) | "s" (س ص ث)
kh = "khe"                       # خ (the value name is "khe", the letter خ)
ch = "che"                       # چ (plain; no Hebrew-style variant)
s_order = ["س","ص","ث"]          # default first
z_order = ["ز","ذ","ض","ظ"]
digits = "persian"               # "persian" | "ascii"
punctuation = "persian"          # "persian" | "none"
shift_letters = "lowercase"      # "lowercase" | "alternate"
space = "preedit"                # "preedit" | "top-candidate"
enter = "commit"                 # "commit" | "commit-and-pass"
number_select = true
candidate_labels = false         # learner hints under glyphs ("sin", "sad", "se")

# Phase 2 only; ignored (and hidden in the settings app) until autocomplete ships
[autocomplete]
enabled = false
predict_min_chars = 2
learning = true

# Raw overrides, applied last. Key = Latin sequence (1-3 ASCII chars), value = ordered candidates, index 0 = default.
[map.singles]
v = ["و"]
[map.digraphs]
sh = ["ش"]
[map.trigraphs]
# 3-letter keys are allowed, e.g. tch = ["چ"]
```

Semantics worth stating:
- `gh = "qaf"` reorders `gh` to `["ق","غ"]`; the other letter stays an alternate. Same for `q`.
- `c = "k"` adds `c -> ["ک"]`; `ch` then still works because the whole preedit is re-tokenised (longest match), so `c`+`h` still renders چ. `c = "s"` adds `c -> ["س","ص","ث"]`. `c = "none"` is the Qamooscheh behaviour (visible pending `c`).
- Digraph starters are derived from map keys, never stored. Tokeniser is longest-match over keys of length 1-3.
- Validation: keys 1-3 printable ASCII; candidate lists non-empty, no duplicates; each candidate is a single NFC code point from the Persian/Arabic block or ZWNJ (strings of more than one code point allowed only behind `allow_sequences`); warn on Arabic-only forms unless `allow_arabic_forms`; digraph prefixes must not make a key unreachable (e.g. defining `sh` while `s` is undefined is fine, defining `shh` without `sh` warns).
- A settings UI (later) is just a TOML editor with presets and a live "try typing" box.

### 3.4 Friendly config errors

A broken config must tell the user what is wrong, where, and how to fix it, in plain language, and must never leave them unable to type.

Behaviour:
- **Syntax error** (TOML does not parse): the whole user file is ignored and the built-in preset is used.
- **Bad value or unknown key**: only that entry is ignored (that option keeps the preset value) and everything else in the file still applies. Unknown keys get a "did you mean" suggestion (edit distance against known keys). Invalid values list the allowed ones.
- **Map problems** (bad candidate, empty list, unreachable key): that map entry is skipped.
- The result is always a `ResolvedConfig` plus a `Vec<ConfigIssue>`; issues are severity `Error` (something was ignored) or `Warning` (applied, but suspicious, e.g. Arabic ي).

`ConfigIssue` carries `severity`, `line`, `column`, the offending snippet, a `message` and a `hint`. Messages are written for non-programmers and never mention serde/TOML internals. Examples:

```
Your Persian Phonetic IME settings have a problem (config.toml, line 7):
  gh = "kaf"
"kaf" isn't one of the choices for gh. Use "ghain" or "qaf".
That setting was skipped; everything else in your file is still in use.
```
```
config.toml, line 12: this line couldn't be read (a closing quote is missing).
Your whole settings file was skipped and the default settings are in use.
Fix the line and save the file; the change is picked up automatically.
```
```
config.toml, line 3: unknown setting "diigts". Did you mean "digits"?
```

Where it is shown (host apps can't safely pop dialogs):
1. **Input indicator / language bar**: the "ف" button gets a warning badge and tooltip "Your settings file has a problem. Click for details." Clicking opens the settings app on the error page.
2. **One-time notification** the first time a given error text is seen after a config change (a Windows toast launched from the settings helper process, not from the host app).
3. **Settings app** error page: the messages above, the file path, an "Open config file" button, "Reset to defaults" (backs the file up first) and "Check again".
4. **`%LOCALAPPDATA%\PersianPhoneticIme\config-errors.txt`**: the same text, rewritten on every load, so it can be opened by hand.
5. **CLI/CI**: `pime-settings --check [path]` and `cargo xtask check-config` print the same messages and exit non-zero on errors.

Error text never includes typed content. Reloading is automatic (3.2); fixing the file clears the badge.

## 4. Data and tests

- `data/presets/qamooscheh.toml`: imported once from Qamooscheh web `phoneticMap.ts` (DIGRAPHS, SINGLES, current `e` alias of `a`); `data/SOURCE.txt` records the web commit it was imported from.
- `data/punctuation.toml`: IME-only digit/punctuation tables.
- `data/golden/*.json`: golden cases for the engine and session. Engine cases (per-keystroke preedit after typing a Latin string under a given config), session cases (key sequences -> committed text, including Backspace, Esc, candidate pick, Shift+Space), config-error cases (broken file -> expected messages and fallback), and (phase 2 only) dictionary cases. Seed phonetic cases from the Android `PersianPhoneticEngineTest.kt`. Each case names the config/preset it runs under so every option gets coverage (gh=qaf, c=k, c=s, bad-config fallbacks, ...).
- Map/config lint tests: Persian-only output, no Arabic-only code points in presets, `e` equals `a` in `qamooscheh`, every preset resolves and round-trips.
- Dictionary evaluation set (WS3): a few hundred common Persian words/phrases with accepted romanisations, measuring top-1/top-5 hit rate; the gate for changing vowel-skipping rules.

## 5. Architecture

### 5.1 Rust workspace

```
Cargo.toml                    workspace
crates/
  pime-core/    NO windows deps: tokeniser, renderer, config (serde+toml), presets, Session (key -> Output), US layout, punctuation
  pime-dict/    PHASE 2 / optional: FST dictionary, build tool, phonetic matching with optional vowels, user dict, history/ranking
  pime-tip/     cdylib, windows-rs: COM, TSF sinks, edit sessions, composition, candidate window, UIA, langbar, registration
  pime-settings/ settings exe (separate process, never in-host)
  pime-setup/   register/unregister helper used by the installer
  xtask/        build dict, lint presets, package
data/ presets, golden, punctuation
installer/ WiX v5
harness/ TSF test host
docs/
```

Dependency direction: `pime-tip` -> `pime-core`, `pime-dict`. Session API (declare this first so streams can code against it):

```rust
pub struct KeyEvent { pub vk: u32, pub shift: bool, pub ctrl: bool, pub alt: bool }
pub struct Output {
    pub commit: String,                 // text to commit now (may be empty)
    pub preedit: String,                // current underlined composition ("" = none)
    pub preedit_pending: Option<Range>, // sub-range to draw with the "pending" attribute (lone c)
    pub candidates: Option<CandidateList>, // items, highlighted index, kind (alternate|completion)
    pub eaten: bool,
}
impl Session {
    pub fn new(cfg: Arc<ResolvedConfig>) -> Self;   // phase 2 adds with_dictionary(cfg, Arc<Dictionary>)
    pub fn would_eat(&self, k: &KeyEvent) -> bool;        // OnTestKeyDown
    pub fn on_key(&mut self, k: &KeyEvent) -> Output;
    pub fn select_candidate(&mut self, index: usize) -> Output;   // mouse click
    pub fn on_focus_lost(&mut self) -> Output;             // commit
    pub fn on_composition_terminated(&mut self);           // host ended it: reset, no edit
    pub fn set_config(&mut self, cfg: Arc<ResolvedConfig>);
}
```

### 5.2 TSF mechanics

- `ITfKeyEventSink`: `OnTestKeyDown` -> `would_eat`; `OnKeyDown` runs the session synchronously then `RequestEditSession(TF_ES_READWRITE | TF_ES_ASYNCDONTCARE)`; keys that must commit before passing through use `TF_ES_SYNC`. Session state is the source of truth; never read document text back (except the Shift+Space ZWNJ toggle).
- Composition: `ITfInsertAtSelection::InsertTextAtSelection(TF_IAS_QUERYONLY)` + `ITfContextComposition::StartComposition`; each key does `ITfRange::SetText` of the full preedit and sets `GUID_PROP_ATTRIBUTE` via our `ITfDisplayAttributeProvider` (input = underline, pending = dotted grey, selected = thick). Commit via `EndComposition` and collapse the selection with `SetSelection`. Even plain commits go start -> set -> end (best CUAS compatibility).
- Interfaces on the text service: `ITfTextInputProcessorEx` (`ActivateEx`), `ITfKeyEventSink`, `ITfThreadMgrEventSink`, `ITfTextEditSink` (selection moved outside our session -> commit), `ITfTextLayoutSink`, `ITfCompositionSink` (`OnCompositionTerminated`), `ITfDisplayAttributeProvider`, `ITfCompartmentEventSink`, `ITfThreadFocusSink`.
- Rejected: synthesising Backspace with `SendInput`.

### 5.3 Candidate UI

- One model, two presentations: `ITfCandidateListUIElement` (+ Behavior) via `ITfUIElementMgr` Begin/Update/EndUIElement (draw nothing when `pbShow == FALSE` or `TF_TMF_UIELEMENTENABLEDONLY`), and our own window.
- Window: `WS_POPUP`, `WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`, `MA_NOACTIVATE`, one reused window per thread.
- Layout: vertical numbered list (1-9), glyph large and right-aligned, optional gloss/label, a marker for completion vs alternate rows, footer hint for paging. DirectWrite, Segoe UI with Tahoma fallback, RTL reading direction.
- Placement: `ITfContextView::GetTextExt` on the composition range inside an edit session; below the preedit, flip above if no room, clamp to `MonitorFromRect` work area; reposition on `OnLayoutChange`; fall back to `GetGUIThreadInfo` caret if `TS_E_NOLAYOUT` persists. Per-monitor V2 DPI on our thread; `LogicalToPhysicalPointForPerMonitorDPI` for DPI-unaware hosts.
- Theme: `AppsUseLightTheme` + `WM_SETTINGCHANGE`; high contrast via system colours.
- Accessibility: UIA provider (List > ListItem, SelectionItemPattern), `UiaRaiseNotificationEvent` "سلام, 2 of 5". Never takes focus.

### 5.4 Registration and install

- GUIDs frozen once in `tip/guids.rs`: `CLSID_PersianPhoneticTextService`, `GUID_Profile_FaPhonetic`, display-attribute GUIDs, langbar button, preserved key.
- `DllRegisterServer`: `HKLM\Software\Classes\CLSID\{clsid}\InprocServer32` (`ThreadingModel=Apartment`); `ITfInputProcessorProfileMgr::RegisterProfile(clsid, 0x0429, profileGuid, "Persian Phonetic", ...)`; `ITfCategoryMgr::RegisterCategory` for `TIP_KEYBOARD`, `DISPLAYATTRIBUTEPROVIDER`, `TIPCAP_UIELEMENTENABLED`, `TIPCAP_IMMERSIVESUPPORT`, `TIPCAP_SYSTRAYSUPPORT`, `TIPCAP_INPUTMODECOMPARTMENT`, `TIPCAP_COMLESS`. No `TIPCAP_SECUREMODE` in v1. `DllUnregisterServer` reverses it.
- Builds: x64 + Win32 DLLs under one CLSID, static CRT, `panic = "unwind"`, `/guard:cf`. Skip ARM64 in v1.
- Installer: WiX v5 MSI, machine-wide, Program Files (MSIX cannot register an in-proc TIP). Per-user custom action `InstallLayoutOrTip(L"0429:{CLSID}{ProfileGUID}", 0)` from input.dll. Versioned DLL names (`PersianPhoneticIme.<ver>.x64.dll`), repoint `InprocServer32` on upgrade, delete old files on reboot. Uninstall: `InstallLayoutOrTip(..., ILOT_UNINSTALL)` then unregister. Ships `dict.bin`.
- Signing: Authenticode on DLLs, EXEs and MSI (D8); unsigned only for development.

## 6. Testing

1. Golden tests (section 4) run in `cargo test` on `pime-core`/`pime-dict`, no Windows needed.
2. Property/fuzz: `cargo-fuzz` on `Session` with random key/modifier/focus sequences and random (valid and invalid) configs. Invariants: no panic, preedit is valid text, committed text only from the allowed set, Backspace to empty always ends composition.
3. Config tests: every `[options]` value, bad files fall back to the preset, hot-swap mid-composition is safe.
4. TSF harness (`harness/`): Win32 host with a TSF-enabled RichEdit; activate our profile with `ITfInputProcessorProfileMgr::ActivateProfile`, drive with `SendInput`, assert via UIA TextPattern. Local and in a VM.
5. Manual matrix (`docs/compat-matrix.md`): Notepad (CUAS on Win10), WordPad, Word, Outlook, Chrome/Edge (input, contenteditable, Google Docs), Firefox, VS Code, Discord/Teams, Windows Terminal and conhost, Settings/Start search, Mail (immersive), Explorer rename, elevated Notepad, password fields (must be disabled), RDP. Per host: Enter behaviour, candidate click, Latin toggle, candidate popup placement, broken-config badge. Both Windows 10 19045 and Windows 11, 100/150/200% DPI, light/dark/high-contrast, with Narrator.

## 7. Decisions

Defaults are ASSUMED so work can proceed; flag work that depends on a changed answer.

| # | Decision | Assumed default | Status |
|---|---|---|---|
| D1 | Language | Rust, gated by spike (plan B: Rust core + C++ shell) | LEANING Rust (user) |
| D2 | Canonical map/config in this repo, default preset imported once from Qamooscheh | yes | DECIDED |
| D3 | Lone `c` | `c = "none"` in the default preset (visible pending `c`); `"k"` / `"s"` available | CONFIG |
| D4 | Enter | `commit` (Google IME style); `commit-and-pass` option | CONFIG |
| D5 | Autocomplete/predictions | Approved (user, 2026-10-21) as a toggleable option (`[autocomplete] enabled`); v1 candidate list (alternates of the last ambiguous letter) is unchanged when it is off. See D20-D24 | DECIDED |
| D6 | Digits/punctuation | Persian digits on, set per 2.2, `1-9` selects candidates | OPEN |
| D7 | On/off hotkey | unbound by default, configurable | OPEN |
| D8 | Product name, signing, distribution | TBD (GitHub Releases) | OPEN |
| D9 | Ledger | dropped (standalone) | DROPPED |
| D10 | Scope | Persian only, ARM64 out of v1 | OPEN |
| D11 | Shift+letter | `lowercase`; `alternate` available | CONFIG |
| D12 | Extra scripts (e.g. romaji/kana, other languages) | out of scope | DECIDED |
| D13 | Learner extras (labels, ezafe/vowel marks) | labels optional; marks out of v1 | OPEN |
| D14 | Space | commit what is shown (`preedit`); `top-candidate` available | CONFIG |
| D15 | Dictionary source and licence | `wordfreq` Persian data (user's choice); licence and attribution to be verified by the data stream before shipping | DECIDED (licence check pending) |
| D16 | Esc | closes list, then cancels composition (Google style) | DECIDED (user: fine for now) |
| D17 | Hebrew-style `ch` | dropped: `ch` is plain چ, `kh` is خ | DROPPED |
| D18 | Vowel-optional matching rules | DROPPED: the IME is purely phonetic and users spell the letters that appear in the Persian word (user); completion matches the Persian preedit prefix, no vowel skipping | DROPPED |
| D19 | Cursor movement inside the preedit | out of v1 (Left/Right commit and pass through) | OPEN |
| D20 | What autocomplete matches on | The current working output (the Persian preedit of the word being typed), as a prefix of dictionary words. No Latin-side matching; the phonetic map is considered solved (user) | DECIDED |
| D21 | Adaptive ranking ("fuzzy" scoring) | Soft score = wordfreq prior + learned boost from accepted completions - penalty from rejections (shown but ignored or dismissed). Fuzzy here means graded scoring, not edit-distance matching | DECIDED (interpretation to confirm) |
| D22 | ZWNJ in completions | Offer the form with ZWNJ unless the user has already typed past the ZWNJ position without one; then complete without it | DECIDED |
| D23 | Arabic letter variants | Never emitted; source data is normalised to ی ک ه at build time and Arabic-only forms are dropped | DECIDED |
| D24 | Learning storage | Local only, `learning` toggle, clear-history command, never recorded in disabled/password contexts, never logged, no network. This is the one sanctioned exception to "never persist typed content" in CLAUDE.md, limited to accepted/rejected dictionary words and their counts | DECIDED (user) |
| Loc | Repo location | this folder | DECIDED |

## 8. Workstreams (parallelisable)

Exclusive file ownership per workstream; use separate git worktrees/branches. Contracts between streams: the Session API (5.1), the config schema (3.3) and the golden format (section 4).

| ID | Workstream | Owns | Depends on | Done when |
|---|---|---|---|---|
| WS0 | Seed data and schema | `data/presets/qamooscheh.toml`, `minimal.toml`, `data/punctuation.toml`, `data/SOURCE.txt`, `data/golden/` seeds, schema doc | none (read-only on Qamooscheh repos) | preset imported from web `phoneticMap.ts`; golden seeded from the Kotlin test; presets lint |
| WS1 | Rust TSF spike (disposable) | scratch crate | none | Under fa-IR in Notepad, Word, Edge, Settings search: typing `sh` shows a composing ش underlined, Space commits. Answers: windows-rs ergonomics for the interface set in 5.2, `catch_unwind` at every entry, i686 build, pass-through / `hklSubstitute`, `TF_ES_SYNC` for Enter pass-through, Shift+Space read-back, `InstallLayoutOrTip`, clean unregister, whether immersive apps need a signed TIP (S4). Decision gate: Rust shell vs plan B. Findings in `docs/spike.md` |
| WS2 | `pime-core` | `crates/pime-core`, config resolver/validation and friendly `ConfigIssue` messages (3.4), tokeniser/renderer, `Session`, golden runner, fuzz | WS0; Session API header lands first | all golden + config tests pass; fuzz clean |
| WS3 | `pime-dict` (PHASE 2, optional; do not start until autocomplete is approved) | `crates/pime-dict`, dict build tool in `xtask`, matching with optional vowels, user dict, history, evaluation set | WS0; D15 for real data (use a tiny fixture first) | fixture dictionary tests pass; evaluation harness reports top-1/top-5 |
| WS4 | TIP skeleton | `crates/pime-tip` entry points, class factory, registration, text service, key sink, compartments, kill switch | WS1 findings; Session API | clean register/activate/unregister; key sink wired to a stub session |
| WS5 | Composition layer | edit sessions, composition, display attributes, focus/termination/caret finalize, full key table 2.2, ZWNJ, digits, punctuation, US-layout pass-through | WS2, WS4 | key table works in the matrix hosts |
| WS6 | Candidate UI | candidate window, UI element, UIA, placement/DPI/theme | candidate model from WS2; hooks from WS4 | numbered list, mouse select, UIless mode, Narrator |
| WS6a | Standalone candidate window (prototype) | `crates/pime-candidate-ui`: popup window, placement/DPI/theme, demo exe, no TSF | none (own `CandidateView` input) | window + demo build; placement math unit-tested; TIP adapter comes in WS6 |
| WS7 | Settings + langbar + config watching | `crates/pime-settings` (incl. config error page, `--check`), langbar button with error badge/tooltip, toast, `config-errors.txt`, reload signalling | WS2 config schema, WS4 | edits to config take effect without restart |
| WS8 | Installer | `installer/`, `crates/pime-setup` | WS4 registration (WS3 `dict.bin` only if phase 2 ships) | x64+x86 MSI, versioned DLLs, per-user enable, upgrade/uninstall verified in Sandbox |
| WS9 | Harness + compat | `harness/`, `docs/compat-matrix.md`, CI | WS4+ | automated SendInput/UIA tests in a VM; manual matrix recorded |
| WS10 | Docs + hygiene | `README.md`, repo `CLAUDE.md`, `docs/DESIGN.md`, `.gitignore`, CI | none | README explains build, install, config |

Suggested waves:
- Wave 1: WS0, WS1, WS2 (independent). WS3 is phase 2 and not scheduled.
- Wave 2: WS4, then WS5/WS6/WS7 in parallel once the skeleton and `Session`/`CandidateList` types exist.
- Wave 3: WS8, WS9, WS10.

Interface-first tip: WS2 lands `pime-core` public types (`KeyEvent`, `Output`, `CandidateList`, `ResolvedConfig`) as compiling stubs first.

## 9. Status

| Workstream | Status | Owner/session | Notes |
|---|---|---|---|
| WS0 | done | wave-1 agent | 228 golden cases, presets, schema doc; `python data/validate.py` passes. Assumptions listed in case `notes` |
| WS1 | in progress (code done; awaiting VM run) | WS1 spike session | Spike builds x64+x86, static CRT, tests pass; nothing registered on host. Recommendation: continue Rust shell pending VM checklist in docs/spike.md section 6 |
| WS2 | done | WS2 agent | config + friendly issues, renderer, Session; 228 golden cases pass; seeded random robustness tests (no cargo-fuzz) |
| WS3 | in progress | autocomplete build (2026-10-21) | pime-dict, xtask, data/dict, core/UI/TIP/settings/installer wiring landed per docs/autocomplete.md; autocomplete defaults off; VM verification and eval gate (Q1) pending |
| WS4 | in progress (code done; awaiting sandbox run) | stream A | crates/pime-tip: exports, registration (pime-ids GUIDs), kill switch, sinks; COM tests in-process only; x64+x86 release, dumpbin OK (exports, no CRT imports, CFG); sandbox package crates/pime-tip/dist |
| WS5 | in progress (code done; awaiting sandbox run) | stream A | Session -> pure EditPlan (golden replay through a simulated document), edit sessions, display attrs, ZWNJ read-back, popup/langbar/config wiring; checklist in crates/pime-tip/README.md |
| WS6 | in progress (stream B part done; awaiting stream A wiring + VM run) | stream B agent | popup restyled, prefer_above, PopupPresenter + UI element in pime-tip/candidates.rs; demo drives a real Session (`--smoke` passes). Visuals, UIA, UI-less hosts unverified |
| WS6a | done (compile + unit tests only) | wave-1 agent | demo: target-ws6adebugexamplesdemo.exe; visuals, UIA, DPI unverified |
| WS7 | in progress (host-independent part done) | WS7 agent | `pime-settings`: --check/--reset/--print-default/--write-errors/--open, window (error page, options form, try typing), reload event; 50 tests (grep count of #[test] in crates/pime-settings). Toast (PLAN 3.4) not implemented; see docs/DESIGN.md "Known drift" |
| WS8 | in progress (built; awaiting VM run + real TIP DLLs) | WS8 agent | pime-setup (19 tests, x64+x86, /status verified read-only); WiX v5 placeholder MSI built + ICE-validated, never installed; checklist in installer/README.md |
| WS9 | not started | | |
| WS10 | in progress | WS10 agent | README, CLAUDE.md, DESIGN.md, contracts.md refreshed for pime-tip/settings/setup; CI at .github/workflows/ci.yml (fmt, clippy, test on windows-latest); LICENSE file still missing |

## 10. Risks

IME crash takes down the host app (catch_unwind everywhere, kill switch, VM-first, fuzzing). DLL stays loaded in every process (versioned names). Pass-through via the Persian layout. Async edit-session ordering against pass-through keys. `GetTextExt` under DPI virtualisation. AppContainer ACLs and immersive window banding. Chromium rendering RTL preedit in LTR text. Candidate popup flicker (the list opens on nearly every ambiguous letter; reuse one window). Config complexity: bad user files must never break typing and errors must be understandable (3.4). Phase 2 only: vowel-optional phonetic matching quality and dictionary licence (D15) could block autocomplete. Rust TSF shell may need plan B. Signing cost and SmartScreen reputation. Processes enforcing Microsoft-signed-only DLLs never load third-party TIPs (documented gap). Windows 10 22H2 is past mainstream support, so keep a Windows 11 VM in the matrix.

## 11. Effort

v1 (everything except WS3), one developer new to TSF: about 35-50 working days: WS0 1-2d, WS1 4-6d, WS2 6-9d (incl. config + friendly errors), WS4 4-6d, WS5 6-9d, WS6 5-8d, WS7 4-6d, WS8 4-5d, WS9/WS10 5-8d. A usable v0 (WS0-WS2, WS4-WS6: whole-word composition, config, candidate list; no settings app) is about half of that. Phase 2 autocomplete (WS3 plus candidate-UI extensions) adds roughly 12-20 days plus dictionary sourcing. The spike (WS1) carries most of the risk.
