//! Live DoH parsing + config smoke tests.
//!
//! The actual DoH transport in `crawlex` is opt-in and currently wired
//! at the config layer only — see `src/impersonate/doh.rs` for the
//! rationale. These tests cover the config surface (parser, provider
//! table, env var) so that when the hickory transport flag flips to
//! enabled the caller code doesn't need to change shape.
//!
//! The `#[ignore]`-marked test makes a real DoH HTTPS request to
//! Cloudflare via the crate's own `ImpersonateClient` — it's gated so
//! CI without network access stays green, but `cargo test -- --ignored
//! doh_live_cloudflare` verifies the endpoint answers a valid DoH wire
//! response for a realistic query.

use crawlex::impersonate::doh::{DohConfig, DohProvider};

#[test]
fn parse_off_equivalents() {
    for v in ["", "off", "OFF", "system", " System "] {
        let c = DohConfig::parse(v).unwrap();
        assert!(
            !c.is_enabled(),
            "{v:?} should disable DoH but produced {:?}",
            c
        );
    }
}

#[test]
fn parse_every_known_provider() {
    assert_eq!(
        DohConfig::parse("cloudflare").unwrap().provider,
        DohProvider::Cloudflare
    );
    assert_eq!(
        DohConfig::parse("google").unwrap().provider,
        DohProvider::Google
    );
    assert_eq!(
        DohConfig::parse("quad9").unwrap().provider,
        DohProvider::Quad9
    );
}

#[test]
fn endpoint_url_is_well_known_for_cloudflare() {
    let c = DohConfig::parse("cloudflare").unwrap();
    let u = c.endpoint_url().expect("cloudflare endpoint");
    assert_eq!(u.host_str(), Some("cloudflare-dns.com"));
    assert_eq!(u.path(), "/dns-query");
    assert_eq!(u.scheme(), "https");
}

#[test]
fn custom_url_requires_https() {
    assert!(DohConfig::parse("http://doh.test/dns-query").is_err());
    assert!(DohConfig::parse("ftp://doh.test/dns-query").is_err());
    let ok = DohConfig::parse("https://doh.test/dns-query").unwrap();
    assert_eq!(ok.provider, DohProvider::Custom);
}

/// Live-network smoke test — hits real Cloudflare DoH.
///
/// Ignored by default because:
///   1. Not every CI environment has egress.
///   2. We don't want Cloudflare seeing our test runner IP on every
///      `cargo test` invocation.
///
/// Run with `cargo test -- --ignored doh_live_cloudflare`.
#[tokio::test]
#[ignore]
async fn doh_live_cloudflare() {
    use crawlex::impersonate::{ImpersonateClient, Profile};
    // We issue a GET against cloudflare-dns.com with the `?dns=` query
    // parameter carrying a DNS wire-format AAAA-for-example.com message
    // (base64url-encoded, padding-stripped). If the endpoint answers
    // 200 with `content-type: application/dns-message` we know the DoH
    // path is live. This is the probe Chrome itself uses at first boot
    // when the user opts into "Secure DNS".
    //
    // Wire bytes for an A query for "example.com." with id=0 and
    // recursion-desired bit set. RFC 8484 §4.1.1 / §6.
    let dns_query = [
        0x00, 0x00, // id
        0x01, 0x00, // flags (RD)
        0x00, 0x01, // qdcount
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0x03,
        b'c', b'o', b'm', 0x00, // root label
        0x00, 0x01, // QTYPE=A
        0x00, 0x01, // QCLASS=IN
    ];
    let b64 = base64_url_nopad(&dns_query);
    let url = format!("https://cloudflare-dns.com/dns-query?dns={b64}")
        .parse()
        .unwrap();

    let client = ImpersonateClient::new(Profile::Chrome131Stable).expect("client");
    let resp = client.get(&url).await.expect("doh GET");
    assert!(resp.status.is_success(), "DoH status: {}", resp.status);
    let ctype = resp
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ctype.contains("application/dns-message"),
        "unexpected content-type: {ctype}"
    );
    // A DNS-message response is never empty — at minimum it echoes the
    // question section plus an answer section.
    assert!(
        resp.body.len() >= dns_query.len(),
        "body too short: {}",
        resp.body.len()
    );
}

/// RFC 4648 §5 base64url without padding. Pulled in here to avoid
/// adding a dev-dep just for the one live test.
fn base64_url_nopad(b: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((b.len() * 4).div_ceil(3));
    for chunk in b.chunks(3) {
        let (b0, b1, b2) = (chunk[0], chunk.get(1).copied(), chunk.get(2).copied());
        let n = ((b0 as u32) << 16) | ((b1.unwrap_or(0) as u32) << 8) | (b2.unwrap_or(0) as u32);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if b1.is_some() {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        }
        if b2.is_some() {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        }
    }
    out
}

#[test]
fn base64_url_encoder_matches_known_vector() {
    // "Hello" → base64url "SGVsbG8" (no padding).
    assert_eq!(base64_url_nopad(b"Hello"), "SGVsbG8");
}
