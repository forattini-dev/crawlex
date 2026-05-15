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

/// Platform / framework markers — fire on any status, identify the
/// runtime/CMS/ecommerce stack. Antibot vendor signatures moved to
/// `AntibotMarkerSource` in B5 — that source owns the
/// "what antibot vendor is this?" question now.
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

        // Platform / framework markers — fire on any status. Antibot
        // detection is `AntibotMarkerSource`'s job; this source only
        // recognises CMS / framework / ecommerce platforms.
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

    // Antibot detection tests live in `antibot_marker.rs` after B5.

    #[test]
    fn healthy_html_no_detections() {
        let h = html_headers();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><body><h1>healthy</h1><p>plenty of plain content here</p></body></html>";
        let dets = BodyMarkerSource::new().analyze(&ctx_with(&h, &u, 200, body));
        assert!(dets.is_empty(), "unexpected: {dets:?}");
    }
}
