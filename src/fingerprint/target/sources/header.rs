//! Header source — Hot tier (slice B1, tracer-bullet).
//!
//! Detects vendors via response header markers. Initial coverage:
//! Cloudflare, Akamai, Fastly, AWS CloudFront, Nginx, Apache, IIS,
//! plus Vercel / Netlify / Caddy. Designed to be the simplest possible
//! Source impl so the engine + types + report shapes get exercised
//! end-to-end before deeper sources land.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct HeaderSource;

impl HeaderSource {
    pub fn new() -> Self {
        Self
    }
}

/// (header-name-lower, weight, category, vendor, detail-fmt).
/// Header name compared case-insensitively. Weight 10 = exclusive
/// vendor header (only that vendor would ever send it). Weight 6-8 =
/// strong but not exclusive. Weight 4 = supporting evidence.
const HEADER_TABLE: &[(&str, u8, Category, Vendor, &str)] = &[
    // Cloudflare
    ("cf-ray", 10, Category::Cdn, Vendor::Cloudflare, "header cf-ray"),
    (
        "cf-cache-status",
        8,
        Category::Cdn,
        Vendor::Cloudflare,
        "header cf-cache-status",
    ),
    ("cf-connecting-ip", 8, Category::Cdn, Vendor::Cloudflare, "header cf-connecting-ip"),
    // Akamai
    ("x-akamai-transformed", 10, Category::Cdn, Vendor::Akamai, "header x-akamai-transformed"),
    ("x-akamai-request-id", 10, Category::Cdn, Vendor::Akamai, "header x-akamai-request-id"),
    ("akamai-grn", 10, Category::Cdn, Vendor::Akamai, "header akamai-grn"),
    // Fastly
    ("x-served-by", 6, Category::Cache, Vendor::Fastly, "header x-served-by"),
    ("x-fastly-request-id", 10, Category::Cdn, Vendor::Fastly, "header x-fastly-request-id"),
    // CloudFront
    ("x-amz-cf-id", 10, Category::Cdn, Vendor::CloudFront, "header x-amz-cf-id"),
    ("x-amz-cf-pop", 10, Category::Cdn, Vendor::CloudFront, "header x-amz-cf-pop"),
    // Vercel / Netlify
    ("x-vercel-cache", 10, Category::Cdn, Vendor::Vercel, "header x-vercel-cache"),
    ("x-vercel-id", 10, Category::Cdn, Vendor::Vercel, "header x-vercel-id"),
    ("x-nf-request-id", 10, Category::Cdn, Vendor::Netlify, "header x-nf-request-id"),
    // Bunny
    ("server: bunnycdn", 10, Category::Cdn, Vendor::Bunny, "server header BunnyCDN"),
    // Varnish (cache)
    ("via: varnish", 8, Category::Cache, Vendor::Varnish, "via header varnish"),
];

/// Server header value → vendor table. Compared case-insensitively
/// after `Server` value is lowercased.
const SERVER_VALUE_TABLE: &[(&str, u8, Category, Vendor)] = &[
    ("nginx", 8, Category::WebServer, Vendor::Nginx),
    ("apache", 8, Category::WebServer, Vendor::Apache),
    ("microsoft-iis", 10, Category::WebServer, Vendor::Iis),
    ("caddy", 10, Category::WebServer, Vendor::Caddy),
    ("litespeed", 10, Category::WebServer, Vendor::LiteSpeed),
    ("openresty", 10, Category::WebServer, Vendor::OpenResty),
    ("cloudflare", 10, Category::Cdn, Vendor::Cloudflare),
    ("akamaighost", 10, Category::Cdn, Vendor::Akamai),
    ("ats", 8, Category::Cache, Vendor::Generic),
    ("varnish", 10, Category::Cache, Vendor::Varnish),
    ("bunnycdn", 10, Category::Cdn, Vendor::Bunny),
];

impl Source for HeaderSource {
    fn name(&self) -> &'static str {
        "header"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let mut out: Vec<Detection> = Vec::new();

        // Pass 1: direct header presence matches.
        for (needle, weight, cat, vendor, detail) in HEADER_TABLE {
            if has_header_signal(ctx, needle) {
                out.push(Detection::from_single(
                    *cat,
                    *vendor,
                    Evidence::new(EvidenceSource::Header, *detail, *weight),
                ));
            }
        }

