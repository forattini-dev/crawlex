//! CookieName source — Hot tier (B2).
//!
//! Reads `Set-Cookie` header values and matches cookie names against
//! a table of well-known antibot / CDN session cookies. A session that
//! picks up `__cf_bm` immediately surfaces "this host gates traffic
//! with Cloudflare Bot Management" even if no challenge fired.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct CookieSource;

impl CookieSource {
    pub fn new() -> Self {
        Self
    }
}

/// (cookie-name-prefix, weight, category, vendor, detail).
/// Cookie names compared by prefix (case-sensitive — cookie names are
/// case-sensitive per RFC 6265 spec, and vendor names are stable).
const COOKIE_TABLE: &[(&str, u8, Category, Vendor, &str)] = &[
    // Cloudflare
    ("__cf_bm", 10, Category::Antibot, Vendor::CloudflareBotManagement, "cookie __cf_bm"),
    ("__cfruid", 8, Category::Cdn, Vendor::Cloudflare, "cookie __cfruid"),
    ("__cflb", 8, Category::Cdn, Vendor::Cloudflare, "cookie __cflb"),
    ("cf_clearance", 10, Category::Antibot, Vendor::Cloudflare, "cookie cf_clearance"),
    ("__cf_chl", 10, Category::Antibot, Vendor::Cloudflare, "cookie __cf_chl_*"),
    // DataDome
    ("_dd_s", 10, Category::Antibot, Vendor::DataDome, "cookie _dd_s"),
    ("datadome", 10, Category::Antibot, Vendor::DataDome, "cookie datadome"),
    // PerimeterX
    ("_px", 10, Category::Antibot, Vendor::PerimeterX, "cookie _px*"),
    ("_pxhd", 10, Category::Antibot, Vendor::PerimeterX, "cookie _pxhd"),
    ("_pxff_", 8, Category::Antibot, Vendor::PerimeterX, "cookie _pxff_*"),
    // Imperva / Incapsula
    ("incap_ses_", 10, Category::Antibot, Vendor::Imperva, "cookie incap_ses_*"),
    ("visid_incap_", 10, Category::Antibot, Vendor::Imperva, "cookie visid_incap_*"),
    ("nlbi_", 6, Category::Antibot, Vendor::Imperva, "cookie nlbi_*"),
    // Akamai
    ("BMSC", 10, Category::Antibot, Vendor::AkamaiBotManager, "cookie BMSC"),
    ("ak_bmsc", 10, Category::Antibot, Vendor::AkamaiBotManager, "cookie ak_bmsc"),
    ("bm_sv", 10, Category::Antibot, Vendor::AkamaiBotManager, "cookie bm_sv"),
    ("bm_sz", 10, Category::Antibot, Vendor::AkamaiBotManager, "cookie bm_sz"),
    // Shape Security / F5
    ("TS01", 6, Category::Antibot, Vendor::ShapeSecurity, "cookie TS01*"),
    // F5 BIG-IP
    ("BIGipServer", 8, Category::ReverseProxyLb, Vendor::F5BigIp, "cookie BIGipServer*"),
    // Magento / ecommerce sessions (cookie patterns)
    ("PHPSESSID", 4, Category::Backend, Vendor::Php, "cookie PHPSESSID"),
    // Shopify
    ("_shopify_y", 8, Category::Ecommerce, Vendor::Shopify, "cookie _shopify_y"),
    ("_shopify_s", 8, Category::Ecommerce, Vendor::Shopify, "cookie _shopify_s"),
    // VTEX
    ("VtexFingerPrint", 10, Category::Ecommerce, Vendor::Vtex, "cookie VtexFingerPrint"),
    ("VtexWorkspace", 10, Category::Ecommerce, Vendor::Vtex, "cookie VtexWorkspace"),
    // WordPress
    ("wordpress_logged_in_", 8, Category::Cms, Vendor::Wordpress, "cookie wordpress_logged_in_*"),
    ("wp-settings-", 6, Category::Cms, Vendor::Wordpress, "cookie wp-settings-*"),
];

impl Source for CookieSource {
    fn name(&self) -> &'static str {
        "cookie"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let mut out: Vec<Detection> = Vec::new();
        let cookies = ctx.headers.get_all("set-cookie");
        for cookie_value in cookies.iter().filter_map(|v| v.to_str().ok()) {
            let name = cookie_value
                .split(';')
                .next()
                .and_then(|kv| kv.split('=').next())
                .map(|s| s.trim())
                .unwrap_or("");
            if name.is_empty() {
                continue;
            }
            for (needle, weight, cat, vendor, detail) in COOKIE_TABLE {
                if name.starts_with(needle) {
                    out.push(Detection::from_single(
                        *cat,
                        *vendor,
                        Evidence::new(EvidenceSource::CookieName, *detail, *weight),
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

    fn build_ctx<'a>(headers: &'a HeaderMap, url: &'a Url) -> TargetContext<'a> {
        TargetContext::http_only(url, 200, headers, b"")
    }

    fn headers_with(cookies: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for c in cookies {
            h.append("set-cookie", c.parse().unwrap());
        }
        h
    }

    #[test]
    fn detects_cf_bm() {
        let h = headers_with(&["__cf_bm=xxx; Path=/; HttpOnly"]);
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = CookieSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets
            .iter()
            .any(|d| d.vendor == Vendor::CloudflareBotManagement && d.category == Category::Antibot));
    }

    #[test]
    fn detects_datadome() {
        let h = headers_with(&["_dd_s=abc; Path=/"]);
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = CookieSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.iter().any(|d| d.vendor == Vendor::DataDome));
    }

    #[test]
    fn detects_perimeterx_prefix() {
        let h = headers_with(&["_pxhd=xxx", "_px_session=yyy"]);
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = CookieSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.iter().any(|d| d.vendor == Vendor::PerimeterX));
    }

    #[test]
    fn detects_imperva_incap_prefix() {
        let h = headers_with(&["incap_ses_42_999=xxx; Path=/"]);
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = CookieSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Imperva));
    }

    #[test]
    fn detects_akamai_bot_manager() {
        let h = headers_with(&["ak_bmsc=ABC123; Path=/"]);
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = CookieSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.iter().any(|d| d.vendor == Vendor::AkamaiBotManager));
    }

    #[test]
    fn detects_vtex_ecommerce() {
        let h = headers_with(&["VtexFingerPrint=xxx; Path=/"]);
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = CookieSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Vtex));
    }

    #[test]
    fn no_detection_on_innocent_cookie() {
        let h = headers_with(&["session_id=xxx", "csrf=yyy"]);
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = CookieSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.is_empty(), "unexpected detections: {dets:?}");
    }
}
