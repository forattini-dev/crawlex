//! `CliCommand` trait — the deepened CLI seam.
//!
//! Existing `cli/mod.rs::run()` dispatches a 10-arm match tree to ~15
//! `cmd_*` async functions. Each handler is a self-contained mini-CLI
//! (config load → DB open → execute → format output → print). Common
//! concerns (error display, JSON-vs-table formatting, exit codes) are
//! duplicated across handlers.
//!
//! This module lands the trait that lets each subcommand become an
//! adapter struct in `src/cli/commands/<subcommand>.rs`. Migration is
//! incremental: handlers keep working as free functions until they're
//! converted; once a `CliCommand` impl is in place, `run()` swaps the
//! match arm to construct + execute the command struct.
//!
//! Why a trait instead of "each cmd_* takes a Context arg":
//! * **Testability** — a test can construct a `CliCommand` and call
//!   `.execute(ctx)` without spawning a process.
//! * **Output uniformity** — `CliOutput` carries the formatted shape;
//!   the `CliRenderer` impl decides JSON vs human, exit code, ANSI on/off.
//! * **Locality** — adding a subcommand becomes one new file plus one
//!   line in the routing table, instead of editing the central match.
//!
//! See also: `src/cli/render.rs` (output formatting trait).

use async_trait::async_trait;

use crate::error::Result;

/// Read-only context every `CliCommand` receives. Carries shared
/// resources (config path, storage handle if needed) plus the output
/// renderer. Populated by `cli::run()` from the parsed `clap` args
/// before dispatching to the command.
pub struct CliContext {
    /// Renderer used by commands to materialise their `CliOutput`. The
    /// `--json` flag (per-subcommand or global) decides which adapter
    /// gets installed here.
    pub renderer: Box<dyn crate::cli::render::CliRenderer>,
}

/// What a command produces. A command may print incrementally (long
/// running crawls), but the final summary lands here for the renderer
/// to project into the operator's terminal or `--out` file.
#[derive(Debug)]
pub enum CliOutput {
    /// Nothing to print — exit code 0, silent. Used by commands that
    /// either streamed all their output already (crawl progress) or
    /// have side-effects only (queue purge, session drop).
    Silent,

    /// Single key/value pairs to print. Renderer chooses table or JSON.
    KeyValue(Vec<(String, String)>),

    /// Tabular output. Header row + body rows, columns aligned by the
    /// renderer.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },

    /// Free-form JSON. Used by structured exports (`fingerprint export`,
    /// `catalog show --json`).
    Json(serde_json::Value),

    /// Pre-formatted multi-line text. Renderer just prints. Used by
    /// commands that need exact layout control (the help-style output
    /// from `crawlex stealth catalog show` without `--json`).
    Lines(Vec<String>),
}

/// One subcommand. Implementations live in `src/cli/commands/`.
#[async_trait]
pub trait CliCommand: Send + Sync {
    /// Stable name used in error messages and tracing (`stealth.catalog.list`,
    /// `pages.run`, etc.). Returned from `name()` not as `const NAME` so
    /// dynamic-dispatch trait objects work.
    fn name(&self) -> &'static str;

    /// Execute the command. Returns the output for the renderer to
    /// project, or an error that the dispatch layer maps to a non-zero
    /// exit code + human message.
    async fn execute(&self, ctx: &CliContext) -> Result<CliOutput>;
}
