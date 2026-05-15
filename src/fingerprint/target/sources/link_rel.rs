//! LinkRel source — Hot tier (B3).
//!
//! Reads `<link rel="...">` tags. Preconnect / dns-prefetch hints to
//! well-known CDN / analytics / payment hosts produce supporting
//! evidence (lower weight than ScriptSrc — a preconnect proves the
//! site *plans* to use the host, not necessarily that it does).
//! Manifest links are evidence the host ships a PWA.

use scraper::{Html, Selector};

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct LinkRelSource;

impl LinkRelSource {
    pub fn new() -> Self {
        Self
    }
}

/// (host-substring lower, weight, category, vendor).
/// Used for preconnect / dns-prefetch href values.
const HOST_TABLE: &[(&str, u8, Category, Vendor)] = &[
    ("googletagmanager.com", 6, Category::TagManager, Vendor::Gtm),
    ("google-analytics.com", 6, Category::Analytics, Vendor::GoogleAnalytics),
    ("cdn.shopify.com", 6, Category::Ecommerce, Vendor::Shopify),
    ("cloudflare.com", 4, Category::Cdn, Vendor::Cloudflare),
    ("akamaihd.net", 4, Category::Cdn, Vendor::Akamai),
    ("akamaized.net", 4, Category::Cdn, Vendor::Akamai),
    ("cloudfront.net", 4, Category::Cdn, Vendor::CloudFront),
    ("vercel-storage.com", 6, Category::Cdn, Vendor::Vercel),
    ("js.stripe.com", 6, Category::Payment, Vendor::Stripe),
    ("paypalobjects.com", 6, Category::Payment, Vendor::Paypal),
];

impl Source for LinkRelSource {
    fn name(&self) -> &'static str {
        "link_rel"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let body_str = match std::str::from_utf8(ctx.body) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let doc = Html::parse_document(body_str);
        let sel = Selector::parse("link[rel][href]").unwrap();

        let mut seen: std::collections::HashSet<(Category, Vendor)> = std::collections::HashSet::new();
        let mut out: Vec<Detection> = Vec::new();

        for link in doc.select(&sel) {
            let Some(rel) = link.value().attr("rel") else {
                continue;
            };
            let rel_lower = rel.to_ascii_lowercase();
            // Only care about preconnect / dns-prefetch / preload here.
            if !rel_lower.contains("preconnect")
                && !rel_lower.contains("dns-prefetch")
                && !rel_lower.contains("preload")
            {
                continue;
            }
            let Some(href) = link.value().attr("href") else {
                continue;
            };
            let lower = href.to_ascii_lowercase();
            for (needle, weight, cat, vendor) in HOST_TABLE {
                if lower.contains(needle) && seen.insert((*cat, *vendor)) {
                    out.push(Detection::from_single(
                        *cat,
                        *vendor,
                        Evidence::new(
                            EvidenceSource::LinkRel,
                            format!("link rel={rel_lower} href '{href}'"),
                            *weight,
                        ),
                    ));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use url::Url;

    fn ctx_with<'a>(headers: &'a HeaderMap, url: &'a Url, body: &'a [u8]) -> TargetContext<'a> {
        TargetContext::http_only(url, 200, headers, body)
    }

    #[test]
    fn detects_preconnect_to_gtm() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><head><link rel="preconnect" href="https://www.googletagmanager.com"></head></html>"#;
        let dets = LinkRelSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Gtm));
    }

    #[test]
    fn detects_dns_prefetch_cloudflare() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><head><link rel="dns-prefetch" href="//cdnjs.cloudflare.com"></head></html>"#;
        let dets = LinkRelSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare));
    }

    #[test]
    fn no_detection_for_stylesheet_link() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><head><link rel="stylesheet" href="https://www.googletagmanager.com/x.css"></head></html>"#;
        let dets = LinkRelSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.is_empty());
    }
}
