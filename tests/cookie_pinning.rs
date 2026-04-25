//! Antibot cookie pinning — vendor-specific replay.
//!
//! Covers `CookiePinStore` pin/get/expiry round-trips on both the
//! in-memory backend and the SQLite-backed one, plus the header-capture
//! path used by `antibot::bypass`.

use crawlex::antibot::bypass::{
    capture_from_headers, pin_captured, prepare_turnstile_attempt, BypassLevel,
    TurnstileAttemptOutcome,
};
use crawlex::antibot::cookie_pin::{
    CookiePinStore, InMemoryCookiePinStore, AKAMAI_ABCK_TTL_SECS, DATADOME_TTL_SECS,
};

use http::HeaderMap;

fn hdr(values: &[&str]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for v in values {
        h.append("set-cookie", v.parse().unwrap());
    }
    h
}

#[test]
fn memory_store_pin_retrieve_expiry() {
    let store = InMemoryCookiePinStore::new();
    store
        .pin(
            "akamai",
            "https://target.example",
            "_abck",
            "VALID-SOLVED-VALUE",
            AKAMAI_ABCK_TTL_SECS,
        )
        .unwrap();
    let got = store
        .get_pinned("akamai", "https://target.example", "_abck")
        .unwrap()
        .expect("must retrieve a freshly-pinned cookie");
    assert_eq!(got.value, "VALID-SOLVED-VALUE");
    assert_eq!(got.ttl_secs, AKAMAI_ABCK_TTL_SECS);

    // ttl=0 → immediately expired.
    store
        .pin("datadome", "https://target.example", "datadome", "X", 0)
        .unwrap();
    assert!(store
        .get_pinned("datadome", "https://target.example", "datadome")
        .unwrap()
        .is_none());
}

#[test]
#[cfg(feature = "sqlite")]
fn sqlite_store_pin_retrieve_roundtrip() {
    use crawlex::antibot::cookie_pin::SqliteCookiePinStore;
    let store = SqliteCookiePinStore::open_in_memory().unwrap();
    store
        .pin(
            "perimeterx",
            "https://target.example",
            "_px3",
            "ABC123",
            3600,
        )
        .unwrap();
    let got = store
        .get_pinned("perimeterx", "https://target.example", "_px3")
        .unwrap()
        .expect("sqlite roundtrip");
    assert_eq!(got.value, "ABC123");
    assert_eq!(got.ttl_secs, 3600);

    // Missing entry → None, no error.
    let miss = store
        .get_pinned("perimeterx", "https://target.example", "_pxvid")
        .unwrap();
    assert!(miss.is_none());

    // Overwrite semantics: re-pinning same triple updates value.
    store
        .pin(
            "perimeterx",
            "https://target.example",
            "_px3",
            "DEF456",
            3600,
        )
        .unwrap();
    let got = store
        .get_pinned("perimeterx", "https://target.example", "_px3")
        .unwrap()
        .unwrap();
    assert_eq!(got.value, "DEF456");
}

#[test]
fn capture_and_pin_end_to_end() {
    let store = InMemoryCookiePinStore::new();
    let h = hdr(&[
        "_abck=DEADBEEFCAFE0123456789ABCDEF01234567~solved~flags; Path=/; Domain=.target.example",
        "datadome=ZZZ999XXX111; Path=/; Max-Age=3600",
        "_px3=pxABCDEF; Path=/",
        "csrf=unrelated; Path=/",
    ]);
    let captured = capture_from_headers(&h, 403);
    assert_eq!(
        captured.len(),
        3,
        "three vendor cookies captured, csrf ignored"
    );

    let n = pin_captured(&store, "https://target.example", &captured);
    assert_eq!(n, 3);

    let abck = store
        .get_pinned("akamai", "https://target.example", "_abck")
        .unwrap()
        .unwrap();
    assert_eq!(abck.ttl_secs, AKAMAI_ABCK_TTL_SECS);

    let dd = store
        .get_pinned("datadome", "https://target.example", "datadome")
        .unwrap()
        .unwrap();
    assert_eq!(dd.ttl_secs, DATADOME_TTL_SECS);
    assert_eq!(dd.value, "ZZZ999XXX111");
}

#[test]
fn bypass_level_default_is_none() {
    // Regression guard: flipping the default would silently enable
    // tricks on every operator's run. Must stay opt-in.
    let level = BypassLevel::default();
    assert_eq!(level, BypassLevel::None);
    assert!(!level.allows_replay());
    assert!(!level.allows_aggressive());
}

#[test]
fn turnstile_attempt_is_gated_on_aggressive_level() {
    // Level=None refuses even with sitekey + invisible widget.
    let none = prepare_turnstile_attempt(BypassLevel::None, Some("0x4AAA"), true);
    assert!(matches!(
        none.outcome,
        TurnstileAttemptOutcome::NotAttempted(_)
    ));

    // Level=Replay also refuses (it's passive-only).
    let replay = prepare_turnstile_attempt(BypassLevel::Replay, Some("0x4AAA"), true);
    assert!(matches!(
        replay.outcome,
        TurnstileAttemptOutcome::NotAttempted(_)
    ));

    // Level=Aggressive with sitekey + invisible → prepared.
    let agg = prepare_turnstile_attempt(BypassLevel::Aggressive, Some("0x4AAA"), true);
    match agg.outcome {
        TurnstileAttemptOutcome::Prepared {
            sitekey,
            endpoint,
            dummy_token,
        } => {
            assert_eq!(sitekey, "0x4AAA");
            assert!(endpoint.contains("challenges.cloudflare.com"));
            assert!(dummy_token.contains("DUMMY"));
        }
        other => panic!("expected Prepared, got {other:?}"),
    }
}
