//! Fingerprinting (`fingerprint/`).
//!
//! Built per PRD forattini-dev/crawlex#25. Two directions:
//!
//! - `target/` — FP-A: detection of the host we hit. Categories cover
//!   CDN, WAF, Antibot, CMS, Ecommerce, Frontend, Backend, WebServer,
//!   ReverseProxy, Cache, Analytics, TagManager, AbTesting, Auth,
//!   Payment, Chat, DnsHosting, CookiePattern, and Other. Sources
//!   plug into the engine via the [`target::sources::Source`] trait.
//!
//! - `self/` — FP-B: introspection of our outbound identity. Lives in
//!   a separate submodule that lands in slices B10–B12.
//!
//! Slice B1 (this slice) ships the engine, the shared types, the
//! `FingerprintReport` aggregator, and a tracer-bullet `Header` source
//! so the architecture is exercised end-to-end before the rest of the
//! sources land.

use std::sync::Arc;

pub mod coherence;
pub mod detection;
pub mod introspect;
pub mod report;
pub mod target;

pub use coherence::compute_coherence;

pub use detection::{Category, Confidence, Detection, Evidence, EvidenceSource, Tier, Vendor};
pub use report::{Coherence, FingerprintReport, Tiers};
pub use target::{Engine, TargetContext};

/// Top-level entry. Holds an [`Engine`] and exposes the
/// `analyze_hot/warm/cold` methods consumers reach for. Cache, oracle,
/// and self-introspection plumbing land in later slices.
pub struct Fingerprinter {
    engine: Engine,
}

impl Default for Fingerprinter {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl Fingerprinter {
    pub fn new(engine: Engine) -> Self {
        Self { engine }
    }

    pub fn with_defaults() -> Self {
        Self::new(Engine::with_defaults())
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Run the Hot tier against `ctx` and return an aggregated report.
    pub fn analyze_hot(&self, ctx: &TargetContext<'_>) -> FingerprintReport {
        self.engine.analyze_hot(ctx)
    }

    /// Convenience for callers that already hold an `Arc<Fingerprinter>`.
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use url::Url;

    fn ctx<'a>(headers: &'a HeaderMap, url: &'a Url, body: &'a [u8]) -> TargetContext<'a> {
        TargetContext::http_only(url, 200, headers, body)
    }

    #[test]
    fn fingerprinter_default_detects_cloudflare_via_cf_ray() {
        let mut h = HeaderMap::new();
        h.insert("cf-ray", "8a3abc-LAX".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let fp = Fingerprinter::default();
        let report = fp.analyze_hot(&ctx(&h, &u, b""));
        assert_eq!(report.host, "example.com:443");
        assert!(!report.cdn.is_empty(), "expected cdn detection");
        assert_eq!(report.cdn[0].vendor, Vendor::Cloudflare);
        assert_eq!(report.cdn[0].confidence, Confidence::High);
        assert!(report.tiers_run.hot);
    }

    #[test]
    fn fingerprinter_empty_engine_yields_empty_report() {
        let mut h = HeaderMap::new();
        h.insert("cf-ray", "x".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let fp = Fingerprinter::new(Engine::new());
        let report = fp.analyze_hot(&ctx(&h, &u, b""));
        assert_eq!(report.total_detections(), 0);
    }

    #[test]
    fn host_label_carries_port() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com:8443/".parse().unwrap();
        let ctx = ctx(&h, &u, b"");
        assert_eq!(ctx.host_label(), "example.com:8443");
    }

    #[test]
    fn host_label_defaults_https_443() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let ctx = ctx(&h, &u, b"");
        assert_eq!(ctx.host_label(), "example.com:443");
    }

    #[test]
    fn host_label_defaults_http_80() {
        let h = HeaderMap::new();
        let u: Url = "http://example.com/".parse().unwrap();
        let ctx = ctx(&h, &u, b"");
        assert_eq!(ctx.host_label(), "example.com:80");
    }

    #[test]
    fn report_total_counts_all_slots() {
        let mut h = HeaderMap::new();
        h.insert("cf-ray", "x".parse().unwrap());
        h.insert("server", "nginx/1.21".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let fp = Fingerprinter::default();
        let report = fp.analyze_hot(&ctx(&h, &u, b""));
        // cf-ray → cdn (Cloudflare), server nginx → webserver (Nginx)
        assert!(report.total_detections() >= 2);
    }

    #[test]
    fn fingerprinter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Fingerprinter>();
    }
}
