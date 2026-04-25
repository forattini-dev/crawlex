//! TLS stealth fingerprint validation.
//!
//! Two layers:
//!   1. Pure unit tests on `stealth_assertions` — cheap, always run.
//!   2. Live-network test that actually hits `tls.peet.ws/api/clean` and
//!      asserts JA4 matches Chrome class. Marked `#[ignore]` so CI stays
//!      offline-friendly; run manually with `cargo test -- --ignored`.

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use url::Url;

use crawlex::cli::{stealth_assertions, StealthReport};
use crawlex::impersonate::{ImpersonateClient, Profile, Response};
use crawlex::metrics::NetworkTimings;

fn mock_response(alpn: Option<&str>, cipher: Option<&str>, body: &str) -> Response {
    Response {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        body: Bytes::from(body.to_string()),
        final_url: Url::parse("https://example.com/").unwrap(),
        alpn: alpn.map(|s| s.to_string()),
        tls_version: Some("TLSv1.3".into()),
        cipher: cipher.map(|s| s.to_string()),
        timings: NetworkTimings::default(),
        peer_cert: None,
        body_truncated: false,
    }
}

fn passes_all(report: &StealthReport) -> bool {
    report.checks.iter().all(|(ok, _)| *ok)
}

#[test]
fn alpn_h2_and_aead_cipher_pass() {
    let r = mock_response(Some("h2"), Some("TLS_AES_128_GCM_SHA256"), "");
    let report = stealth_assertions(&r, "");
    assert!(
        passes_all(&report),
        "expected all pass, got {:?}",
        report.checks
    );
}

#[test]
fn alpn_http1_fails() {
    let r = mock_response(Some("http/1.1"), Some("TLS_AES_128_GCM_SHA256"), "");
    let report = stealth_assertions(&r, "");
    assert!(!passes_all(&report));
    assert!(report
        .checks
        .iter()
        .any(|(ok, l)| !ok && l.contains("ALPN")));
}

#[test]
fn sha1_cipher_fails() {
    // Legacy cipher that we explicitly removed from the Chrome list;
    // if it ever appears we regressed.
    let r = mock_response(Some("h2"), Some("ECDHE-RSA-AES128-SHA"), "");
    let report = stealth_assertions(&r, "");
    assert!(!passes_all(&report));
    assert!(report
        .checks
        .iter()
        .any(|(ok, l)| !ok && l.contains("SHA1")));
}

#[test]
fn aead_gcm_suite_with_sha256_in_name_is_not_flagged() {
    // Guard against a false positive: `TLS_AES_128_GCM_SHA256` ends in
    // `SHA256`, not `-SHA`, so the SHA1 regex must not trip.
    let r = mock_response(Some("h2"), Some("TLS_AES_128_GCM_SHA256"), "");
    let report = stealth_assertions(&r, "");
    assert!(passes_all(&report));
}

#[test]
fn ja4_t13d_prefix_pass() {
    let body = r#"{"ja4":"t13d1715h2_5b57614c22b0_3d5424432f57","other":"x"}"#;
    let r = mock_response(Some("h2"), Some("TLS_AES_128_GCM_SHA256"), body);
    let report = stealth_assertions(&r, body);
    assert!(passes_all(&report));
    assert!(report
        .checks
        .iter()
        .any(|(_, l)| l.contains("JA4") && l.contains("t13d")));
}

#[test]
fn ja4_non_t13d_fails() {
    // A `t12d...` prefix would mean we negotiated TLS 1.2 or fell back;
    // fail loud so we catch accidental downgrades.
    let body = r#"{"ja4":"t12d1715h2_abc_def"}"#;
    let r = mock_response(Some("h2"), Some("TLS_AES_128_GCM_SHA256"), body);
    let report = stealth_assertions(&r, body);
    assert!(!passes_all(&report));
}

#[test]
fn body_without_ja4_field_skips_that_check() {
    // When the endpoint doesn't include a JA4 field, we shouldn't fail
    // — the other checks still run and decide.
    let r = mock_response(Some("h2"), Some("TLS_AES_128_GCM_SHA256"), "");
    let report = stealth_assertions(&r, "{}");
    // 3 checks present (TLS summary + ALPN + cipher), no JA4.
    assert_eq!(report.checks.len(), 3);
    assert!(passes_all(&report));
}

// ---------- Network-only smoke test ----------

#[tokio::test]
#[ignore] // needs outbound network; run with `cargo test -- --ignored`
async fn live_peet_ja4_starts_with_t13d() {
    let client = ImpersonateClient::new(Profile::Chrome131Stable).unwrap();
    let url = Url::parse("https://tls.peet.ws/api/clean").unwrap();
    let r = client.get(&url).await.expect("peet reachable");
    let body = String::from_utf8_lossy(&r.body);
    let report = stealth_assertions(&r, &body);
    for (ok, label) in &report.checks {
        println!("{} {label}", if *ok { "PASS" } else { "FAIL" });
    }
    assert!(passes_all(&report), "stealth assertions failed on peet.ws");
}
