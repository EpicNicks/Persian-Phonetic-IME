# Persian Phonetic IME

A general-purpose phonetic Persian input method for Windows. Type Latin letters and get Persian script. The mapping is letter by letter and has no dictionary, so you spell the letters that appear in the Persian word: `slam` gives سلام, `khub` gives خوب, `shir` gives شیر. Vowels are typed only when they appear in the Persian spelling; every `a` becomes ا. It is a standalone native text service (TSF), written in Rust, and has no dependency on any other project or service.

## Status

Pre-release. The engine, candidate window, settings app, TSF text service (`pime-tip`) and a placeholder MSI all build and pass their tests, but the text service and installer have **not yet been run in a VM or Windows Sandbox**, so treat them as unverified. Nothing is signed. See `PLAN.md` section 9 for the live status table.

## Planned behaviour

- **Type Latin, get Persian.** Letter-by-letter phonetic input (`a` is ا, `o` is و, `y` is ی); digraphs such as `sh`, `kh`, `ch`, `gh` are handled as you type.
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
docs/              contracts.md, DESIGN.md, config-schema.md, spike.md
crates/
  pime-core/         engine, config, Session (no Windows dependencies)
  pime-candidate-ui/ candidate window
  pime-tip/          the TSF text service DLL (COM shell around pime-core)
  pime-settings/     settings app and CLI (--check, --reset, --print-default, ...)
  pime-setup/        registration helper used by the installer
  pime-ids/          frozen GUIDs shared by the above
data/              presets, punctuation tables, golden test cases
installer/         WiX v5 MSI project and build script
spike/             disposable TSF spike (not part of the workspace)
.github/workflows/ CI (Windows)
```

Still planned (PLAN.md section 5): `xtask`, `harness/`, and the optional `pime-dict` (phase 2).

## Build and test

Requires Windows, Rust stable with the MSVC toolchain, and (for 32-bit builds) `rustup target add i686-pc-windows-msvc`.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

When several people or agents build in the same checkout, give each its own target directory to avoid lock contention, for example `set CARGO_TARGET_DIR=target-mydir` (any `target*/` directory is git-ignored).

## Install

Building is safe anywhere. Installing is not: the MSI (`installer/build.ps1`, see `installer/README.md`) and `pime-setup /register` register a text service that loads into every application. Do this only in a VM or Windows Sandbox, never on a development machine. The `pime-tip` sandbox checklist is in `crates/pime-tip/README.md`.

## Safety warning

An input method runs inside every application. A crashing or misbehaving development build can break typing system-wide. **Never register a development build on a machine whose only keyboard is that machine's default.** Test registration only in a VM or Windows Sandbox, and keep another keyboard layout available (Win+Space). A kill switch is planned: setting the registry value `Disabled` under `HKCU\Software\PersianPhoneticIme` (also checked under HKLM) turns the IME off.

## Download

Installers are published on the [Releases page](https://github.com/EpicNicks/Persian-Phonetic-IME/releases). Releases are pre-release until the text service has been verified in a VM (see Status).

Release builds are currently **unsigned**, so Windows SmartScreen will warn and the IME may be ignored in some modern (immersive) apps. Verify a download against the `.sha256` file attached to the release.

## Privacy

See [PRIVACY.md](PRIVACY.md). This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

## Licence

GNU General Public License v3.0 only (`GPL-3.0-only`); see `LICENSE`. The bundled dictionary data (`dict.bin` and `data/dict/*.tsv`) is a separate work under CC BY-SA 4.0; see `data/dict/NOTICE.txt`.
