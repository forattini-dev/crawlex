//! Unit tests for `crate::antibot` — DOM / HTTP / cookie detection paths.
//!
//! We read fixtures from `tests/antibot_fixtures/`; each vendor HTML must
//! match its expected vendor, and the innocent fixture must never trigger.

use crawlex::antibot::{
    detect_from_cookies, detect_from_html, detect_from_http_response, ChallengeLevel,
    ChallengeVendor,
};
use http::HeaderMap;
use std::fs;
use url::Url;

fn fixture(name: &str) -> String {
    let path = format!("tests/antibot_fixtures/{name}");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn url() -> Url {
    Url::parse("https://example.com/protected").unwrap()
}

#[test]
fn cloudflare_js_challenge_matches() {
    let html = fixture("cloudflare_jschallenge.html");
    let raw = detect_from_html(&html, &url(), None).expect("cf should match");
    assert_eq!(raw.vendor, ChallengeVendor::CloudflareJsChallenge);
    assert_eq!(raw.level, ChallengeLevel::ChallengePage);
}

#[test]
fn cloudflare_turnstile_matches() {
    let html = fixture("cloudflare_turnstile.html");
    let raw = detect_from_html(&html, &url(), None).expect("turnstile should match");
    assert_eq!(raw.vendor, ChallengeVendor::CloudflareTurnstile);
    assert_eq!(raw.level, ChallengeLevel::WidgetPresent);
    assert_eq!(
        raw.metadata.get("sitekey").and_then(|v| v.as_str()),
        Some("0x4AAAAAAAExampleSiteKey")
    );
    assert_eq!(
        raw.metadata.get("widget_present").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(raw
        .metadata
        .get("iframe_srcs")
        .and_then(|v| v.as_array())
        .is_some_and(|items| !items.is_empty()));
}

#[test]
fn recaptcha_enterprise_matches_before_plain() {
    let html = fixture("recaptcha_enterprise.html");
    let raw = detect_from_html(&html, &url(), None).expect("enterprise should match");
    assert_eq!(raw.vendor, ChallengeVendor::RecaptchaEnterprise);
}

#[test]
fn recaptcha_matches() {
    let html = fixture("recaptcha.html");
    let raw = detect_from_html(&html, &url(), None).expect("recaptcha should match");
    assert_eq!(raw.vendor, ChallengeVendor::Recaptcha);
    assert_eq!(
        raw.metadata.get("sitekey").and_then(|v| v.as_str()),
        Some("6LExampleKey")
    );
}

#[test]
fn hcaptcha_matches() {
    let html = fixture("hcaptcha.html");
    let raw = detect_from_html(&html, &url(), None).expect("hcaptcha should match");
    assert_eq!(raw.vendor, ChallengeVendor::HCaptcha);
    assert_eq!(
        raw.metadata.get("sitekey").and_then(|v| v.as_str()),
        Some("10000000-ffff-ffff-ffff-000000000001")
    );
}

#[test]
fn datadome_matches() {
    let html = fixture("datadome.html");
    let raw = detect_from_html(&html, &url(), None).expect("datadome should match");
    assert_eq!(raw.vendor, ChallengeVendor::DataDome);
    assert_eq!(raw.level, ChallengeLevel::ChallengePage);
}

#[test]
fn perimeterx_matches() {
    let html = fixture("perimeterx.html");
    let raw = detect_from_html(&html, &url(), None).expect("px should match");
    assert_eq!(raw.vendor, ChallengeVendor::PerimeterX);
}

#[test]
fn akamai_matches() {
    let html = fixture("akamai.html");
    let raw = detect_from_html(&html, &url(), None).expect("akamai should match");
    assert_eq!(raw.vendor, ChallengeVendor::Akamai);
}

#[test]
fn innocent_html_never_matches() {
    let html = fixture("innocent.html");
    assert!(
        detect_from_html(&html, &url(), None).is_none(),
        "innocent page must not trigger any vendor"
    );
}

#[test]
fn innocent_html_is_not_false_positive_for_http_403() {
    // The HTML mentions "Just a moment" and "Access denied" but has no
    // CF platform script and a large body — HTTP detector should bail.
    let html = fixture("innocent.html");
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "text/html".parse().unwrap());
    let raw = detect_from_http_response(403, html.as_bytes(), &headers, &url());
    // Could match AccessDenied via generic path if body is small; our
    // innocent fixture is > 512 bytes so no HardBlock fallback.
    if let Some(ref r) = raw {
        // If any match did occur, it must not be CloudflareJsChallenge
        // (that's the worst false-positive we guard against).
        assert_ne!(r.vendor, ChallengeVendor::CloudflareJsChallenge);
    }
}

#[test]
fn datadome_cookie_header() {
    let mut headers = HeaderMap::new();
    headers.append("set-cookie", "datadome=abcd; Path=/".parse().unwrap());
    let raw = detect_from_http_response(200, b"<html></html>", &headers, &url())
        .expect("datadome cookie should be Suspected");
    assert_eq!(raw.vendor, ChallengeVendor::DataDome);
    assert_eq!(raw.level, ChallengeLevel::Suspected);
}

#[test]
fn perimeterx_cookie_header() {
    let mut headers = HeaderMap::new();
    headers.append("set-cookie", "_px3=xyz; Path=/".parse().unwrap());
    let raw = detect_from_http_response(200, b"<html></html>", &headers, &url())
        .expect("_px3 cookie should be Suspected");
    assert_eq!(raw.vendor, ChallengeVendor::PerimeterX);
}

#[test]
fn akamai_ghost_403() {
    let mut headers = HeaderMap::new();
    headers.insert("server", "AkamaiGHost".parse().unwrap());
    let raw =
        detect_from_http_response(403, b"", &headers, &url()).expect("Akamai 403 should HardBlock");
    assert_eq!(raw.vendor, ChallengeVendor::Akamai);
    assert_eq!(raw.level, ChallengeLevel::HardBlock);
}

#[test]
fn akamai_ghost_200_is_clean() {
    // Legit Akamai-fronted sites must not trigger on 200.
    let mut headers = HeaderMap::new();
    headers.insert("server", "AkamaiGHost".parse().unwrap());
    assert!(detect_from_http_response(200, b"<html>hi</html>", &headers, &url()).is_none());
}

#[test]
fn cloudflare_503_with_body_matches() {
    let body = fixture("cloudflare_jschallenge.html");
    let mut headers = HeaderMap::new();
    headers.insert("server", "cloudflare".parse().unwrap());
    let raw = detect_from_http_response(503, body.as_bytes(), &headers, &url())
        .expect("CF 503 should match");
    assert_eq!(raw.vendor, ChallengeVendor::CloudflareJsChallenge);
}

#[test]
fn cookie_name_datadome() {
    let raw = detect_from_cookies(&["datadome"]).unwrap();
    assert_eq!(raw.vendor, ChallengeVendor::DataDome);
}

#[test]
fn cookie_name_unknown() {
    assert!(detect_from_cookies(&["session_id", "csrf"]).is_none());
}

#[test]
fn no_cross_vendor_false_positive() {
    // Each fixture must only match its own vendor — pairwise sanity.
    let cases: &[(&str, ChallengeVendor)] = &[
        (
            "cloudflare_jschallenge.html",
            ChallengeVendor::CloudflareJsChallenge,
        ),
        (
            "cloudflare_turnstile.html",
            ChallengeVendor::CloudflareTurnstile,
        ),
        (
            "recaptcha_enterprise.html",
            ChallengeVendor::RecaptchaEnterprise,
        ),
        ("recaptcha.html", ChallengeVendor::Recaptcha),
        ("hcaptcha.html", ChallengeVendor::HCaptcha),
        ("datadome.html", ChallengeVendor::DataDome),
        ("perimeterx.html", ChallengeVendor::PerimeterX),
        ("akamai.html", ChallengeVendor::Akamai),
    ];
    for (path, expected) in cases {
        let html = fixture(path);
        let raw =
            detect_from_html(&html, &url(), None).unwrap_or_else(|| panic!("{path} had no match"));
        assert_eq!(
            raw.vendor, *expected,
            "{path} matched wrong vendor: {:?}",
            raw.vendor
        );
    }
}
