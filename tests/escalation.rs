//! Tests for the anti-bot escalation heuristic. Fast and deterministic —
//! pure input→output, no network, no browser.
//!
//! Each case here is a signature we've observed in the wild. Regressions
//! here would silently break the `FetchMethod::Auto` fallback path.

use crawlex::escalation::should_escalate;
use http::{HeaderMap, HeaderValue};

fn html_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        "content-type",
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h
}

#[test]
fn cloudflare_challenge_503_escalates() {
    let body = b"<html><body><div id=\"cf-chl-bypass\"></div>Just a moment...</body></html>";
    assert!(should_escalate(503, &html_headers(), body));
}

#[test]
fn cloudflare_challenge_403_escalates() {
    let body = b"<html>attention required: cf-chl-bypass <script>/cdn-cgi/challenge-platform/</script></html>";
    assert!(should_escalate(403, &html_headers(), body));
}

#[test]
fn datadome_blocks_escalate() {
    let body = b"<html>DataDome detected automated traffic</html>";
    assert!(should_escalate(403, &html_headers(), body));
}

#[test]
fn perimeterx_blocks_escalate() {
    let body = b"<html>Access denied by PerimeterX</html>";
    assert!(should_escalate(403, &html_headers(), body));
}

#[test]
fn imperva_blocks_escalate() {
    let body = b"<html>_Incapsula_ - Imperva request blocked</html>";
    assert!(should_escalate(403, &html_headers(), body));
}

#[test]
fn distilnetworks_blocks_escalate() {
    let body = b"<html>distilnetworks automated request</html>";
    assert!(should_escalate(403, &html_headers(), body));
}

#[test]
fn small_html_with_window_location_escalates() {
    // Classic bot-landing stub: tiny HTML that redirects via JS.
    let body = b"<html><head></head><body><script>window.location='/real';</script></body></html>";
    assert!(should_escalate(200, &html_headers(), body));
}

#[test]
fn small_html_with_script_tag_escalates() {
    let body = b"<html><body><script>document.title='loading'</script></body></html>";
    assert!(should_escalate(200, &html_headers(), body));
}

#[test]
fn noscript_enable_js_escalates() {
    let body =
        b"<html><body><noscript>Please enable JavaScript to continue.</noscript></body></html>";
    assert!(should_escalate(200, &html_headers(), body));
}

#[test]
fn healthy_large_html_does_not_escalate() {
    // 4 KiB of real content — above the small-HTML cutoff (2 KiB).
    let filler = "<p>content content content content </p>".repeat(150);
    let body = format!("<html><body>{filler}</body></html>");
    assert!(!should_escalate(200, &html_headers(), body.as_bytes()));
}

#[test]
fn non_html_response_does_not_escalate_on_small_body() {
    // JSON API response with 200 + tiny body — NOT an HTML challenge.
    let mut h = HeaderMap::new();
    h.insert("content-type", HeaderValue::from_static("application/json"));
    let body = br#"{"ok":true}"#;
    assert!(!should_escalate(200, &h, body));
}

#[test]
fn vendor_signature_without_5xx_status_does_not_escalate() {
    // Body mentioning "DataDome" but server returned 200 — treat as content.
    let body = b"<html>article about DataDome bot detection company</html>";
    let filler = "<p>lorem ipsum dolor sit amet</p>".repeat(100);
    let full = format!("<html>{filler} DataDome </html>");
    assert!(!should_escalate(200, &html_headers(), full.as_bytes()));
    // But a minimal body with the signature + 200 stays non-escalating.
    assert!(!should_escalate(200, &html_headers(), body));
}
