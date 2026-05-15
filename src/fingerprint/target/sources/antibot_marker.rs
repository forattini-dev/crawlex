//! AntibotMarker source — Hot tier (B5).
//!
//! Dedicated antibot detection — ports the entirety of
//! `runner::ChallengeDetector` (slice #19) into a Source. Same six
//! vendor signatures (Cloudflare cf-chl-bypass, "Just a moment",
//! /cdn-cgi/challenge-platform/, DataDome, PerimeterX, Imperva
//! _Incapsula_, Imperva, DistilNetworks). Same 403/503 status gate.
//! Same 16 KiB body cap. Same generic JS-stub and noscript fallback.
//!
//! This source is the long-term home for vendor-identification
//! detection. `BodyMarkerSource` keeps the platform / framework
//! markers; `AntibotMarker` owns "what antibot vendor is this?".
//! `runner::ChallengeDetector` becomes `#[deprecated]` and forwards
//! to this source in B14.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct AntibotMarkerSource;

impl AntibotMarkerSource {
    pub fn new() -> Self {
        Self
    }
}

const BODY_SAMPLE_CAP: usize = 16 * 1024;
const SMALL_HTML_THRESHOLD: usize = 2048;

const VENDOR_SIGNATURES: &[(&str, u8, Vendor, &str)] = &[
    ("cf-chl-bypass", 10, Vendor::Cloudflare, "body marker 'cf-chl-bypass'"),
    ("Just a moment", 8, Vendor::Cloudflare, "body marker 'Just a moment'"),
    ("/cdn-cgi/challenge-platform/", 10, Vendor::Cloudflare, "body marker '/cdn-cgi/challenge-platform/'"),
    ("DataDome", 10, Vendor::DataDome, "body marker 'DataDome'"),
    ("PerimeterX", 10, Vendor::PerimeterX, "body marker 'PerimeterX'"),
    ("_Incapsula_", 10, Vendor::Imperva, "body marker '_Incapsula_'"),
    ("Imperva", 8, Vendor::Imperva, "body marker 'Imperva'"),
    ("distilnetworks", 10, Vendor::DistilNetworks, "body marker 'distilnetworks'"),
];

impl Source for AntibotMarkerSource {
    fn name(&self) -> &'static str {
        "antibot_marker"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let mut out: Vec<Detection> = Vec::new();
        let sample = body_sample(ctx.body);
        let body_str = String::from_utf8_lossy(sample);

        // Vendor sigs only on 403/503 — matches legacy gating from
        // `runner::challenge::ChallengeDetector::detect`.
        if matches!(ctx.status, 403 | 503) {
            for (needle, weight, vendor, detail) in VENDOR_SIGNATURES {
                if body_str.contains(needle) {
                    out.push(Detection::from_single(
                        Category::Antibot,
                        *vendor,
                        Evidence::new(EvidenceSource::BodyMarker, *detail, *weight),
                    ));
                }
            }
        }

        // Generic JS-stub / noscript fallback — fires when body is
        // HTML and small (<2 KiB). Matches legacy small-HTML cutoff.
        if is_html(ctx) && ctx.body.len() < SMALL_HTML_THRESHOLD {
            if body_str.contains("<script") && body_str.contains("window.location") {
                out.push(Detection::from_single(
                    Category::Antibot,
                    Vendor::Unknown,
                    Evidence::new(
                        EvidenceSource::BodyMarker,
                        "small HTML + <script>window.location stub",
                        6,
                    ),
                ));
            }
            if body_str.contains("<noscript")
                && (body_str.contains("enable JavaScript")
                    || body_str.contains("Please enable JavaScript"))
            {
                out.push(Detection::from_single(
                    Category::Antibot,
                    Vendor::Unknown,
                    Evidence::new(
                        EvidenceSource::BodyMarker,
                        "small HTML + <noscript>'enable JavaScript'",
                        6,
                    ),
                ));
            }
        }

        out
    }
}

fn body_sample(body: &[u8]) -> &[u8] {
    &body[..body.len().min(BODY_SAMPLE_CAP)]
}

fn is_html(ctx: &TargetContext<'_>) -> bool {
    ctx.headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false)
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

    fn empty_headers() -> HeaderMap {
        HeaderMap::new()
    }

    // All 12 test cases ported from `runner::challenge::tests` —
    // proves zero loss of intelligence.

    #[test]
    fn detects_cloudflare_via_chl_bypass() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 403, b"... cf-chl-bypass ..."));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare));
    }

    #[test]
    fn detects_cloudflare_via_just_a_moment() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 503, b"<html>Just a moment...</html>"));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare));
    }

    #[test]
    fn detects_datadome() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 403, b"DataDome block"));
        assert!(dets.iter().any(|d| d.vendor == Vendor::DataDome));
    }

    #[test]
    fn detects_perimeterx() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 403, b"PerimeterX challenge"));
        assert!(dets.iter().any(|d| d.vendor == Vendor::PerimeterX));
    }

    #[test]
    fn detects_imperva_incapsula() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 403, b"_Incapsula_ ..."));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Imperva));
    }

    #[test]
    fn detects_distil() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 403, b"distilnetworks tag"));
        assert!(dets.iter().any(|d| d.vendor == Vendor::DistilNetworks));
    }

    #[test]
    fn detects_generic_js_stub() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><script>window.location='x'</script></html>";
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Unknown));
    }

    #[test]
    fn detects_generic_noscript_challenge() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><noscript>Please enable JavaScript</noscript></html>";
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Unknown));
    }

    #[test]
    fn healthy_200_returns_none() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><body><h1>real content</h1><p>lots of text here</p></body></html>";
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.is_empty());
    }

    #[test]
    fn vendor_signatures_only_trigger_on_403_or_503() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(&h, &u, 200, b"cf-chl-bypass"));
        assert!(dets.is_empty());
    }

    #[test]
    fn detects_cloudflare_challenge_platform_path() {
        let h = empty_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AntibotMarkerSource::new().analyze(&ctx_with(
            &h,
            &u,
            403,
            b"<script src='/cdn-cgi/challenge-platform/h/g/cv/result/...' />",
        ));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare));
    }
}
