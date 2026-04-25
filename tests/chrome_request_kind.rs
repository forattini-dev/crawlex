//! Unit tests for the per-request-kind header-order contract.
//!
//! Chrome emits distinct header orders for different request types (document
//! navigation vs XHR/fetch vs script/style/image/font). `ChromeRequestKind`
//! codifies that contract; these tests lock it down.

use crawlex::impersonate::headers::ChromeRequestKind;

#[test]
fn document_order_is_chrome_m144_observed() {
    let expected: &[&str] = &[
        "sec-ch-ua",
        "sec-ch-ua-mobile",
        "sec-ch-ua-platform",
        "upgrade-insecure-requests",
        "user-agent",
        "accept",
        "sec-fetch-site",
        "sec-fetch-mode",
        "sec-fetch-user",
        "sec-fetch-dest",
        "accept-encoding",
        "accept-language",
        "cookie",
    ];
    assert_eq!(ChromeRequestKind::Document.header_order(), expected);
}

#[test]
fn xhr_order_drops_uir_and_sec_fetch_user() {
    let xhr = ChromeRequestKind::Xhr.header_order();
    assert!(!xhr.contains(&"upgrade-insecure-requests"));
    assert!(!xhr.contains(&"sec-fetch-user"));
    assert!(xhr.contains(&"origin"));
    // cookie must be last for every kind — detectors key on that.
    assert_eq!(xhr.last().copied(), Some("cookie"));
}

#[test]
fn fetch_order_matches_xhr() {
    assert_eq!(
        ChromeRequestKind::Xhr.header_order(),
        ChromeRequestKind::Fetch.header_order()
    );
}

#[test]
fn script_has_referer_and_sec_fetch_dest_script() {
    let order = ChromeRequestKind::Script.header_order();
    assert!(order.contains(&"referer"));
    assert!(order.contains(&"sec-fetch-dest"));
    assert_eq!(ChromeRequestKind::Script.sec_fetch_dest(), "script");
    assert_eq!(ChromeRequestKind::Script.default_accept(), "*/*");
}

#[test]
fn style_accept_is_chrome_specific() {
    assert_eq!(
        ChromeRequestKind::Style.default_accept(),
        "text/css,*/*;q=0.1"
    );
    assert_eq!(ChromeRequestKind::Style.sec_fetch_dest(), "style");
}

#[test]
fn image_accept_lists_modern_formats_first() {
    let accept = ChromeRequestKind::Image.default_accept();
    // avif/webp must precede the generic image/* wildcard — Chrome's
    // content-negotiation preference.
    let avif_idx = accept.find("image/avif").unwrap();
    let webp_idx = accept.find("image/webp").unwrap();
    let wildcard_idx = accept.find("image/*").unwrap();
    assert!(avif_idx < wildcard_idx);
    assert!(webp_idx < wildcard_idx);
}

#[test]
fn font_mode_is_cors() {
    assert_eq!(ChromeRequestKind::Font.sec_fetch_mode(), "cors");
    assert_eq!(ChromeRequestKind::Font.sec_fetch_dest(), "font");
    // Font fetches carry origin even same-origin (Chrome quirk).
    assert!(ChromeRequestKind::Font.header_order().contains(&"origin"));
}

#[test]
fn ping_order_is_minimal_but_contains_ping_from_to() {
    let order = ChromeRequestKind::Ping.header_order();
    assert!(order.contains(&"ping-from"));
    assert!(order.contains(&"ping-to"));
    assert_eq!(ChromeRequestKind::Ping.sec_fetch_dest(), "empty");
    assert_eq!(ChromeRequestKind::Ping.sec_fetch_mode(), "no-cors");
}

#[test]
fn sec_fetch_user_only_on_document() {
    for kind in [
        ChromeRequestKind::Xhr,
        ChromeRequestKind::Fetch,
        ChromeRequestKind::Script,
        ChromeRequestKind::Style,
        ChromeRequestKind::Image,
        ChromeRequestKind::Font,
        ChromeRequestKind::Ping,
    ] {
        assert!(
            !kind.includes_sec_fetch_user(),
            "{kind:?} must NOT include sec-fetch-user"
        );
    }
    assert!(ChromeRequestKind::Document.includes_sec_fetch_user());
}

#[test]
fn upgrade_insecure_only_on_document() {
    assert!(ChromeRequestKind::Document.includes_upgrade_insecure_requests());
    for kind in [
        ChromeRequestKind::Xhr,
        ChromeRequestKind::Fetch,
        ChromeRequestKind::Script,
        ChromeRequestKind::Style,
        ChromeRequestKind::Image,
        ChromeRequestKind::Font,
        ChromeRequestKind::Ping,
    ] {
        assert!(
            !kind.includes_upgrade_insecure_requests(),
            "{kind:?} must NOT include upgrade-insecure-requests"
        );
    }
}

#[test]
fn header_order_never_contains_duplicates() {
    for kind in [
        ChromeRequestKind::Document,
        ChromeRequestKind::Xhr,
        ChromeRequestKind::Fetch,
        ChromeRequestKind::Script,
        ChromeRequestKind::Style,
        ChromeRequestKind::Image,
        ChromeRequestKind::Font,
        ChromeRequestKind::Ping,
    ] {
        let order = kind.header_order();
        let mut sorted: Vec<&str> = order.to_vec();
        sorted.sort();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "{kind:?} has duplicate headers");
    }
}
