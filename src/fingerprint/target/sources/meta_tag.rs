//! MetaTag source — Hot tier (B2).
//!
//! Parses `<meta name="..." content="...">` tags. The `generator`
//! meta is the highest-signal one — most CMS frameworks ship it. Uses
//! `scraper` (already a workspace dep) for HTML parsing.

use scraper::{Html, Selector};

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct MetaTagSource;

impl MetaTagSource {
    pub fn new() -> Self {
        Self
    }
}

/// (generator-substring lower, weight, category, vendor).
/// Substring match against the `content` attribute lowercased.
const GENERATOR_TABLE: &[(&str, u8, Category, Vendor)] = &[
    ("wordpress", 10, Category::Cms, Vendor::Wordpress),
    ("drupal", 10, Category::Cms, Vendor::Drupal),
    ("joomla", 10, Category::Cms, Vendor::Joomla),
    ("ghost", 8, Category::Cms, Vendor::Ghost),
    ("wagtail", 10, Category::Cms, Vendor::Wagtail),
    ("magento", 10, Category::Ecommerce, Vendor::Magento),
    ("shopify", 10, Category::Ecommerce, Vendor::Shopify),
    ("vtex", 10, Category::Ecommerce, Vendor::Vtex),
    ("bigcommerce", 10, Category::Ecommerce, Vendor::BigCommerce),
    ("woocommerce", 10, Category::Ecommerce, Vendor::WooCommerce),
    ("nuxt", 10, Category::Frontend, Vendor::Nuxt),
];

impl Source for MetaTagSource {
    fn name(&self) -> &'static str {
        "meta_tag"
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
        // Selector: `meta[name="generator"]` — robust for the canonical
        // case; supports any case via attribute selector below.
        let gen_sel = Selector::parse(r#"meta[name="generator"]"#).unwrap();
        let mut out: Vec<Detection> = Vec::new();
        for meta in doc.select(&gen_sel) {
            let Some(content) = meta.value().attr("content") else {
                continue;
            };
            let content_lower = content.to_ascii_lowercase();
            // Try to extract a version token like "WordPress 6.5".
            let version = content
                .split_whitespace()
                .skip_while(|w| !w.chars().any(|c| c.is_ascii_digit()))
                .next()
                .map(|s| s.to_string());
            for (needle, weight, cat, vendor) in GENERATOR_TABLE {
                if content_lower.contains(needle) {
                    let mut d = Detection::from_single(
                        *cat,
                        *vendor,
                        Evidence::new(
                            EvidenceSource::MetaTag,
                            format!("meta generator '{content}'"),
                            *weight,
                        ),
                    );
                    d.version = version.clone();
                    out.push(d);
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
    fn detects_wordpress_generator() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body =
            b"<html><head><meta name=\"generator\" content=\"WordPress 6.5\"></head></html>";
        let dets = MetaTagSource::new().analyze(&ctx_with(&h, &u, body));
        let wp = dets.iter().find(|d| d.vendor == Vendor::Wordpress).unwrap();
        assert_eq!(wp.version.as_deref(), Some("6.5"));
    }

    #[test]
    fn detects_drupal_generator() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><head><meta name=\"generator\" content=\"Drupal 10 (https://www.drupal.org)\"></head></html>";
        let dets = MetaTagSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Drupal));
    }

    #[test]
    fn detects_magento_generator() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><head><meta name=\"generator\" content=\"Magento Open Source 2.4.6\"></head></html>";
        let dets = MetaTagSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Magento));
    }

    #[test]
    fn no_detection_without_generator() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><head><meta charset=\"utf-8\"></head></html>";
        let dets = MetaTagSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.is_empty());
    }

    #[test]
    fn handles_non_utf8_gracefully() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        // Invalid UTF-8 bytes → source returns empty without panicking.
        let body: &[u8] = &[0xFF, 0xFE, 0x00, 0x00];
        let dets = MetaTagSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.is_empty());
    }
}
