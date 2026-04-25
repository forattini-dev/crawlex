//! Unit smoke test for the SPA observer JS bundle. Doesn't need
//! Chrome — we just make sure the emitted source is well-formed
//! enough to be injected (balanced braces, contains every named
//! global / wrapper we rely on) and that the `serde` types parse the
//! JS-shape payload the observer produces.
//!
//! Guards against typos in `src/render/spa_observer.rs` without
//! having to spin up a browser.

#![cfg(feature = "cdp-backend")]

use crawlex::render::spa_observer::{
    collect_expression, observer_js, CollectedObservations, OBSERVER_SAMPLE_CAP,
};

#[test]
fn observer_js_mentions_every_wrapper() {
    let js = observer_js();
    for needle in [
        "__crawlex_observer_installed__",
        "__crawlex_runtime_routes__",
        "__crawlex_network_endpoints__",
        "history.pushState",
        "history.replaceState",
        "popstate",
        "hashchange",
        "window.fetch",
        "XMLHttpRequest",
    ] {
        assert!(
            js.contains(needle),
            "observer_js() missing expected token `{needle}`; got:\n{js}"
        );
    }
    // Cap value must surface in the rendered source so runtime
    // changes to OBSERVER_SAMPLE_CAP stay wired.
    assert!(js.contains(&OBSERVER_SAMPLE_CAP.to_string()));
}

#[test]
fn observer_js_has_balanced_braces_and_parens() {
    let js = observer_js();
    let opens = js.chars().filter(|c| *c == '{').count();
    let closes = js.chars().filter(|c| *c == '}').count();
    assert_eq!(opens, closes, "unbalanced braces in observer js");
    let p_open = js.chars().filter(|c| *c == '(').count();
    let p_close = js.chars().filter(|c| *c == ')').count();
    assert_eq!(p_open, p_close, "unbalanced parens in observer js");
}

#[test]
fn collect_expression_shape_parses() {
    // Mimic what `Runtime.evaluate` would return via return_by_value.
    let payload = serde_json::json!({
        "routes": [
            { "type": "pushState", "url": "https://example.com/a", "at": 1 },
            { "type": "hashchange", "url": "https://example.com/#/b", "at": 2 }
        ],
        "endpoints": [
            { "kind": "fetch", "method": "GET", "url": "https://api.example.com/v1/ping", "started_at": 3, "status": 200, "ok": true, "duration_ms": 42 },
            { "kind": "xhr", "method": "POST", "url": "https://api.example.com/v1/telemetry", "started_at": 4 }
        ]
    });
    let parsed: CollectedObservations = serde_json::from_value(payload).unwrap();
    assert_eq!(parsed.routes.len(), 2);
    assert_eq!(parsed.routes[0].kind, "pushState");
    assert_eq!(parsed.endpoints.len(), 2);
    assert_eq!(parsed.endpoints[0].kind, "fetch");
    assert_eq!(parsed.endpoints[0].status, Some(200));
    // The expression the renderer evaluates must reference both globals;
    // guards against swapping to a stale format without updating the read.
    let expr = collect_expression();
    assert!(expr.contains("__crawlex_runtime_routes__"));
    assert!(expr.contains("__crawlex_network_endpoints__"));
}
