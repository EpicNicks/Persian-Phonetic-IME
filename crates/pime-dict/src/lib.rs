//! Word-frequency dictionary, adaptive history and runtime for autocomplete.
//! Contract: docs/autocomplete.md section 3.

#![warn(missing_docs)]

mod build;
mod dict;
pub mod eval;
mod format;
mod fuzzy;
mod history;
mod runtime;
mod score;

pub use build::{build_from_tsv, BuildError, BuildOptions, BuildReport};
pub use dict::{Dictionary, RankCtx};
pub use format::DictError;
pub use history::{Entry, History, HistoryError, Rejection};
pub use runtime::{LiveCompleter, Runtime, RuntimeOptions, RuntimePaths, Status};
pub use score::{Day, ScoreParams};
