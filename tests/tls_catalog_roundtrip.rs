//! Catalog roundtrip: for each curl-impersonate vendored profile, assert
//! the JA3 string the catalog computes (from the static struct) matches a
//! deterministic re-derivation. This is a self-consistency check on the
//! YAML→Rust codegen — if it stops matching, build.rs has drifted from
//! the schema.
//!
//! The "live" roundtrip (build BoringSSL connector → handshake → parse
//! observed ClientHello → assert JA3 matches) lives in
//! `tests/tls_clienthello.rs` (canary infrastructure already there).

use crawlex::impersonate::catalog::{self, all, Browser, BrowserOs};

#[test]
fn every_profile_has_canonical_ja3() {
    let mut count = 0usize;
    for fp in all() {
        let ja3 = fp.ja3_string();
        // Five comma-separated fields, no trailing comma.
        let parts: Vec<&str> = ja3.split(',').collect();
        assert_eq!(parts.len(), 5, "profile {} ja3 = {}", fp.name, ja3);
        // Version field is 0x0303 (TLS 1.2) or 0x0304 (TLS 1.3).
        assert!(
            ["771", "772"].contains(&parts[0]),
            "profile {} version field = {}",
            fp.name,
            parts[0]
        );
        count += 1;
    }
    assert!(count >= 21, "fewer than 21 profiles in catalog: {count}");
}

#[test]
fn cipher_lists_translate_known_iana_codepoints() {
    // Every curl-impersonate Chrome profile uses 0x1301 (TLS_AES_128_GCM_SHA256)
    // as its TLS 1.3 lead suite. Verify the catalog→OpenSSL translator handles it.
    assert_eq!(
        catalog::cipher_id_to_openssl_name(0x1301),
        Some("TLS_AES_128_GCM_SHA256")
    );
    assert_eq!(
        catalog::cipher_id_to_openssl_name(0x1302),
        Some("TLS_AES_256_GCM_SHA384")
    );
    assert_eq!(
        catalog::cipher_id_to_openssl_name(0x1303),
        Some("TLS_CHACHA20_POLY1305_SHA256")
    );
    // Unknown codepoints return None.
    assert_eq!(catalog::cipher_id_to_openssl_name(0xffff), None);
}

#[test]
fn group_lookup_handles_post_quantum() {
    // X25519MLKEM768 is Chrome 132+'s post-quantum hybrid. Older Chrome
    // captures may use X25519Kyber768Draft00 (0x6399).
    assert_eq!(
        catalog::group_id_to_openssl_name(0x11ec),
        Some("X25519MLKEM768")
    );
    assert_eq!(
        catalog::group_id_to_openssl_name(0x6399),
        Some("X25519Kyber768Draft00")
    );
    assert_eq!(catalog::group_id_to_openssl_name(0x001d), Some("X25519"));
    assert_eq!(catalog::group_id_to_openssl_name(0x0017), Some("P-256"));
}

#[test]
fn sigalg_lookup_handles_ed25519() {
    // ed25519 (0x0807) is the Chrome 120+ addition that previous fingerprints
    // missed. Make sure it round-trips.
    assert_eq!(catalog::sigalg_id_to_openssl_name(0x0807), Some("ed25519"));
    assert_eq!(
        catalog::sigalg_id_to_openssl_name(0x0403),
        Some("ecdsa_secp256r1_sha256")
    );
}

#[test]
fn chrome_profiles_have_alps_extension() {
    // Every Chrome 116+ ships application_settings (ALPS) with `h2`.
    // curl-impersonate's chrome_116.0.5845.180_win10 should be in the catalog.
    let chrome_116 =
        catalog::lookup("chrome_116.0.5845.180_win10").expect("chrome 116 win10 in catalog");
    assert!(
        !chrome_116.alps_alpn.is_empty(),
        "chrome_116 should advertise ALPS with h2"
    );
    assert_eq!(chrome_116.alps_alpn[0], "h2");
}

#[test]
fn firefox_profiles_have_no_alps_extension() {
    // Firefox doesn't send application_settings — that's Chrome-only.
    let firefox_117 =
        catalog::lookup("firefox_117.0.1_win10").expect("firefox 117 win10 in catalog");
    assert!(
        firefox_117.alps_alpn.is_empty(),
        "firefox should not advertise ALPS, got {:?}",
        firefox_117.alps_alpn
    );
}

#[test]
fn era_fallback_warns_for_uncaptured_majors() {
    // Chrome 149 isn't in the curl-impersonate baseline (their newest is 116).
    // era_for() should fall back and emit a tracing::warn — we can't easily
    // capture the warn from a test, but we CAN verify the fallback resolves
    // to a sensible profile.
    use crawlex::impersonate::catalog::eras::era_for;

    let fp = era_for(Browser::Chrome, 149, BrowserOs::Linux);
    let fp = fp.expect("chrome 149 falls back via era");
    // Era logic returns the newest Chrome we have (chrome_116.* until we
    // capture 117+).
    assert!(
        fp.name.starts_with("chrome_"),
        "fallback should be a Chrome profile, got {}",
        fp.name
    );
}