        // Pass 2: parse `Server` header value.
        if let Some(server) = ctx
            .headers
            .get("server")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_ascii_lowercase())
        {
            for (needle, weight, cat, vendor) in SERVER_VALUE_TABLE {
                if server.contains(needle) {
                    out.push(Detection::from_single(
                        *cat,
                        *vendor,
                        Evidence::new(
                            EvidenceSource::Header,
                            format!("server header contains '{needle}'"),
                            *weight,
                        ),
                    ));
                }
            }
        }

        out
    }
}

/// Header signal lookup. Supports two forms:
///   1. `"name"` — case-insensitive header presence.
///   2. `"name: value-substr"` — header value (lowercased) contains
///      the substring after the colon. Used for `via: varnish`,
///      `server: bunnycdn`, etc. without a dedicated table entry.
fn has_header_signal(ctx: &TargetContext<'_>, needle: &str) -> bool {
    if let Some((name, value_substr)) = needle.split_once(':') {
        let name = name.trim();
        let value_substr = value_substr.trim().to_ascii_lowercase();
        return ctx
            .headers
            .get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.to_ascii_lowercase().contains(&value_substr));
    }
    ctx.headers.get_all(needle).iter().next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::detection::Confidence;
    use http::HeaderMap;
    use url::Url;

    fn ctx_with(headers: Vec<(&str, &str)>) -> (HeaderMap, Url) {
        let mut h = HeaderMap::new();
        for (k, v) in headers {
            h.insert(
                http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        let u: Url = "https://example.com/".parse().unwrap();
        (h, u)
    }

    fn build_ctx<'a>(headers: &'a HeaderMap, url: &'a Url, body: &'a [u8]) -> TargetContext<'a> {
        TargetContext::http_only(url, 200, headers, body)
    }

    #[test]
    fn detects_cloudflare_via_cf_ray() {
        let (h, u) = ctx_with(vec![("cf-ray", "8a3abc-LAX")]);
        let ctx = build_ctx(&h, &u, b"");
        let dets = HeaderSource::new().analyze(&ctx);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare && d.confidence == Confidence::High));
    }

    #[test]
    fn detects_akamai_via_x_akamai_transformed() {
        let (h, u) = ctx_with(vec![("x-akamai-transformed", "9 4567 0 pmb=mTOE")]);
        let ctx = build_ctx(&h, &u, b"");
        let dets = HeaderSource::new().analyze(&ctx);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Akamai));
    }

    #[test]
    fn detects_fastly_via_x_fastly_request_id() {
        let (h, u) = ctx_with(vec![("x-fastly-request-id", "abc")]);
        let ctx = build_ctx(&h, &u, b"");
        let dets = HeaderSource::new().analyze(&ctx);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Fastly));
    }

    #[test]
    fn detects_nginx_via_server_value() {
        let (h, u) = ctx_with(vec![("server", "nginx/1.21.6")]);
        let ctx = build_ctx(&h, &u, b"");
        let dets = HeaderSource::new().analyze(&ctx);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Nginx));
    }

    #[test]
    fn detects_cloudfront_via_x_amz_cf_id() {
        let (h, u) = ctx_with(vec![("x-amz-cf-id", "Ag0o4uPrm...")]);
        let ctx = build_ctx(&h, &u, b"");
        let dets = HeaderSource::new().analyze(&ctx);
        assert!(dets.iter().any(|d| d.vendor == Vendor::CloudFront));
    }

    #[test]
    fn no_detections_on_bare_headers() {
        let (h, u) = ctx_with(vec![("content-type", "text/html")]);
        let ctx = build_ctx(&h, &u, b"");
        let dets = HeaderSource::new().analyze(&ctx);
        assert!(dets.is_empty(), "unexpected: {dets:?}");
    }

    #[test]
    fn case_insensitive_header_match() {
        // headers in HeaderMap are case-insensitive — assert engine
        // does not depend on caller normalising.
        let (h, u) = ctx_with(vec![("CF-Ray", "abc")]);
        let ctx = build_ctx(&h, &u, b"");
        let dets = HeaderSource::new().analyze(&ctx);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare));
    }
}
