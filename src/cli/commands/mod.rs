//! Subcommand adapters — one struct per CLI verb.
//!
//! Each adapter implements [`CliCommand`](crate::cli::command::CliCommand)
//! and lives in its own file. The dispatch layer in `cli::run()`
//! constructs the appropriate adapter from parsed `clap` args, hands
//! it the [`CliContext`](crate::cli::command::CliContext), and projects
//! the returned [`CliOutput`](crate::cli::command::CliOutput) via the
//! installed renderer.
//!
//! ## Migration status
//!
//! Adapters land here incrementally. The legacy `cmd_*` async fns in
//! `cli/mod.rs` continue to work; once a command's logic moves into an
//! adapter struct, the corresponding match arm in `run()` switches to
//! the new dispatch path. No big-bang flip.

pub mod catalog_list;

pub use catalog_list::CatalogListCommand;
