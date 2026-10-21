# CLAUDE.md

Persian Phonetic IME: a standalone Windows TSF input method (Rust) for typing Persian from Latin letters. No dependency on Qamooscheh or any other project.

## Orientation

1. Read `PLAN.md` (behaviour, config schema, decisions table in section 7, workstreams in section 8, status in section 9).
2. Read `docs/contracts.md` (cross-workstream interfaces, ownership, golden-test format). `docs/DESIGN.md` is the distilled architecture.
3. Work only inside the files your workstream owns (contracts.md "Ownership"). Do not edit another workstream's files, `PLAN.md` decisions, or the root `Cargo.toml` without being asked; report needed changes instead.

## Current layout

`crates/pime-core` (engine, config, Session; no Windows deps), `crates/pime-candidate-ui`, `crates/pime-tip` (TSF DLL), `crates/pime-settings`, `crates/pime-setup`, `crates/pime-ids` (frozen GUIDs), `crates/pime-dict` (autocomplete dictionary, history, runtime; see `docs/autocomplete.md`), `xtask` (build-dict, check-dict, eval-dict), `tools/wordlist` and `data/dict` (word list and licence notices), `fuzz/` (standalone cargo-fuzz package), `installer/` (WiX MSI), `data/` (presets, golden), `spike/` (disposable, excluded from the workspace), `docs/`. Not yet present from PLAN.md section 5.1: `harness/`.

## Rules

- **Never register the IME on this machine**: no regsvr32, no `RegisterProfile`, no `InstallLayoutOrTip`. A broken text service breaks typing everywhere. Building is fine; registration is tested only in a VM or Windows Sandbox by the user.
- **Cargo target dir**: never write to the shared `target\`. Set `CARGO_TARGET_DIR` to `target-<workstream>` (e.g. `target-ws2`); `target*/` is git-ignored.
- **Privacy**: no network, no telemetry, never log or persist typed content or put it in error messages.
- **Crash safety**: every COM entry point is wrapped in `catch_unwind` and returns `E_FAIL`; no work in `DllMain`; no file I/O on the per-key path. Bad config must never break typing.
- **Persian output**: use ی U+06CC and ک U+06A9; never emit Arabic-only ي ك ة by default.
- **Do not commit** unless the user asks. The repo may not be initialised.
- Read-only on the Qamooscheh repos.

## Code style

`rustfmt.toml` (max width 100); `cargo clippy -- -D warnings` must pass. Match surrounding style; comments only for the non-obvious why. Keep `pime-core` free of Windows types. Prefer tests over prose: golden cases in `data/golden/*.json` per contracts.md.

## Commands

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

(with `CARGO_TARGET_DIR` set as above). CI is `.github/workflows/ci.yml` (windows-latest).

## Where things are recorded

- Decisions: `PLAN.md` section 7 (D-numbers). Change a decision there only with the user's say-so.
- Interface changes: edit `docs/contracts.md` and mention it in your final report.
- Spike findings: `docs/spike.md`. Config reference: `docs/config-schema.md` (PLAN.md wins on conflict).
- Status: in `PLAN.md` section 9 set your workstream's row to `in progress` / `done` (or `blocked`), fill Owner/session and a one-line Note. Edit only your own row.
