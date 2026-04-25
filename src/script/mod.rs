//! ScriptSpec v1 — unified AST for declarative crawl scripts.
//!
//! Playwright/Puppeteer-inspired contract that lets a single file describe
//! everything a render job should do end-to-end: selectors, steps,
//! captures, assertions, exports. JSON, YAML and Lua all parse to this
//! same AST so the executor is backend-agnostic.
//!
//! ## Example (JSON)
//!
//! ```json
//! {
//!   "version": 1,
//!   "defaults": { "timeout_ms": 10000 },
//!   "selectors": {
//!     "email": "role=textbox[name=\"Email\"]",
//!     "login": "role=button[name=\"Sign in\"]"
//!   },
//!   "steps": [
//!     { "goto": "https://example.com/login" },
//!     { "type": { "locator": "@email", "text": "a@b.c" } },
//!     { "click": { "locator": "@login" } },
//!     { "wait_for": { "locator": "role=heading[name=\"Dashboard\"]" } }
//!   ],
//!   "captures": [
//!     { "screenshot": { "mode": "full_page", "name": "dashboard" } },
//!     { "snapshot": { "kind": "post_js_html" } }
//!   ],
//!   "assertions": [
//!     { "contains": { "locator": "body", "text": "Welcome" } }
//!   ],
//!   "exports": {
//!     "title": "text=h1",
//!     "items": { "locator": "@cards", "as": "list" }
//!   }
//! }
//! ```
//!
//! Selector names prefixed with `@` reference the `selectors` map; bare
//! selectors use the full selector DSL (see `render::selector`).
//!
//! Design notes:
//!   * `defaults.timeout_ms` sets the fallback actionability timeout per
//!     step; individual steps can override.
//!   * `captures` always emit `artifact.saved` NDJSON events with a
//!     `stage` matching the capture kind.
//!   * `assertions` are checked in order; a failure emits
//!     `job.failed` with `why=assertion:<name>` and halts the script.
//!   * `exports` populate the `ExtractCompleted` event payload.

pub mod executor;
#[cfg(feature = "cdp-backend")]
pub mod legacy;
#[cfg(feature = "cdp-backend")]
pub mod runner;
pub mod spec;

pub use executor::{plan, Plan, PlanError, ResolvedExport, ResolvedStep};
#[cfg(feature = "cdp-backend")]
pub use legacy::actions_to_script_spec;
#[cfg(feature = "cdp-backend")]
pub use runner::{ArtifactRef, RunOutcome, ScriptRunner, StepOutcome};
pub use spec::*;
