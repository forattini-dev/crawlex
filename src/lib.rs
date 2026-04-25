// Style-only clippy lints we've chosen not to chase — each flags working
// code that reviewers disagree on. The bug-catching lints (await_holding_lock,
// unused imports, dead code, etc.) stay enabled.
#![allow(macro_expanded_macro_exports_accessed_by_absolute_paths)]
#![allow(
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::enum_variant_names,
    clippy::len_without_is_empty,
    clippy::doc_lazy_continuation,
    clippy::explicit_auto_deref,
    clippy::collapsible_if,
    clippy::field_reassign_with_default,
    clippy::useless_format,
    clippy::match_like_matches_macro,
    clippy::single_match,
    clippy::while_let_loop,
    clippy::needless_range_loop,
    clippy::manual_strip,
    clippy::duplicated_attributes,
    clippy::empty_line_after_doc_comments,
    clippy::manual_pattern_char_comparison,
    clippy::unnecessary_lazy_evaluations,
    clippy::or_fun_call,
    clippy::redundant_clone,
    clippy::single_component_path_imports,
    clippy::needless_borrow,
    clippy::single_char_pattern,
    clippy::extra_unused_lifetimes,
    clippy::let_unit_value,
    clippy::manual_inspect
)]

pub mod antibot;
pub mod config;
pub mod crawler;
pub mod discovery;
pub mod error;
pub mod escalation;
pub mod events;
pub mod extract;
pub mod frontier;
pub mod hooks;
pub mod http;
pub mod identity;
pub mod impersonate;
#[cfg(feature = "sqlite")]
pub mod intel;
pub mod metrics;
pub mod metrics_server;
pub mod policy;
pub mod proxy;
pub mod queue;
/// Render/browser path. Only compiled when `cdp-backend` is
/// enabled. `crawlex-mini` builds without this module; callers that need
/// runtime "render not available" errors use `Error::RenderDisabled`.
#[cfg(feature = "cdp-backend")]
pub mod render;
pub mod robots;
pub mod scheduler;
pub mod script;
pub mod storage;
pub mod url_util;
pub mod wait_strategy;

#[cfg(feature = "cli")]
pub mod cli;

pub use config::{Config, ConfigBuilder};
pub use crawler::Crawler;
pub use error::{Error, Result};
pub use hooks::{HookDecision, HookEvent};
