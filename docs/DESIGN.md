# Design overview

Stable, distilled architecture. Behaviour, config schema and the decision table live in [`PLAN.md`](../PLAN.md) (sections 2, 3, 7); interfaces between workstreams in [`contracts.md`](contracts.md). This file does not repeat them.

## Components

```
+-------------------------------------------------------------+
| Host application (Notepad, browser, ...)                    |
|   +-----------------------------------------------------+   |
|   | pime-tip.dll  (in-proc TSF text service, Win32/x64) |   |
|   |  key sink -> edit sessions -> composition/attrs     |   |
|   |  candidate UI adapter, langbar, compartments        |   |
|   |        |                                            |   |
|   |        v        (plain Rust types, no Windows)      |   |
|   |  pime-core:  Session, tokeniser/renderer, config    |   |
|   +-----------------------------------------------------+   |
+-------------------------------------------------------------+
        ^ config file / named event            (separate processes)
        |
 pime-settings.exe   pime-setup.exe / MSI installer
```

- `pime-core`: no Windows dependencies. Config resolution (preset, options, raw map, validation), tokeniser and renderer, `Session` state machine, US-layout and punctuation tables. Builds and tests on any host.
- `pime-candidate-ui`: candidate window. It takes its own plain `CandidateView`, so it does not depend on `pime-core`; an adapter in the TIP crate converts `CandidateList`.
- `pime-tip` (planned): the COM/TSF shell. Thin: translates TSF events to `KeyEvent`, applies `Output` to the document.
- `pime-settings`, `pime-setup`, installer, harness (planned): separate processes, never loaded in a host.
- `pime-dict` (phase 2, optional): dictionary and autocomplete. v1 must not depend on it.

## Data flow

```
physical key
   |  ITfKeyEventSink::OnTestKeyDown --> Session::would_eat(KeyEvent)
   v
OnKeyDown --> Session::on_key(KeyEvent)   (synchronous, pure, no I/O)
   v
Output { commit, preedit, preedit_pending, candidates, eaten }
   v
RequestEditSession (async; TF_ES_SYNC when a commit must precede a pass-through key)
   v
edit session: start/update composition -> ITfRange::SetText(preedit)
              -> display attributes -> on commit EndComposition
   +--> candidate UI update (UIElement + own window), from Output.candidates
```

The `Session` holds the raw Latin buffer and re-renders the entire preedit on every key, so digraph fix-ups and Backspace need no special cases. Session state is the source of truth; document text is not read back (exception: the Shift+Space ZWNJ toggle).

## Config layering

```
built-in preset (compiled in)
  -> [options]  (friendly switches expand to map patches)
  -> [map.*]    (raw overrides)
  -> validation
  => ResolvedConfig (immutable, Arc) + Vec<ConfigIssue>
```

The user file is `%APPDATA%\PersianPhoneticIme\config.toml`. A reload builds a new `ResolvedConfig` off the key path and swaps it in atomically (`Arc`); the session picks it up with `set_config`. Triggers are focus change and a named event, never polling on the key path.

## Error handling philosophy

- Typing always works. A bad config degrades (skip the entry, or the whole file on a syntax error) and never fails activation.
- Problems are data: a `ConfigIssue` list with severity, location and a plain-language hint, surfaced outside the host (language-bar badge, settings app, `config-errors.txt`, CLI check). Host apps never get dialogs.
- Messages never contain typed content.
- Unexpected states reset the session (commit or discard the preedit) rather than propagate.

## Threading and crash-safety rules

- TIP objects use the apartment threading model: calls arrive on the host's UI thread. Keep per-key work short; no file or network I/O there.
- Every `extern "system"` / COM entry point is wrapped in `catch_unwind` and returns `E_FAIL`; panics never cross the boundary. Build with `panic = "unwind"`.
- Nothing in `DllMain` beyond trivial setup.
- Shared state (config) is immutable behind `Arc`; background work (config watching) must not touch COM objects from other threads.
- A kill switch (`Disabled` under `HKCU` / `HKLM` `Software\PersianPhoneticIme`) disables the service; the disabled and empty-context compartments (password fields) are respected.
- Logging is ETW only and contains no typed content.
- The DLL stays loaded in every process that used it, so DLL names are versioned for upgrades.

## v1 versus phase 2

| Area | v1 | Phase 2 (optional) |
|---|---|---|
| Composition | whole-word preedit, ZWNJ, digits, punctuation | cursor movement inside the preedit |
| Candidates | alternates of the last ambiguous token | dictionary alternates and completions |
| Dictionary / learning | none | `pime-dict`, user dictionary, local history |
| Config | presets, options, map overrides, friendly errors | `[autocomplete]` section |
| Targets | x64 and Win32 on Windows 10 22H2 and 11 | ARM64 (undecided) |

PLAN.md sections 2.3 and 7 are authoritative on scope.
