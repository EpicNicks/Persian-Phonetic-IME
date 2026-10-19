# pime-tip: the Persian Phonetic TSF text service

`pime_tip.dll` (x64 and x86, one CLSID, GUIDs from `crates/pime-ids`). **Never register it on
your development machine**: a broken text service breaks typing in every app. Test only in
Windows Sandbox or a VM.

## Build (host, safe)

```
powershell -ExecutionPolicy Bypass -File crates\pime-tip\scripts\build.ps1 [-SkipTests]
```

Runs `cargo test`/`clippy` for `pime-tip`, builds both release DLLs into `target-tipA`, and
stages `crates\pime-tip\dist\{x64,x86,scripts,pime.wsb}`. Nothing is registered or loaded.

## Sandbox run

1. Double-click `crates\pime-tip\dist\pime.wsb` (maps `dist` read-only to `C:\pime`, no network).
2. Elevated PowerShell in the sandbox:
   `powershell -ExecutionPolicy Bypass -File C:\pime\scripts\register.ps1`
   (`-PassPrintable` lets idle digits/punctuation through to the Persian layout; `-HklSubstitute`
   registers with hklSubstitute 0x04090409.) Switch with Win+Space to "Persian Phonetic".
3. Work through the checklist below. `scripts\config.ps1` writes test configs.
4. `C:\pime\scripts\unregister.ps1`: every check must report absent.

Emergency: Win+Space to another keyboard, or `killswitch.ps1 -On` (next activation).
Runtime switch: `HKCU\Software\PersianPhoneticIme\Tip\EatPrintable` (DWORD, default 1).

## Checklist

Notepad first, then Edge (textarea, contenteditable), WordPad. Record results per host.

| # | Do | Expect |
|---|---|---|
| 1 | type `s` | underlined س, numbered list س ص ث below it |
| 2 | then `h` | preedit ش, list closes; Space commits "ش " |
| 3 | `s`, Tab / Down / Up | highlight moves, preedit follows; no flicker/relayout |
| 4 | `s`, `2` (or click row 3) | ص (ث) committed, list gone |
| 5 | `salam`, Backspace ×2 | preedit shrinks per Latin letter |
| 6 | `s`, Escape, Escape | first closes the list (preedit stays), second cancels the word |
| 7 | `ba`, Enter | با committed, no newline (default `enter = "commit"`) |
| 8 | `bc` | ب + grey dotted `c` (pending); `h` turns it into بچ |
| 9 | `mi`, Shift+Space, `xam`, Space | می‌خم (ZWNJ inside the word) |
| 10 | idle Shift+Space twice | ZWNJ inserted, then removed (read-back); if it inserts twice, note the host |
| 11 | idle `1 2 , ?` | ۱ ۲ ، ؟ |
| 12 | `ba`, Left arrow; `ba`, Ctrl+A | commits first, then the key acts |
| 13 | `ba`, click elsewhere | با stays, no underline left, next key starts fresh |
| 14 | `config.ps1 -Good`, click away and back | ASCII digits; list opens above the word |
| 15 | `config.ps1 -Broken`, click away and back | warning badge + tooltip on the ف button; typing still works (defaults) |
| 16 | `config.ps1 -Remove`, click away and back | badge clears |
| 17 | click the ف indicator -> A | Latin letters (US layout), nothing composes; back to ف |
| 18 | password field (Edge login) | nothing composes |
| 19 | `killswitch.ps1 -On`, switch away/back | nothing composes; `-Off` restores |
| 20 | Event Viewer > Application | no crash naming `pime_tip.dll` |

## Code map

`exports.rs` (DLL exports, `guard` = catch_unwind -> E_FAIL + poison), `factory.rs`,
`registry.rs` (register/unregister, kill switch), `settings.rs` (runtime switches),
`service.rs` (the TIP object and its sinks), `keysink.rs` (key decoding + ITfKeyEventSink),
`composition.rs` (pure Session `Output` -> `EditPlan`, golden replay tests), `edit.rs`
(edit sessions executing a plan), `anchor.rs` (GetTextExt anchor), `display.rs` (display
attributes). Stream B: `candidates.rs`; stream C: `config.rs`, `langbar.rs`, `reload.rs`.
