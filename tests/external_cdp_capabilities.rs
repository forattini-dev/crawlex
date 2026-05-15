//! Slice 31 — endpoint capability detection contract.
//!
//! These tests stand up two flavours of `/json/version` mock and check
//! that detection cleanly separates generic Chromium DevTools from a
//! native-stealth multiplexer (cloakserve-like). The capability layer
//! is what gates whether crawlex appends identity query parameters to
//! the CDP connection URL.

#![cfg(feature = "cdp-backend")]

use crawlex::render::cdp_capabilities::{EndpointCapabilities, EndpointKind, IdentityHints};
use crawlex::render::cdp_probe::probe;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn generic_chromium_endpoint_yields_generic_capability() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Browser": "Chrome/149.0.0.0",
            "Protocol-Version": "1.3",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/abc"
        })))
        .mount(&server)
        .await;

    let probed = probe(&server.uri()).await.expect("compatible endpoint");
    assert!(probed.stealth_provider.is_empty());

    let caps = EndpointCapabilities::detect(&probed);
    assert_eq!(caps.kind, EndpointKind::GenericCdp);
    assert!(!caps.identity_params);

    // Generic endpoints must not get identity params even when hints
    // are present — crawlex must not assume the host can parse them.
    let hints = IdentityHints {
        timezone: Some("Europe/Lisbon"),
        locale: Some("pt-PT"),
        ..IdentityHints::default()
    };
    let url = caps.build_connect_url(&server.uri(), &hints).unwrap();
    assert_eq!(url, server.uri());
}

#[tokio::test]
async fn cloakserve_browser_banner_yields_native_stealth_capability() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Browser": "cloakserve/0.4.2 (chrome 149)",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/xyz"
        })))
        .mount(&server)
        .await;

    let probed = probe(&server.uri()).await.expect("compatible endpoint");
    let caps = EndpointCapabilities::detect(&probed);
    assert_eq!(caps.kind, EndpointKind::NativeStealth);
    assert!(caps.identity_params);

    let hints = IdentityHints {
        seed: Some("session-7"),
        timezone: Some("Europe/Lisbon"),
        locale: Some("pt-PT"),
        proxy: Some("http://user:pass@proxy.example:3128"),
        geoip: Some("PT"),
    };
    let url = caps.build_connect_url(&server.uri(), &hints).unwrap();
    let parsed = url::Url::parse(&url).unwrap();
    let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(q.get("seed").map(String::as_str), Some("session-7"));
    assert_eq!(q.get("timezone").map(String::as_str), Some("Europe/Lisbon"));
    assert_eq!(q.get("locale").map(String::as_str), Some("pt-PT"));
    assert_eq!(
        q.get("proxy").map(String::as_str),
        Some("http://user:pass@proxy.example:3128"),
    );
    assert_eq!(q.get("geoip").map(String::as_str), Some("PT"));
}

#[tokio::test]
async fn explicit_stealth_provider_field_yields_native_stealth() {
    // Some multiplexers may keep `Browser` set to the upstream Chrome
    // banner and instead advertise themselves via a custom field. The
    // capability layer must pick that up too.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/json/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Browser": "Chrome/149.0.0.0",
            "Stealth-Provider": "stealth-cdp/1.0",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/x"
        })))
        .mount(&server)
        .await;

    let probed = probe(&server.uri()).await.expect("compatible endpoint");
    assert_eq!(probed.stealth_provider, "stealth-cdp/1.0");

    let caps = EndpointCapabilities::detect(&probed);
    assert_eq!(caps.kind, EndpointKind::NativeStealth);
    assert_eq!(caps.vendor.as_deref(), Some("stealth-cdp/1.0"));
}
