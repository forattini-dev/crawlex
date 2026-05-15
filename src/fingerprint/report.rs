//! Aggregated output of one fingerprint analysis.
//!
//! Slice B1 of PRD forattini-dev/crawlex#25. `FingerprintReport` uses
//! typed slots per `Category` (instead of a flat `Vec<Detection>`) so
//! hot callers (runner deciding escalation, CLI rendering) can hit the
//! slot they care about directly. Single-detection slots
//! (`tls_profile`, `http_fingerprint`) and the embedded
//! `self_fp`/`coherence` are stubs filled by later slices.

use serde::{Deserialize, Serialize};

use crate::fingerprint::detection::Detection;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FingerprintReport {
    pub host: String,

    pub cdn: Vec<Detection>,
    pub waf: Vec<Detection>,
    pub antibot: Vec<Detection>,
    pub cms: Vec<Detection>,
    pub ecommerce: Vec<Detection>,
    pub frontend: Vec<Detection>,
    pub backend: Vec<Detection>,
    pub webserver: Vec<Detection>,
    pub proxy_lb: Vec<Detection>,
    pub cache: Vec<Detection>,
    pub analytics: Vec<Detection>,
    pub tag_manager: Vec<Detection>,
    pub ab_testing: Vec<Detection>,
    pub auth: Vec<Detection>,
    pub payment: Vec<Detection>,
    pub chat: Vec<Detection>,
    pub dns_hosting: Vec<Detection>,
    pub cookie_pattern: Vec<Detection>,
    pub other: Vec<Detection>,

    /// Which tiers ran. Hot is always true; Warm/Cold are filled by
    /// engine entry points landed in B8/B9.
    pub tiers_run: Tiers,

    /// Cross-check FP-A vs FP-B. Populated by B13.
    pub coherence: Coherence,
}

impl FingerprintReport {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            ..Default::default()
        }
    }

    /// Sum of detections across every slot. Convenience for tests and
    /// CLI summaries.
    pub fn total_detections(&self) -> usize {
        self.cdn.len()
            + self.waf.len()
            + self.antibot.len()
            + self.cms.len()
            + self.ecommerce.len()
            + self.frontend.len()
            + self.backend.len()
            + self.webserver.len()
            + self.proxy_lb.len()
            + self.cache.len()
            + self.analytics.len()
            + self.tag_manager.len()
            + self.ab_testing.len()
            + self.auth.len()
            + self.payment.len()
            + self.chat.len()
            + self.dns_hosting.len()
            + self.cookie_pattern.len()
            + self.other.len()
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Tiers {
    pub hot: bool,
    pub warm: bool,
    pub cold: bool,
}

impl Tiers {
    pub fn hot() -> Self {
        Self {
            hot: true,
            warm: false,
            cold: false,
        }
    }
}

/// Cross-check between target detection (FP-A) and our outbound
/// self-fingerprint (FP-B). Populated by B13 once `SelfFingerprint`
/// lands in B10/B11.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Coherence {
    pub our_ja3_matches_profile: Option<bool>,
    pub their_antibot_compatible_with_our_profile: Option<bool>,
    pub warnings: Vec<String>,
}
