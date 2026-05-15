//! Source trait — the extension point. Every detection input plugs in
//! by implementing [`Source`] and registering with [`crate::fingerprint::Fingerprinter`].
//!
//! Slice B1 of PRD forattini-dev/crawlex#25.

use crate::fingerprint::detection::{Detection, Tier};
use crate::fingerprint::target::TargetContext;

pub mod alt_svc;
pub mod antibot_marker;
pub mod block_pattern;
pub mod body_marker;
pub mod cookie;
pub mod favicon_hash;
pub mod h2_settings;
pub mod header;
pub mod json_ld;
pub mod link_rel;
pub mod meta_tag;
pub mod peer_cert;
pub mod robots_txt;
pub mod script_src;
pub mod status_pattern;
pub mod timing_pattern;
pub mod tls_server;
pub mod well_known;
pub use alt_svc::AltSvcSource;
pub use antibot_marker::AntibotMarkerSource;
pub use block_pattern::BlockPatternSource;
pub use body_marker::BodyMarkerSource;
pub use cookie::CookieSource;
pub use favicon_hash::FaviconHashSource;
pub use h2_settings::H2SettingsSource;
pub use header::HeaderSource;
pub use json_ld::JsonLdSource;
pub use link_rel::LinkRelSource;
pub use meta_tag::MetaTagSource;
pub use peer_cert::PeerCertSource;
pub use robots_txt::RobotsTxtSource;
pub use script_src::ScriptSrcSource;
pub use status_pattern::StatusPatternSource;
pub use timing_pattern::TimingPatternSource;
pub use tls_server::TlsServerHelloSource;
pub use well_known::WellKnownSource;

/// One detector. `analyze` is synchronous; the engine handles any
/// async work (DNS, RDAP, oracle) before invoking sources.
pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    fn tier(&self) -> Tier;
    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection>;
}
