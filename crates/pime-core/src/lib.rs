//! Engine of the Persian Phonetic IME: configuration, tokeniser/renderer and the
//! composition [`Session`]. No Windows dependencies; builds and tests anywhere.
//!
//! Typical host use:
//!
//! ```
//! use pime_core::{keys, load_config, Dictionary, KeyEvent, Session};
//! use std::sync::Arc;
//!
//! let loaded = load_config("[options]\ndigits = \"ascii\"\n");
//! assert!(loaded.issues.is_empty());
//! let mut s = Session::new(Arc::new(loaded.config), Arc::new(Dictionary::empty()));
//! s.on_key(&KeyEvent::plain(keys::VK_A + 18)); // s
//! let out = s.on_key(&KeyEvent::plain(keys::VK_A + 7)); // h
//! assert_eq!(out.preedit, "ش");
//! let out = s.on_key(&KeyEvent::plain(keys::VK_SPACE));
//! assert_eq!(out.commit, "ش ");
//! ```

#![warn(missing_docs)]

pub mod complete;
pub mod config;
pub mod keys;
mod labels;
pub mod render;
mod session;

pub use complete::{
    completable, complete_with_zwnj_rule, fuzzy_max_edits, match_key, prefix_edit_distance,
    CompleteCtx, Completer, Completion, CompletionList, ContextPolicy, LearnEvent, LearnKind,
    ListCompleter, ZWNJ,
};
pub use config::{
    autocomplete_specs, builtin_preset_names, decode_config_bytes, fallback_of, format_issues,
    format_issues_for, load_config, load_config_bytes, load_config_with_base, option_specs,
    ConfigIssue, DecodedConfig, Fallback, IssueKind, LoadedConfig, OptionKind, OptionSpec,
    OptionValue, ResolvedConfig, Severity, TextEncoding,
};
pub use labels::candidate_label;
pub use render::{render, ChoiceOverride};
pub use session::{CandidateKind, CandidateList, Dictionary, KeyEvent, Output, Session};
