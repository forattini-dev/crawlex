//! PeerCert source — Hot tier (B4).
//!
//! Peer certificate evidence — TLS issuer organization, subject
//! organization, SAN. `O=Akamai Technologies` is the canonical
//! example of an org field that pins CDN identity.
//!
//! TargetContext today does not carry a peer-cert slot directly; the
//! source is wired so the moment B8 adds `peer_cert: Option<&PeerCert>`
//! the matchers fire. For B4 this source emits nothing in production
//! — its tests use a synthetic context expansion guard.

use crate::fingerprint::detection::{Detection, Tier};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct PeerCertSource;

impl PeerCertSource {
    pub fn new() -> Self {
        Self
    }
}

impl Source for PeerCertSource {
    fn name(&self) -> &'static str {
        "peer_cert"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, _ctx: &TargetContext<'_>) -> Vec<Detection> {
        // No peer_cert slot in TargetContext yet — added in B8 along
        // with Warm-tier facts. Source registered so the wiring is
        // ready when that lands.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use url::Url;

    #[test]
    fn returns_empty_until_target_context_gains_peer_cert() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let ctx = TargetContext::http_only(&u, 200, &h, b"");
        assert!(PeerCertSource::new().analyze(&ctx).is_empty());
    }
}
