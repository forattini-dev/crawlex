//! ScriptSrc source — Hot tier (B3).
//!
//! Extracts `<script src="...">` URLs and matches the src host against
//! a table of well-known third-party CDN domains. Detects analytics,
//! tag managers, A/B testing, and chat widgets.

use scraper::{Html, Selector};

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct ScriptSrcSource;

impl ScriptSrcSource {
    pub fn new() -> Self {
        Self
    }
}

/// (host-substring lower, weight, category, vendor).
/// Substring match against the script src URL lowercased.
const HOST_TABLE: &[(&str, u8, Category, Vendor)] = &[
    // Tag managers
    ("googletagmanager.com", 10, Category::TagManager, Vendor::Gtm),
    ("assets.adobedtm.com", 10, Category::TagManager, Vendor::AdobeLaunch),
    ("tags.tiqcdn.com", 10, Category::TagManager, Vendor::Tealium),
    // Analytics
    ("google-analytics.com", 10, Category::Analytics, Vendor::GoogleAnalytics),
    ("googletagmanager.com/gtag", 8, Category::Analytics, Vendor::GoogleAnalytics),
    ("adobedc.net", 10, Category::Analytics, Vendor::AdobeAnalytics),
    ("cdn.segment.com", 10, Category::Analytics, Vendor::Segment),
    ("cdn.mxpnl.com", 10, Category::Analytics, Vendor::Mixpanel),
    ("static.hotjar.com", 10, Category::Analytics, Vendor::Hotjar),
    ("plausible.io", 10, Category::Analytics, Vendor::Plausible),
    // A/B testing
    ("cdn.optimizely.com", 10, Category::AbTesting, Vendor::Optimizely),
    ("dev.visualwebsiteoptimizer.com", 10, Category::AbTesting, Vendor::Vwo),
    ("googleoptimize.com", 10, Category::AbTesting, Vendor::GoogleOptimize),
    // Chat / support
    ("widget.intercom.io", 10, Category::Chat, Vendor::Intercom),
    ("static.intercomcdn.com", 10, Category::Chat, Vendor::Intercom),
    ("static.zdassets.com", 10, Category::Chat, Vendor::Zendesk),
    ("js.driftt.com", 10, Category::Chat, Vendor::Drift),
    ("code.jivosite.com", 10, Category::Chat, Vendor::JivoChat),
    // Auth
    ("cdn.auth0.com", 10, Category::Auth, Vendor::Auth0),
    // Payment widgets
    ("js.stripe.com", 10, Category::Payment, Vendor::Stripe),
    ("checkoutshopper-live.adyen.com", 10, Category::Payment, Vendor::Adyen),
    ("paypalobjects.com", 10, Category::Payment, Vendor::Paypal),
];

impl Source for ScriptSrcSource {
    fn name(&self) -> &'static str {
        "script_src"
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
        let sel = Selector::parse("script[src]").unwrap();

        let mut seen: std::collections::HashSet<(Category, Vendor)> = std::collections::HashSet::new();
        let mut out: Vec<Detection> = Vec::new();

        for script in doc.select(&sel) {
            let Some(src) = script.value().attr("src") else {
                continue;
            };
            let lower = src.to_ascii_lowercase();
            for (needle, weight, cat, vendor) in HOST_TABLE {
                if lower.contains(needle) && seen.insert((*cat, *vendor)) {
                    out.push(Detection::from_single(
                        *cat,
                        *vendor,
                        Evidence::new(
                            EvidenceSource::ScriptSrc,
                            format!("script src '{src}'"),
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
    fn detects_gtm() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><script src="https://www.googletagmanager.com/gtm.js?id=GTM-X"></script></html>"#;
        let dets = ScriptSrcSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Gtm));
    }

    #[test]
    fn detects_segment_mixpanel_hotjar() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html>
            <script src="https://cdn.segment.com/x.js"></script>
            <script src="https://cdn.mxpnl.com/x.js"></script>
            <script src="https://static.hotjar.com/c/x.js"></script>
        </html>"#;
        let dets = ScriptSrcSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Segment));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Mixpanel));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Hotjar));
    }

    #[test]
    fn detects_intercom_chat() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><script src="https://widget.intercom.io/widget/x"></script></html>"#;
        let dets = ScriptSrcSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Intercom));
    }

    #[test]
    fn detects_stripe_payment() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><script src="https://js.stripe.com/v3/"></script></html>"#;
        let dets = ScriptSrcSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Stripe));
    }

    #[test]
    fn duplicates_deduped_by_vendor_category() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html>
            <script src="https://www.googletagmanager.com/gtm.js?id=X"></script>
            <script src="https://www.googletagmanager.com/gtm.js?id=Y"></script>
        </html>"#;
        let dets = ScriptSrcSource::new().analyze(&ctx_with(&h, &u, body));
        let gtm_count = dets.iter().filter(|d| d.vendor == Vendor::Gtm).count();
        assert_eq!(gtm_count, 1, "GTM should not duplicate; got {dets:?}");
    }

    #[test]
    fn no_detection_on_innocent_scripts() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let body = br#"<html><script src="/static/app.js"></script></html>"#;
        let dets = ScriptSrcSource::new().analyze(&ctx_with(&h, &u, body));
        assert!(dets.is_empty());
    }
}
