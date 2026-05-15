//! StatusPattern source — Hot tier (B3).
//!
//! Block-shape heuristic: a 403 or 503 response with a very small body
//! and no `Server` header is the shape of "we got blocked, vendor
//! refuses to identify itself". Emits a low-confidence
//! `Antibot::Unknown` detection — `BlockPattern` in B6 will provide
//! the deeper variant; this source is the cheap heuristic.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct StatusPatternSource;

impl StatusPatternSource {
    pub fn new() -> Self {
        Self
    }
}

const SMALL_BODY_THRESHOLD: usize = 1024;

impl Source for StatusPatternSource {
    fn name(&self) -> &'static str {
        "status_pattern"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        if !matches!(ctx.status, 403 | 503) {
            return Vec::new();
        }
        if ctx.body.len() >= SMALL_BODY_THRESHOLD {
            return Vec::new();
        }
        if ctx.headers.get("server").is_some() {
            // A normal origin returned 403/503 with a small body —
            // doesn't fit the "vendor hidden" pattern.
            return Vec::new();
        }
        vec![Detection::from_single(
            Category::Antibot,
            Vendor::Unknown,
            Evidence::new(
                EvidenceSource::StatusPattern,
                format!(
                    "status={} small body ({}B) no server header",
                    ctx.status,
                    ctx.body.len()
                ),
                4,
            ),
        )]
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
        status: u16,
        body: &'a [u8],
    ) -> TargetContext<'a> {
        TargetContext::http_only(url, status, headers, body)
    }

    #[test]
    fn fires_on_403_small_no_server() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = StatusPatternSource::new().analyze(&ctx_with(&h, &u, 403, b"blocked"));
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].vendor, Vendor::Unknown);
    }

    #[test]
    fn skips_on_200() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = StatusPatternSource::new().analyze(&ctx_with(&h, &u, 200, b""));
        assert!(dets.is_empty());
    }

    #[test]
    fn skips_on_403_large_body() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let big = vec![b'x'; 2048];
        let dets = StatusPatternSource::new().analyze(&ctx_with(&h, &u, 403, &big));
        assert!(dets.is_empty());
    }

    #[test]
    fn skips_when_server_header_present() {
        let mut h = HeaderMap::new();
        h.insert("server", "nginx".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = StatusPatternSource::new().analyze(&ctx_with(&h, &u, 403, b"x"));
        assert!(dets.is_empty());
    }
}
