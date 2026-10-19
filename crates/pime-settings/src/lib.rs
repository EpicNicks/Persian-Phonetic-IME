//! Settings app for the Persian Phonetic IME: config file paths, the `--check`
//! report, comment-preserving edits, backups/reset, the default config text, the
//! "config changed" reload signal and the "try typing" preview.
//!
//! Everything here except [`reload`] (and the binary's window code) is plain Rust
//! with no Windows dependency, so it is unit-tested on any host. The settings app
//! always runs as its own process; nothing in this crate is loaded into host apps.
//!
//! Contract for the TIP (text service) side, see also `README.md`:
//! - File locations: [`paths::Paths`] (`PIME_CONFIG_DIR` overrides all of them).
//! - Decoding the file: `pime_core::decode_config_bytes` / `load_config_bytes`, shared
//!   by both sides (UTF-8 with or without BOM, UTF-16 with BOM; else whole-file fallback).
//! - `config-errors.txt` content: [`report::errors_file_text`].
//! - Reload signal: [`reload`] (event name, pulse protocol).
//! - Exit codes of `pime-settings --check`: [`report::exit`].

pub mod backup;
pub mod cli;
pub mod defaults;
pub mod edit;
pub mod options;
pub mod paths;
pub mod reload;
pub mod report;
pub mod text;
pub mod try_typing;

#[cfg(test)]
mod testutil;
