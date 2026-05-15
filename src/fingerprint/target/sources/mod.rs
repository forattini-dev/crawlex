//! Source trait — the extension point. Every detection input plugs in
//! by implementing [`Source`] and registering with [`crate::fingerprint::Fingerprinter`].
//!
//! Slice B1 of PRD forattini-dev/crawlex#25.

use crate::fingerprint::detection::{Detection, Tier};
use crate::fingerprint::target::TargetContext;

pub mod header;
pub use header::HeaderSource;

/// One detector. `analyze` is synchronous; the engine handles any
/// async work (DNS, RDAP, oracle) before invoking sources.
pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    fn tier(&self) -> Tier;
    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection>;
}
