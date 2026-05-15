//! Source trait — the extension point. Every detection input plugs in
//! by implementing [`Source`] and registering with [`crate::fingerprint::Fingerprinter`].
//!
//! Slice B1 of PRD forattini-dev/crawlex#25.

use crate::fingerprint::detection::{Detection, Tier};
use crate::fingerprint::target::TargetContext;

pub mod alt_svc;
pub mod body_marker;
pub mod cookie;
pub mod header;
pub mod json_ld;
pub mod link_rel;
pub mod meta_tag;
pub mod script_src;
pub mod status_pattern;
pub use alt_svc::AltSvcSource;
pub use body_marker::BodyMarkerSource;
pub use cookie::CookieSource;
pub use header::HeaderSource;
pub use json_ld::JsonLdSource;
pub use link_rel::LinkRelSource;
pub use meta_tag::MetaTagSource;
pub use script_src::ScriptSrcSource;
pub use status_pattern::StatusPatternSource;

/// One detector. `analyze` is synchronous; the engine handles any
/// async work (DNS, RDAP, oracle) before invoking sources.
pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    fn tier(&self) -> Tier;
    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection>;
}
