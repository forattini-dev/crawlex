//! TimingPattern source — Hot tier (B4).
//!
//! Crude transport-tier heuristic: when the Server header is missing
//! and the response body is large, the origin likely answered without
//! CDN cache layer in between — feeds low-confidence "no edge cache"
//! evidence. The richer signal (TTFB > N ms, network timings) flows
//! in via Warm tier when B8 adds the per-host facts slot.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct TimingPatternSource;

impl TimingPatternSource {
    pub fn new() -> Self {
        Self
    }
}

const LARGE_BODY: usize = 100 * 1024;

impl Source for TimingPatternSource {
    fn name(&self) -> &'static str {
        "timing_pattern"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        if ctx.status != 200 {
            return Vec::new();
        }
        if ctx.body.len() < LARGE_BODY {
            return Vec::new();
        }
        let no_server = ctx.headers.get("server").is_none();
        let no_via = ctx.headers.get("via").is_none();
        let no_cache_status = ctx.headers.get("x-cache").is_none()
            && ctx.headers.get("cf-cache-status").is_none()
            && ctx.headers.get("x-served-by").is_none()
            && ctx.headers.get("age").is_none();
        if no_server && no_via && no_cache_status {
            return vec![Detection::from_single(
                Category::Other,
                Vendor::Generic,
                Evidence::new(
                    EvidenceSource::TimingPattern,
                    format!(
                        "200 OK, {} KiB body, no server / via / cache headers (likely origin-only)",
                        ctx.body.len() / 1024
                    ),
                    2,
                ),
            )];
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use url::Url;

    fn ctx_with<'a>(
        headers: &'a HeaderMap,
        url: &'a Url,
        body: &'a [u8],
    ) -> TargetContext<'a> {
        TargetContext::http_only(url, 200, headers, body)
    }

    #[test]
    fn fires_on_large_body_no_cache_headers() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = vec![b'x'; 200_000];
        let dets = TimingPatternSource::new().analyze(&ctx_with(&h, &u, &body));
        assert_eq!(dets.len(), 1);
    }

    #[test]
    fn skips_when_server_header_present() {
        let mut h = HeaderMap::new();
        h.insert("server", "nginx".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let body = vec![b'x'; 200_000];
        let dets = TimingPatternSource::new().analyze(&ctx_with(&h, &u, &body));
        assert!(dets.is_empty());
    }

    #[test]
    fn skips_when_cache_header_present() {
        let mut h = HeaderMap::new();
        h.insert("cf-cache-status", "HIT".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let body = vec![b'x'; 200_000];
        let dets = TimingPatternSource::new().analyze(&ctx_with(&h, &u, &body));
        assert!(dets.is_empty());
    }

    #[test]
    fn skips_on_small_body() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = TimingPatternSource::new().analyze(&ctx_with(&h, &u, b"tiny"));
        assert!(dets.is_empty());
    }
}
