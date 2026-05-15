//! Target fingerprinting (FP-A).
//!
//! `TargetContext` carries the inputs every Source reads from. Slice
//! B1 ships the Hot-tier surface; Warm and Cold tiers grow this
//! struct's optional slots (h2 SETTINGS, robots.txt, well-known,
//! favicon hash, DNS, ASN, peer cert) in B8/B9.

use http::HeaderMap;
use url::Url;

pub mod cache;
pub mod engine;
pub mod sources;

pub use cache::{CachedFingerprint, WarmCache};
pub use engine::Engine;

/// Per-target input bundle. Hot sources see fields populated by the
/// fetch path; Warm and Cold sources see the optional slots filled in
/// by the engine's higher-tier dispatch.
pub struct TargetContext<'a> {
    pub final_url: &'a Url,
    pub status: u16,
    pub headers: &'a HeaderMap,
    pub body: &'a [u8],
}

impl<'a> TargetContext<'a> {
    /// Construct a Hot-tier context — the minimum slots Hot sources
    /// can read. Higher tiers extend with Warm/Cold builders later.
    pub fn http_only(
        final_url: &'a Url,
        status: u16,
        headers: &'a HeaderMap,
        body: &'a [u8],
    ) -> Self {
        Self {
            final_url,
            status,
            headers,
            body,
        }
    }

    /// `host:port` cache key. Falls back to the URL's scheme default
    /// (443/443/80) when the URL omits an explicit port. Used by
    /// Warm-tier cache lookups in B8.
    pub fn host_label(&self) -> String {
        let host = self.final_url.host_str().unwrap_or("");
        let port = self
            .final_url
            .port()
            .or_else(|| match self.final_url.scheme() {
                "https" | "wss" => Some(443),
                "http" | "ws" => Some(80),
                _ => None,
            })
            .unwrap_or(0);
        format!("{host}:{port}")
    }
}
