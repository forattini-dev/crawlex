//! BlockPattern source — Hot tier (B6).
//!
//! Substitutes the legacy `antibot::block_detector` 640-LOC module.
//! Where `AntibotMarkerSource` answers "**what** vendor blocked us?",
//! `BlockPattern` answers "**did we get blocked?**" — the two
//! questions are different, and a response can produce both signals
//! (high-confidence vendor + high-confidence block = strongest WAF
//! signal).
//!
//! Surface heuristics (BlockSurface from legacy):
//!   - HTTP-level: 403/429/503 with very small body, no Server header
//!   - HTML-level: title contains "Access Denied"/"Blocked"/
//!     "Forbidden"/"Bot Detected", body < 4 KiB
//!   - JS-level: small HTML + script-only body
//!   - Cookie-level: vendor antibot cookie set (covered by CookieSource;
//!     this source amplifies confidence when both fire)

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct BlockPatternSource;

impl BlockPatternSource {
    pub fn new() -> Self {
        Self
    }
}

const TINY_BODY: usize = 1024;
const SMALL_BODY: usize = 4 * 1024;

const BLOCK_TITLE_MARKERS: &[&str] = &[
    "access denied",
    "blocked",
    "forbidden",
    "bot detected",
    "are you a robot",
    "verify you are human",
    "checking your browser",
    "rate limited",
    "too many requests",
];

impl Source for BlockPatternSource {
    fn name(&self) -> &'static str {
        "block_pattern"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let mut out: Vec<Detection> = Vec::new();

        // Tier 1 — HTTP-level: 403 / 429 / 503 with tiny body, no
        // Server header. High-weight (8) because the combination is
        // strongly indicative.
        if matches!(ctx.status, 403 | 429 | 503)
            && ctx.body.len() < TINY_BODY
            && ctx.headers.get("server").is_none()
        {
            out.push(Detection::from_single(
                Category::Antibot,
                Vendor::Unknown,
                Evidence::new(
                    EvidenceSource::StatusPattern,
                    format!(
                        "block: status={} tiny body ({}B) no server",
                        ctx.status,
                        ctx.body.len()
                    ),
                    8,
                ),
            ));
        }

        // Tier 2 — HTML-level: title text matches a known block phrase.
        if is_html(ctx) && ctx.body.len() < SMALL_BODY {
            if let Some(title) = extract_title(ctx.body) {
                let lower = title.to_ascii_lowercase();
                for marker in BLOCK_TITLE_MARKERS {
                    if lower.contains(marker) {
                        out.push(Detection::from_single(
                            Category::Antibot,
                            Vendor::Unknown,
                            Evidence::new(
                                EvidenceSource::BodyMarker,
                                format!("block: <title> contains '{marker}' (title={title:?})"),
                                8,
                            ),
                        ));
                        break;
                    }
                }
            }
        }

        // Tier 3 — Rate-limit headers (Retry-After) regardless of status.
        if ctx.headers.get("retry-after").is_some() && matches!(ctx.status, 429 | 503) {
            out.push(Detection::from_single(
                Category::Antibot,
                Vendor::Unknown,
                Evidence::new(
                    EvidenceSource::Header,
                    format!("block: Retry-After on status={}", ctx.status),
                    6,
                ),
            ));
        }

        out
    }
}

fn is_html(ctx: &TargetContext<'_>) -> bool {
    ctx.headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false)
}

/// Cheap `<title>...</title>` extractor without a full HTML parse.
/// Substring + bounded scan; returns `None` if either tag is missing.
fn extract_title(body: &[u8]) -> Option<String> {
    let body_str = std::str::from_utf8(body).ok()?.to_ascii_lowercase();
    let start = body_str.find("<title")?;
    let after_open = body_str[start..].find('>')? + start + 1;
    let end_rel = body_str[after_open..].find("</title>")?;
    let raw = std::str::from_utf8(&body[after_open..after_open + end_rel]).ok()?;
    Some(raw.trim().to_string())
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

    fn html_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("content-type", "text/html".parse().unwrap());
        h
    }

    #[test]
    fn fires_on_403_tiny_no_server() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = BlockPatternSource::new().analyze(&ctx_with(&h, &u, 403, b"x"));
        assert!(dets.iter().any(|d| d.category == Category::Antibot));
    }

    #[test]
    fn fires_on_html_block_title() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><head><title>Access Denied</title></head><body></body></html>";
        let dets = BlockPatternSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.iter().any(|d| {
            d.evidence[0].detail.to_ascii_lowercase().contains("access denied")
        }));
    }

    #[test]
    fn fires_on_429_retry_after() {
        let mut h = HeaderMap::new();
        h.insert("retry-after", "60".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = BlockPatternSource::new().analyze(&ctx_with(&h, &u, 429, b"x"));
        // Status-pattern fires AND retry-after fires — at least one.
        assert!(dets.iter().any(|d| d.category == Category::Antibot));
    }

    #[test]
    fn does_not_fire_on_200_healthy() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><head><title>My Site</title></head><body><h1>hi</h1></body></html>";
        let dets = BlockPatternSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.is_empty(), "unexpected: {dets:?}");
    }

    #[test]
    fn skips_when_server_header_present() {
        let mut h = HeaderMap::new();
        h.insert("server", "nginx".parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = BlockPatternSource::new().analyze(&ctx_with(&h, &u, 403, b"x"));
        // Tier 1 skipped; no title; no retry-after → empty.
        assert!(dets.is_empty());
    }

    #[test]
    fn extract_title_handles_attrs_and_whitespace() {
        let body =
            b"<html><head><title  class=\"x\">  Bot Detected  </title></head></html>";
        let t = extract_title(body).unwrap();
        assert_eq!(t, "Bot Detected");
    }

    #[test]
    fn extract_title_handles_missing() {
        assert!(extract_title(b"<html><body>no title</body></html>").is_none());
    }
}
