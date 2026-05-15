//! WellKnown source — Warm tier (B8).
//!
//! Probes well-known URIs (RFC 8615): /.well-known/security.txt,
//! /.well-known/dnt-policy.txt, /.well-known/openid-configuration.
//! Emits low-confidence "this host exposes well-known X" Detections
//! for recon mapping.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct WellKnownSource;

impl WellKnownSource {
    pub fn new() -> Self {
        Self
    }

    /// Inspect a fetched well-known probe result. Caller (engine)
    /// fetches the path; this function classifies the body/status.
    pub fn classify(path: &str, status: u16, body_has_content: bool) -> Vec<Detection> {
        if status != 200 || !body_has_content {
            return Vec::new();
        }
        let mut out: Vec<Detection> = Vec::new();
        if path.ends_with("/security.txt") {
            out.push(Detection::from_single(
                Category::Other,
                Vendor::Generic,
                Evidence::new(
                    EvidenceSource::WellKnown,
                    "/.well-known/security.txt present",
                    3,
                ),
            ));
        }
        if path.ends_with("/openid-configuration") {
            out.push(Detection::from_single(
                Category::Auth,
                Vendor::Unknown,
                Evidence::new(
                    EvidenceSource::WellKnown,
                    "/.well-known/openid-configuration present (OIDC issuer)",
                    8,
                ),
            ));
        }
        out
    }
}

impl Source for WellKnownSource {
    fn name(&self) -> &'static str {
        "well_known"
    }

    fn tier(&self) -> Tier {
        Tier::Warm
    }

    fn analyze(&self, _ctx: &TargetContext<'_>) -> Vec<Detection> {
        // Well-known probes live outside TargetContext. Engine's
        // Warm-tier dispatch calls `classify` with fetched results.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_security_txt() {
        let dets = WellKnownSource::classify("/.well-known/security.txt", 200, true);
        assert_eq!(dets.len(), 1);
    }

    #[test]
    fn classifies_openid_configuration() {
        let dets = WellKnownSource::classify("/.well-known/openid-configuration", 200, true);
        assert!(dets.iter().any(|d| d.category == Category::Auth));
    }

    #[test]
    fn skips_404() {
        let dets = WellKnownSource::classify("/.well-known/security.txt", 404, false);
        assert!(dets.is_empty());
    }
}
