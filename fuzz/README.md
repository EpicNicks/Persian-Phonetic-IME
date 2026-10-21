# Fuzzing pime-core

Standalone cargo-fuzz package (own `[workspace]`, not in the root workspace).
Invariants live in `src/lib.rs` (`check_session`, `check_config`).

- `session_keys`: byte 0 picks a preset, then 2 bytes per event (keys, ctrl/alt chords,
  focus loss, select_candidate). Checks no panic, no U+064A/U+0643/U+0629 unless
  `allow_arabic_forms`, preedit/composing consistency, bounded sizes, focus loss ends composition.
- `config_parse`: arbitrary bytes as a config file; must never panic and must yield a usable config.
- `session_complete`: like `session_keys` with autocomplete on/off, a `ListCompleter`, policy and
  completer changes. Completions extend the preedit, are unique and bounded (`max_completions`,
  9 rows), respect policy/`predict_min_chars`/pending letters, have no Arabic-only letters beyond
  the typed prefix; learn events only at episode end, deduplicated, and only for shown rows.
- `dict_from_bytes`: raw bytes or a valid `dict.bin` with bytes overwritten/truncated; accepted
  files answer `complete` within the trait contract (pime-dict).
- `dict_build_tsv`: bytes as TSV for `build_from_tsv`; output always loads, matches its report,
  never contains U+0643/U+064A/U+0629.
- `history_from_bytes`: `history.bin` parsing; accepted files round-trip byte-for-byte and
  survive record/decay/merge with clamped learned terms.

Without libFuzzer (works on Windows/stable; runs `fuzz/seeds/**` plus a pseudo-random sweep):

    CARGO_TARGET_DIR=target-fuzz cargo test --manifest-path fuzz/Cargo.toml --no-default-features

With libFuzzer (nightly, `cargo install cargo-fuzz`; may not work on Windows MSVC, prefer Linux/WSL):

    CARGO_TARGET_DIR=target-fuzz cargo +nightly fuzz run session_keys fuzz/seeds/session_keys
    CARGO_TARGET_DIR=target-fuzz cargo +nightly fuzz run config_parse fuzz/seeds/config_parse

`fuzz/corpus/` and `fuzz/artifacts/` are git-ignored; `fuzz/seeds/` is committed.
