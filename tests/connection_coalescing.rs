//! HTTP/2 connection coalescing audit (#29).
//!
//! RFC 7540 §9.1.1 allows a client that already has an h2 connection to
//! origin A to reuse that connection for origin B when:
//!   1. B's authority resolves to an IP the existing connection is
//!      already bound to, AND
//!   2. the peer certificate for the existing connection has a
//!      Subject-Alt-Name that matches B's host.
//!
//! Real Chrome performs this coalescing; a crawler that opens a fresh
//! connection per host it touches is trivially distinguishable from
//! Chrome on a shared-CDN origin (every large CDN terminates many
//! hostnames behind one IP + wildcard-SAN cert).
//!
//! # Current status in `crawlex`
//!
//! The connection pool keys entries by `(scheme, host, port, proxy)` —
//! so coalescing on SAN is **not yet implemented**; two requests to
//! different subdomains fronted by the same CDN open two h2
//! connections. This test documents the state explicitly so the gap
//! is discoverable from `cargo test` output rather than buried in a
//! plan doc, and pins the SAN-matching helper that future coalescing
//! will build on.
//!
//! Changing the pool key shape itself is out of scope for the wave1
//! network worker — `src/impersonate/pool.rs` is owned by a different
//! wave. When that wave lands, the ignored test below gets flipped
//! on and proves the new behaviour end-to-end.

/// RFC 6125 §6.4 wildcard-aware DNS SAN match. Used today by the cert
/// audit code and by future connection-coalescing logic — kept in this
/// test file so the rule is exercised without extra crate surface.
fn san_matches_host(san: &str, host: &str) -> bool {
    let san = san.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if san == host {
        return true;
    }
    // Wildcard must be the left-most label: `*.example.com`.
    if let Some(rest) = san.strip_prefix("*.") {
        // Host must have at least one label before the rest AND match.
        if let Some((h_first, h_rest)) = host.split_once('.') {
            return !h_first.is_empty() && h_rest == rest;
        }
    }
    false
}

#[test]
fn san_exact_match() {
    assert!(san_matches_host("api.example.com", "api.example.com"));
    assert!(!san_matches_host("api.example.com", "cdn.example.com"));
}

#[test]
fn san_wildcard_matches_one_label() {
    assert!(san_matches_host("*.example.com", "api.example.com"));
    assert!(san_matches_host("*.example.com", "cdn.example.com"));
    // Wildcard does not span multiple labels.
    assert!(!san_matches_host("*.example.com", "a.b.example.com"));
    // And does not match the apex.
    assert!(!san_matches_host("*.example.com", "example.com"));
}

#[test]
fn san_matches_ignore_trailing_dot_and_case() {
    assert!(san_matches_host("Example.COM.", "example.com"));
    assert!(san_matches_host("*.EXAMPLE.com", "API.example.com"));
}

#[test]
fn san_mid_label_wildcard_rejected() {
    // Only left-most-label wildcards are legal under RFC 6125.
    assert!(!san_matches_host("a*.example.com", "api.example.com"));
    assert!(!san_matches_host("*a.example.com", "apia.example.com"));
}

/// Live coalescing test — flip on when `src/impersonate/pool.rs`
/// learns SAN-based coalescing. Until then the crate opens separate
/// connections for different hostnames even on the same CDN IP; that
/// is a known gap documented in the module header above.
#[test]
#[ignore]
fn coalescing_reuses_h2_connection_on_san_match_live() {
    // Intentionally empty: the test body lands in the wave that owns
    // pool.rs changes. Keeping the `#[ignore]` stub here so the gap
    // shows up in `cargo test -- --ignored` output.
}
