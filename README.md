# Persian Phonetic IME

A general-purpose phonetic Persian input method for Windows. Type Latin letters the way you would say the word (`salam`) and get Persian script (سلام). It is a standalone native text service (TSF), written in Rust, and has no dependency on any other project or service.

## Status

Planning, wave 1 in progress. There is nothing to install yet. The data files, the `pime-core` engine, the candidate-window crate and a disposable TSF spike are being written in parallel. See `PLAN.md` section 9 for the live status table.

## Planned behaviour

- **Type Latin, get Persian.** `salam`-style phonetic input; digraphs such as `sh`, `kh`, `ch`, `gh` are handled as you type.
- **Whole-word underlined composition.** The entire current word is shown underlined (like kana input in Google Japanese IME) and is committed with Space, Enter, punctuation, or when you click elsewhere. Backspace edits the Latin letters; Escape cancels.
- **Candidate list for ambiguous letters.** `s` can be س ص ث, `z` can be ز ذ ض ظ, `gh` can be غ ق, and so on. Press Tab or Down to open a numbered list, press `1`-`9` to pick.
- **Persian digits and punctuation** by default (`۱۲۳`, `،` `؛` `؟`), Shift+Space for ZWNJ (می‌خواهم).
- **Configurable.** A built-in preset plus an optional TOML file at `%APPDATA%\PersianPhoneticIme\config.toml`. Friendly options include `gh = "ghain" | "qaf"` and `c = "none" | "k" | "s"`, plus raw per-key map overrides.
- **Friendly config errors.** A mistake in your config never stops you typing: the bad entry (or the whole file, if it cannot be parsed) is skipped, and you get a plain-language message with the line number and a fix suggestion.
- **Private.** No network access, no telemetry, typed text is never logged.

Dictionary-backed autocomplete is an optional later phase and is not part of v1.

## Repository layout

```
PLAN.md            decisions, behaviour spec, workstreams, status
CLAUDE.md          orientation for Claude sessions
docs/              contracts.md, DESIGN.md, (config-schema.md, spike.md by other workstreams)
crates/
  pime-core/         engine, config, Session (no Windows dependencies)
  pime-candidate-ui/ candidate window
data/              presets, punctuation tables, golden test cases
spike/             disposable TSF spike (not part of the workspace)
.github/workflows/ CI (Windows)
```

More crates (`pime-tip`, `pime-settings`, `pime-setup`, `xtask`) and `installer/`, `harness/` are planned; see `PLAN.md` section 5.

## Build and test

Requires Windows, Rust stable with the MSVC toolchain, and (for 32-bit builds) `rustup target add i686-pc-windows-msvc`.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

When several people or agents build in the same checkout, give each its own target directory to avoid lock contention, for example `set CARGO_TARGET_DIR=target-mydir` (any `target*/` directory is git-ignored).

## Safety warning

An input method runs inside every application. A crashing or misbehaving development build can break typing system-wide. **Never register a development build on a machine whose only keyboard is that machine's default.** Test registration only in a VM or Windows Sandbox, and keep another keyboard layout available (Win+Space). A kill switch is planned: setting the registry value `Disabled` under `HKCU\Software\PersianPhoneticIme` (also checked under HKLM) turns the IME off.

## Licence

MIT, as declared in the workspace `Cargo.toml`. No `LICENSE` file has been added yet (TODO: add the standard MIT text with the author's name).
