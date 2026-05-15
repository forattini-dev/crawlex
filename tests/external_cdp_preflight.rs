//! Slice 30 — external CDP preflight contract.
//!
//! These tests stand up a minimal `/json/version` mock that mimics the
//! shape of a Chromium CDP host (or, in the negative cases, doesn't)
//! and verifies that `cdp_probe::probe` distinguishes:
//!
//!   * reachable + compatible endpoints (returns `webSocketDebuggerUrl`),
//!   * reachable but incompatible endpoints (HTTP 200 with non-CDP JSON
//!     or missing `webSocketDebuggerUrl`),
//!   * reachable but failing endpoints (non-2xx HTTP status),
//!   * unreachable endpoints (connect-level failure).
//!
//! Together these cover the slice's
//! "Unreachable, invalid, or incompatible CDP endpoints produce
//! actionable errors before target work continues" criterion.

#![cfg(feature = "cdp-backend")]

use crawlex::render::cdp_probe::probe;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn probe_returns_ws_url_for_chromium_compatible_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Browser": "Chrome/149.0.0.0",
            "Protocol-Version": "1.3",
            "User-Agent": "Mozilla/5.0",
            "V8-Version": "12.0",
            "WebKit-Version": "537.36",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/deadbeef"
        })))
        .mount(&server)
        .await;

    let ok = probe(&server.uri()).await.expect("compatible endpoint");
    assert_eq!(
        ok.web_socket_debugger_url,
        "ws://127.0.0.1:9222/devtools/browser/deadbeef"
    );
    assert!(ok.browser.contains("Chrome"));
}

#[tokio::test]
async fn probe_rewrites_ws_scheme_to_http_for_version_lookup() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Browser": "Chrome",
            "webSocketDebuggerUrl": "ws://x/y"
        })))
        .mount(&server)
        .await;

    // Operator pasted a ws:// debugger URL — probe must rewrite to http://
    // against the same authority instead of erroring out on the scheme.
    let host = server.address();
    let ws_endpoint = format!("ws://{host}/devtools/browser/abc");
    let ok = probe(&ws_endpoint).await.expect("ws scheme accepted");
    assert_eq!(ok.web_socket_debugger_url, "ws://x/y");
}

#[tokio::test]
async fn probe_reports_actionable_error_on_non_2xx() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let err = probe(&server.uri()).await.unwrap_err();
    assert!(err.contains("HTTP 500"), "got: {err}");
    assert!(
        err.to_lowercase().contains("devtools"),
        "error should hint at DevTools exposure, got: {err}"
    );
}

#[tokio::test]
async fn probe_rejects_endpoint_without_websocket_debugger_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"Browser": "Other/1.0"})),
        )
        .mount(&server)
        .await;

    let err = probe(&server.uri()).await.unwrap_err();
    assert!(
        err.contains("webSocketDebuggerUrl"),
        "error should call out the missing field, got: {err}"
    );
    assert!(
        err.contains("incompatible"),
        "error should flag the endpoint as incompatible, got: {err}"
    );
}

#[tokio::test]
async fn probe_rejects_non_json_body_as_incompatible() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json at all"))
        .mount(&server)
        .await;

    let err = probe(&server.uri()).await.unwrap_err();
    assert!(
        err.contains("non-CDP response") || err.contains("incompatible"),
        "error should flag non-CDP response, got: {err}"
    );
}

#[tokio::test]
async fn probe_unreachable_endpoint_surfaces_connect_failure() {
    // Port 1 has no listener on virtually every developer host.
    let err = probe("http://127.0.0.1:1").await.unwrap_err();
    assert!(err.contains("unreachable"), "got: {err}");
}

#[tokio::test]
async fn probe_rejects_invalid_scheme() {
    let err = probe("file:///tmp/x").await.unwrap_err();
    assert!(
        err.contains("not a valid endpoint") || err.contains("scheme"),
        "got: {err}"
    );
}
