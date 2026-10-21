//! Persian Phonetic IME text service (TSF). Module ownership while the shell is built in
//! parallel: A = exports, service, keysink, composition, presenter (trait), tests;
//! B = candidates; C = config, langbar, reload. Do not edit another stream's files; the
//! cross-module interfaces below are pinned in `presenter.rs`, `config.rs` and `langbar.rs`.
#![allow(dead_code)]

mod anchor; // A
pub mod autocomplete; // AC-E
pub mod candidates; // B
pub mod composition; // A
pub mod config; // C
mod display; // A
mod edit; // A
mod exports; // A
mod factory; // A
pub mod keysink; // A
pub mod langbar; // C
pub mod presenter; // A defines (pinned), B implements
mod registry; // A
pub mod reload; // C
mod service; // A
pub mod settings; // A

pub use exports::{DllCanUnloadNow, DllGetClassObject, DllRegisterServer, DllUnregisterServer};

#[cfg(test)]
mod com_tests; // A
