//! Closed-loop verifier: does our boringssl stack produce a Chrome-class
//! ClientHello on the wire?
//!
//! Spawns a local TCP canary, drives our `ImpersonateClient` against it,
//! captures the first TLS record (the ClientHello + any pipelined records),
//! parses it with `crawlex::impersonate::ja3::ClientHello`, and asserts the
//! fields we care about for Chrome M131+ impersonation.
//!
//! A handshake failure on the client side is *expected* — the canary
//! drops the connection after capturing the ClientHello; we don't care
//! about the reply, only the bytes we emitted.

use crawlex::impersonate::{
    ja3::{current_chrome_fingerprint_summary, ClientHello},
    ImpersonateClient, Profile,
};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

/// Accept one TCP connection on the listener and return the first bytes
/// we see (up to `max_bytes` or client close / 2s idle). This is the raw
/// TLS record stream the client emitted.
async fn capture_hello(listener: TcpListener, max_bytes: usize) -> Vec<u8> {
    let (mut sock, _peer) = listener.accept().await.expect("accept");
    let mut buf = vec![0u8; max_bytes];
    let mut total = 0usize;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let remaining = max_bytes.saturating_sub(total);
        if remaining == 0 {
            break;
        }
        let slice = &mut buf[total..];
        let sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(sleep);
        tokio::select! {
            n = sock.read(slice) => {
                match n {
                    Ok(0) => break,
                    Ok(n) => {
                        total += n;
                        // A full ClientHello usually fits in one TLS record
                        // (<= ~2 KiB for Chrome M131). Once we've read past
                        // the declared record length we're done.
                        if total >= 5 {
                            let rec_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
                            if total >= 5 + rec_len {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            _ = &mut sleep => break,
        }
    }
    buf.truncate(total);
    buf
}

async fn drive_client_against_canary(port: u16) {
    // Force DNS for "canary.test" to resolve to 127.0.0.1 by going through
    // the low-level impersonate path. Our ImpersonateClient resolves via
    // its own hickory resolver; easiest way to redirect is to use "127.0.0.1"
    // as the SNI host. Cloudflare / servers would reject that; our canary
    // doesn't care — it just reads bytes and drops.
    let client = ImpersonateClient::new(Profile::Chrome131Stable).expect("client");
    let url = format!("https://127.0.0.1:{port}/").parse().expect("url");
    // This WILL fail (canary drops mid-handshake). That's fine — we only
    // need the ClientHello bytes captured server-side.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), client.get(&url)).await;
}

async fn capture_with_profile(_profile: Profile) -> Vec<u8> {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(capture_hello(listener, 16 * 1024));
    drive_client_against_canary(port).await;
    server.await.expect("server join")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chrome131_clienthello_matches_expected_shape() {
    let bytes = capture_with_profile(Profile::Chrome131Stable).await;
    assert!(
        bytes.len() >= 100,
        "captured too few bytes: {} — connection likely never reached TLS",
        bytes.len()
    );

    let ch = ClientHello::parse(&bytes).unwrap_or_else(|e| {
        panic!(
            "parse failed: {e}\nraw={:02x?}",
            &bytes[..bytes.len().min(96)]
        )
    });

    // Print a summary on failure for whatever assertion fails below.
    eprintln!("--- ClientHello summary ---\n{}", ch.summary());

    // TLS 1.3 advertised via supported_versions extension.
    assert!(
        ch.supported_versions.contains(&0x0304),
        "supported_versions missing TLS 1.3: {:?}",
        ch.supported_versions
    );

    // Legacy record version 1.2 (Chrome's compatibility value).
    assert_eq!(
        ch.legacy_version, 0x0303,
        "legacy_version should be TLS 1.2"
    );

    // Cipher count must match Chrome M131's list (we ship 11 suites; real
    // Chrome M131 ships 11 non-GREASE suites too). Any drift here means
    // our cipher list changed vs. Chrome.
    assert_eq!(
        ch.cipher_suites.len(),
        11,
        "cipher_suites length != 11 (Chrome M131 ships 11): got {:?}",
        ch.cipher_suites
    );

    // TLS 1.3 suites MUST lead (Chrome order). First three must be:
    //   0x1301 TLS_AES_128_GCM_SHA256
    //   0x1302 TLS_AES_256_GCM_SHA384
    //   0x1303 TLS_CHACHA20_POLY1305_SHA256
    assert_eq!(&ch.cipher_suites[..3], &[0x1301, 0x1302, 0x1303]);

    // ALPN: h2 first, http/1.1 second.
    assert_eq!(ch.alpn, vec!["h2".to_string(), "http/1.1".to_string()]);

    // Signature algorithms: must include ed25519 (0x0807). Chrome M120+.
    assert!(
        ch.signature_algorithms.contains(&0x0807),
        "signature_algorithms missing ed25519 (0x0807): {:?}",
        ch.signature_algorithms
    );

    // Supported groups: must lead with X25519MLKEM768 (0x11ec), the
    // current Chrome M128+ post-quantum hybrid. 0x6399 (Kyber draft-00)
    // was the legacy value; if this assertion fails with 0x6399, the
    // rename in `chrome_curves()` regressed.
    let first_group = *ch.supported_groups.first().expect("supported_groups empty");
    assert_eq!(
        first_group, 0x11ec,
        "first supported_group must be X25519MLKEM768 (0x11ec), got {first_group:#06x}"
    );

    // cert_compression: Chrome M131 advertises brotli(2), zlib(1), zstd(3).
    // All three must be present.
    for alg in [1u16, 2, 3] {
        assert!(
            ch.cert_compression_algs.contains(&alg),
            "cert_compression missing alg {alg}: {:?}",
            ch.cert_compression_algs
        );
    }

    // ECH GREASE (ext 65037 = 0xfe0d): Chrome M117+ always sends this.
    assert!(
        ch.has_ech_ext,
        "ECH GREASE extension missing — SSL_set_enable_ech_grease call regressed"
    );

    // Mandatory Chrome M131 extensions (SNI ext 0 intentionally excluded
    // here: the canary uses an IP literal host, and real Chrome also omits
    // SNI when the hostname is an IP — testing SNI belongs in a separate
    // hostname-based fixture). 65037 = ECH GREASE.
    for &must in &[10u16, 11, 13, 16, 23, 27, 35, 43, 45, 51, 65037, 65281] {
        assert!(
            ch.extensions.contains(&must),
            "missing extension {must:#06x} in {:?}",
            ch.extensions_sorted
        );
    }
    // ALPS: accept either codepoint — 17513 (new draft, Chrome M131+) or
    // 17613 (old draft, our vendored BoringSSL). Gap is tracked under task
    // #2 "boringssl M131+ parity".
    let has_alps = ch.extensions.contains(&17513) || ch.extensions.contains(&17613);
    assert!(
        has_alps,
        "missing ALPS (neither 17513 nor 17613) in {:?}",
        ch.extensions_sorted
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alps_advertises_h2_in_clienthello() {
    let bytes = capture_with_profile(Profile::Chrome131Stable).await;
    assert!(bytes.len() >= 100, "short capture");

    let ch = ClientHello::parse(&bytes).expect("parse");
    eprintln!("--- ClientHello summary ---\n{}", ch.summary());

    // ALPS ext must be present with "h2" in its protocol list. The h2
    // SETTINGS bytes themselves travel in EncryptedExtensions (TLS 1.3)
    // and are not visible in ClientHello plaintext — the 24-byte
    // SETTINGS blob is covered by the unit test
    // `impersonate::tls::tests::alps_h2_settings_layout_matches_chrome`.
    let protos: Vec<&str> = ch
        .alps_payload_by_proto
        .iter()
        .map(|(p, _)| p.as_str())
        .collect();
    assert!(
        protos.contains(&"h2"),
        "ALPS must advertise h2 in ClientHello proto list, got {:?}",
        protos
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_matches_declared_chrome_fingerprint() {
    // Regression rail: the static `current_chrome_fingerprint_summary`
    // must match what our TLS stack actually emits. If the constant and
    // the wire diverge, either (a) the boringssl knobs regressed, or
    // (b) someone updated the constant without the underlying change —
    // both are caught here.
    let bytes = capture_with_profile(Profile::Chrome131Stable).await;
    let ch = ClientHello::parse(&bytes).expect("parse");

    let expected = current_chrome_fingerprint_summary(Profile::Chrome131Stable);
    let pq = *ch.supported_groups.first().expect("no groups");
    let pq_name = match pq {
        0x11ec => "X25519MLKEM768",
        0x6399 => "X25519Kyber768Draft00",
        other => panic!("unexpected pq group {other:#06x}"),
    };
    let cc: String = ch
        .cert_compression_algs
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let observed = format!(
        "{}|ciphers={}|pq={}|cert_comp=[{}]|ech={}",
        ch.ja4_a(),
        ch.cipher_suites.len(),
        pq_name,
        cc,
        if ch.has_ech_ext { 1 } else { 0 }
    );
    assert_eq!(observed, expected, "wire summary drift");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ja3_and_ja4_are_stable_strings() {
    // JA3 and JA4 must produce non-empty stable strings. We don't assert
    // a specific hash here because that depends on the vendored BoringSSL
    // PQ group (Kyber vs MLKEM) — JA3/JA4 change when the group ID does.
    // The harness is about parity + regression detection, not a fixed
    // magic string.
    let bytes = capture_with_profile(Profile::Chrome131Stable).await;
    let ch = ClientHello::parse(&bytes).expect("parse");
    let ja3 = ch.ja3_string();
    let ja4 = ch.ja4_a();
    assert!(
        ja3.contains(',') && ja3.len() > 20,
        "JA3 looks wrong: {ja3}"
    );
    assert!(
        ja4.starts_with("t13") || ja4.starts_with("t12"),
        "JA4_a should start with t13/t12, got {ja4}"
    );
    eprintln!("JA3={ja3}\nJA4_a={ja4}");
}

/// Helper so the client-side connection attempt is not an inadvertent net
/// dependency when Cargo runs tests offline.
#[allow(dead_code)]
async fn ensure_loopback_reachable() {
    let ok = TcpStream::connect(("127.0.0.1", 53534)).await.is_ok();
    let _ = ok;
}
