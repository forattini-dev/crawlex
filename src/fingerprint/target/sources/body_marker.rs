//! BodyMarker source — Hot tier (B2).
//!
//! Substring scan over the response body for vendor-revealing
//! signatures. Covers antibot vendor names, generic JS-stub /
//! `<noscript>` challenge heuristics, and platform markers
//! (Next.js, Magento). Body sample capped at 16 KiB to bound cost.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct BodyMarkerSource;

impl BodyMarkerSource {
    pub fn new() -> Self {
        Self
    }
}

/// 16 KiB cap matches the legacy escalation body sampler.
const BODY_SAMPLE_CAP: usize = 16 * 1024;

/// Antibot / WAF markers — only meaningful on 403/503 (the challenge
/// statuses). Same gate as the legacy `escalation::detect_antibot_vendor`.
const ANTIBOT_403_503_TABLE: &[(&str, u8, Vendor, &str)] = &[
    ("cf-chl-bypass", 10, Vendor::Cloudflare, "body marker 'cf-chl-bypass'"),
    ("Just a moment", 8, Vendor::Cloudflare, "body marker 'Just a moment'"),
    ("/cdn-cgi/challenge-platform/", 10, Vendor::Cloudflare, "body marker challenge-platform"),
    ("DataDome", 10, Vendor::DataDome, "body marker 'DataDome'"),
    ("PerimeterX", 10, Vendor::PerimeterX, "body marker 'PerimeterX'"),
    ("_Incapsula_", 10, Vendor::Imperva, "body marker '_Incapsula_'"),
    ("Imperva", 8, Vendor::Imperva, "body marker 'Imperva'"),
    ("distilnetworks", 10, Vendor::DistilNetworks, "body marker 'distilnetworks'"),
];

/// Platform / framework markers — fire on any status, identify the
/// runtime/CMS/ecommerce stack.
const PLATFORM_TABLE: &[(&str, u8, Category, Vendor, &str)] = &[
    ("__NEXT_DATA__", 10, Category::Frontend, Vendor::NextJs, "body marker __NEXT_DATA__"),
    ("__NUXT__", 10, Category::Frontend, Vendor::Nuxt, "body marker __NUXT__"),
    ("data-react", 6, Category::Frontend, Vendor::React, "body marker data-react*"),
    ("ng-version=", 10, Category::Frontend, Vendor::Angular, "body marker ng-version="),
    ("Shopify.theme", 10, Category::Ecommerce, Vendor::Shopify, "body marker Shopify.theme"),
    ("Magento", 6, Category::Ecommerce, Vendor::Magento, "body marker 'Magento'"),
    ("/skin/frontend/", 8, Category::Ecommerce, Vendor::Magento, "body marker /skin/frontend/"),
    ("VTEX", 8, Category::Ecommerce, Vendor::Vtex, "body marker 'VTEX'"),
    ("wp-content", 6, Category::Cms, Vendor::Wordpress, "body marker /wp-content/"),
    ("/wp-includes/", 8, Category::Cms, Vendor::Wordpress, "body marker /wp-includes/"),
    ("Drupal.settings", 10, Category::Cms, Vendor::Drupal, "body marker Drupal.settings"),
];

impl Source for BodyMarkerSource {
    fn name(&self) -> &'static str {
        "body_marker"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let mut out: Vec<Detection> = Vec::new();
        let sample = body_sample(ctx.body);
        let body_str = String::from_utf8_lossy(sample);

        // Antibot vendor markers — gated on 403/503 per legacy semantics.
        if matches!(ctx.status, 403 | 503) {
            for (needle, weight, vendor, detail) in ANTIBOT_403_503_TABLE {
                if body_str.contains(needle) {
                    out.push(Detection::from_single(
                        Category::Antibot,
                        *vendor,
                        Evidence::new(EvidenceSource::BodyMarker, *detail, *weight),
                    ));
                }
            }
        }

        // Generic JS-stub / noscript heuristic — only when the body is
        // small enough to look like a stub (legacy 2 KiB threshold).
        if is_html(ctx) && ctx.body.len() < 2048 {
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
                        "small HTML + noscript 'enable JavaScript'",
                        6,
                    ),
                ));
            }
        }

        // Platform / framework markers — fire on any status.
        for (needle, weight, cat, vendor, detail) in PLATFORM_TABLE {
            if body_str.contains(needle) {
                out.push(Detection::from_single(
                    *cat,
                    *vendor,
                    Evidence::new(EvidenceSource::BodyMarker, *detail, *weight),
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

    #[test]
    fn detects_cloudflare_403() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html>cf-chl-bypass</html>";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 403, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare));
    }

    #[test]
    fn detects_datadome_on_503() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"DataDome blocked";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 503, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::DataDome));
    }

    #[test]
    fn cloudflare_marker_on_200_does_not_fire_antibot() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        // Body contains cf-chl-bypass but status is 200 — legacy
        // gating says no antibot signal.
        let body = b"<html>article about cf-chl-bypass...</html>";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(!dets.iter().any(|d| d.category == Category::Antibot));
    }

    #[test]
    fn detects_nextjs() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body =
            b"<html><script id=\"__NEXT_DATA__\" type=\"application/json\">{}</script></html>";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::NextJs));
    }

    #[test]
    fn detects_magento_via_skin_frontend() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<link href=\"/skin/frontend/default/...\" />";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Magento));
    }

    #[test]
    fn js_stub_small_html_fires_unknown_antibot() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body =
            b"<html><script>window.location='/real';</script></html>";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.iter().any(|d| d.category == Category::Antibot));
    }

    #[test]
    fn noscript_challenge_fires_unknown_antibot() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><noscript>Please enable JavaScript</noscript></html>";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.iter().any(|d| d.category == Category::Antibot));
    }

    #[test]
    fn healthy_html_no_detections() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><body><h1>healthy</h1><p>plenty of plain content here</p></body></html>";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.is_empty(), "unexpected: {dets:?}");
    }
}
