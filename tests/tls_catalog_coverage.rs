//! Coverage gate for the TLS fingerprint catalog.
//!
//! User contract: every nominally-supported browser/major/os tuple must
//! resolve to *some* fingerprint (exact catalog hit OR era-fallback). Rule
//! out the case where `Profile::for_chrome(149).os(BrowserOs::Linux).build()`
//! silently returns Err and the crawler falls back to a default that breaks
//! TLS impersonation.
//!
//! This is the contract the v1 release blocks on: 30 last Chrome stable
//! (120-149), 30 last Chromium (120-149), 20 last Firefox (111-130). Any
//! gap = test fail.

use crawlex::impersonate::catalog::BrowserOs;
use crawlex::impersonate::profiles::Profile;

fn assert_resolves(label: &str, p: Result<Profile, impl std::fmt::Debug>) {
    let profile = p.unwrap_or_else(|e| panic!("{label} build failed: {e:?}"));
    let fp = profile
        .tls()
        .unwrap_or_else(|| panic!("{label} no TLS fingerprint resolved"));
    assert!(
        !fp.name.is_empty(),
        "{label} resolved to empty-name fingerprint"
    );
    assert!(
        !fp.ciphersuites.is_empty(),
        "{label} resolved fingerprint has no ciphers"
    );
    assert!(
        !fp.extensions.is_empty(),
        "{label} resolved fingerprint has no extensions"
    );
}

#[test]
fn chrome_last_30_stable_all_resolve() {
    // Chrome stable 120-149 — 30 majors.
    for major in 120u16..=149 {
        for os in [BrowserOs::Linux, BrowserOs::Windows, BrowserOs::MacOs] {
            let label = format!("chrome {} {:?}", major, os);
            assert_resolves(&label, Profile::for_chrome(major).os(os).build());
        }
    }
}

#[test]
fn chromium_last_30_all_resolve_linux() {
    // Chromium 120-149 on Linux — Chromium is rare on Win/Mac.
    for major in 120u16..=149 {
        let label = format!("chromium {} linux", major);
        assert_resolves(
            &label,
            Profile::for_chromium(major).os(BrowserOs::Linux).build(),
        );
    }
}

#[test]
fn firefox_last_20_all_resolve() {
    // Firefox 111-130 — 20 majors.
    for major in 111u16..=130 {
        for os in [BrowserOs::Linux, BrowserOs::Windows, BrowserOs::MacOs] {
            let label = format!("firefox {} {:?}", major, os);
            assert_resolves(&label, Profile::for_firefox(major).os(os).build());
        }
    }
}

#[test]
fn legacy_named_profiles_still_resolve() {
    // Legacy enum variants must keep working through the catalog.
    let p = Profile::Chrome131Stable;
    let fp = p.tls().expect("Chrome131Stable resolves");
    assert!(fp.name.starts_with("chrome_"));

    let p = Profile::Chrome149Stable;
    let fp = p.tls().expect("Chrome149Stable resolves");
    assert!(fp.name.starts_with("chrome_"));
}

#[test]
fn ja3_string_is_well_formed() {
    let p = Profile::for_chrome(149)
        .os(BrowserOs::Linux)
        .build()
        .unwrap();
    let fp = p.tls().unwrap();
    let ja3 = fp.ja3_string();
    // JA3 = "version,ciphers,extensions,groups,formats" — five comma-sep fields.
    assert_eq!(
        ja3.matches(',').count(),
        4,
        "ja3 should have exactly 4 commas (5 fields), got: {ja3}"
    );
    // Version is a 4-digit hex codepoint (0x0303 / 0x0304).
    let first = ja3.split(',').next().unwrap();
    assert!(
        ["771", "772"].contains(&first),
        "ja3 version field should be 771 (TLS 1.2) or 772 (TLS 1.3), got {first}"
    );
}

#[test]
fn cipher_lists_render_to_openssl_names() {
    let p = Profile::for_chrome(149)
        .os(BrowserOs::Linux)
        .build()
        .unwrap();
    let fp = p.tls().unwrap();
    let openssl_list = crawlex::impersonate::catalog::render_cipher_list(fp);
    // Must contain the canonical TLS 1.3 suite Chrome leads with.
    assert!(
        openssl_list.contains("TLS_AES_128_GCM_SHA256"),
        "cipher list missing TLS 1.3 lead suite: {openssl_list}"
    );
    // Must be colon-joined, not comma-joined (BoringSSL convention).
    assert!(
        !openssl_list.contains(','),
        "cipher list should be colon-joined, got: {openssl_list}"
    );
}

#[test]
fn curve_lists_render_to_openssl_names() {
    let p = Profile::for_chrome(149)
        .os(BrowserOs::Linux)
        .build()
        .unwrap();
    let fp = p.tls().unwrap();
    let openssl_list = crawlex::impersonate::catalog::render_curves_list(fp);
    // Must contain X25519 — every Chrome/Firefox era ships it.
    assert!(
        openssl_list.contains("X25519"),
        "curve list missing X25519: {openssl_list}"
    );
}

#[test]
fn sigalg_lists_render_to_openssl_names() {
    let p = Profile::for_chrome(149)
        .os(BrowserOs::Linux)
        .build()
        .unwrap();
    let fp = p.tls().unwrap();
    let openssl_list = crawlex::impersonate::catalog::render_sigalgs_list(fp);
    // Must contain ECDSA P-256 SHA256 — Chrome's preferred sigalg.
    assert!(
        openssl_list.contains("ecdsa_secp256r1_sha256"),
        "sigalg list missing ecdsa_secp256r1_sha256: {openssl_list}"
    );
}

#[test]
fn alpn_wire_encoding_is_length_prefixed() {
    let p = Profile::for_chrome(149)
        .os(BrowserOs::Linux)
        .build()
        .unwrap();
    let fp = p.tls().unwrap();
    let wire = crawlex::impersonate::catalog::encode_alpn_wire(fp.alpn);
    // First byte = first protocol length. For Chrome ["h2", "http/1.1"]:
    // \x02h2\x08http/1.1 = 2 + 2 + 1 + 8 = 13 bytes (no separators in wire).
    assert!(!wire.is_empty(), "alpn wire is empty");
    assert!(
        wire[0] as usize + 1 <= wire.len(),
        "first alpn length byte ({}) > remaining wire ({})",
        wire[0],
        wire.len() - 1
    );
}

#[test]
fn catalog_contains_at_least_curl_impersonate_baseline() {
    // curl-impersonate vendored YAMLs ship 21 profiles. Coverage gate:
    // build.rs must emit at least that many before our own captures land.
    let count = crawlex::impersonate::catalog::all().count();
    assert!(
        count >= 21,
        "expected ≥21 profiles from curl-impersonate baseline, got {count}"
    );
}
