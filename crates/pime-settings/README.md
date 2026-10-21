# pime-settings

Settings app and config checker for the Persian Phonetic IME. Always a separate
process; nothing here is loaded into host applications.

```
pime-settings                 settings window (same as --ui)
pime-settings --show-errors   window on the settings-problems page only
pime-settings --check [FILE]  check a file (default: your config.toml), print friendly messages
pime-settings --write-errors  check config.toml and rewrite config-errors.txt
pime-settings --reset         back up config.toml (config.toml.bak-YYYYMMDD-HHMMSS) and write the defaults
pime-settings --print-default commented default config.toml
pime-settings --open          open config.toml in the .toml editor (Notepad if none), creating it if missing
```

Tests: `cargo test -p pime-settings` (with your own `CARGO_TARGET_DIR`). They use temp
folders through `PIME_CONFIG_DIR` and never touch `%APPDATA%`.

## Contract for the TIP (text service)

**Files** (`src/paths.rs`). With `PIME_CONFIG_DIR` set, all of them are directly in that folder.

| What | Where |
|---|---|
| user config | `%APPDATA%\PersianPhoneticIme\config.toml` |
| error report | `%LOCALAPPDATA%\PersianPhoneticIme\config-errors.txt` |
| history (`history.bin`, cleared by `--clear-history`) | `%LOCALAPPDATA%\PersianPhoneticIme\` |

**Decoding**: both sides use `pime_core::decode_config_bytes` / `load_config_bytes`.
They accept UTF-8 with or without a BOM and UTF-16 LE/BE with a BOM. Anything else is
a whole-file fallback with one issue.

**Error file** (`report::errors_file_text` / `write_errors_file`): written (UTF-8 with
BOM, CRLF) whenever the load has any issue, warnings included; deleted when there are
none. Header line, `File: <path>`, blank line, then `pime_core::format_issues`.

**Reload signal** (`src/reload.rs`):

- Named manual-reset event `Local\PersianPhoneticIme.ConfigChanged`, created by whoever opens it
  first with an explicit descriptor (Everyone and AppContainer packages get SYNCHRONIZE and
  EVENT_MODIFY_STATE, low-integrity label), so an elevated creator does not lock out the
  settings app. If that fails it falls back to default security.
- The settings app, after every save or reset: `SetEvent`, hold 250 ms (`PULSE_MS`),
  `ResetEvent`. It does not use `PulseEvent`.
- The TIP waits with a thread-pool wait, never on the key path. On wake it re-reads the
  file off the UI thread, swaps its `Arc<ResolvedConfig>` and rewrites the error file.
  It treats the wake as a hint (it compares the file's last-write time and size), and
  re-arms no sooner than 1000 ms (`MIN_REARM_MS`) later.
- AppContainer (immersive) hosts never create the event; they only try to open the session's
  `Session\<id>\` copy. Hand edits do not signal. So the TIP also re-checks the file on
  focus change.

**Exit codes** (`report::exit`): 0 no problems (warnings allowed), 1 some entries
skipped, 2 whole file skipped, 3 I/O error (for example an explicit `--check` path that
does not exist), 4 bad command line.

**Launching**: the language-bar badge should run `pime-settings.exe --show-errors`, and
the "Settings" menu item should run `pime-settings.exe`.

## Not verified

The window was run on Windows 10 19045 at 96 DPI. It was captured with `PrintWindow`,
text was set through `WM_SETTEXT`, and one combo change was driven by messages.
Not tested by hand:

- keyboard navigation
- the Reset confirmation dialog
- `--open` / "Open config file" (`ShellExecute`)
- DPI above 100% (the window is system-DPI-aware and is bitmap-scaled on other monitors)
- high contrast
- Narrator
- the console release when started from Explorer
