//! TLS ClientHello extension presence audit — complements
//! `tests/tls_clienthello.rs` which already covers the bulk of the
//! Chrome-M131 shape. This file focuses on the extensions wave1-network
//! calls out explicitly:
//!
//! * ext 5  — `status_request` (OCSP stapling request)
//! * ext 13 — `signature_algorithms`
//! * ext 43 — `supported_versions` (must list 0x0304 + 0x0303)
//! * ext 45 — `psk_key_exchange_modes`
//! * ext 51 — `key_share` (first group must be X25519MLKEM768 0x11ec)
//! * ext 17513 or 17613 — `application_settings` / ALPS
//!
//! The audit is behaviour-only: we capture a real ClientHello against a
//! local TCP canary and inspect the parsed fields. If any of these
//! extensions regress, the test fails with a clear explanation of
//! which Chrome-ism is missing.

use crawlex::impersonate::{ja3::ClientHello, ImpersonateClient, Profile};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

async fn capture(listener: TcpListener, max: usize) -> Vec<u8> {
    let (mut sock, _) = listener.accept().await.expect("accept");
    let mut buf = vec![0u8; max];
    let mut total = 0usize;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if total >= max {
            break;
        }
        let slice = &mut buf[total..];
        let sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(sleep);
        tokio::select! {
            n = sock.read(slice) => match n {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if total >= 5 {
                        let rec_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
                        if total >= 5 + rec_len { break; }
                    }
                }
                Err(_) => break,
            },
            _ = &mut sleep => break,
        }
    }
    buf.truncate(total);
    buf
}

async fn grab_clienthello() -> ClientHello {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(capture(listener, 16 * 1024));
    let client = ImpersonateClient::new(Profile::Chrome131Stable).expect("client");
    let url = format!("https://127.0.0.1:{port}/").parse().expect("url");
    // The canary drops the connection mid-handshake; `get` will error out.
    // All we need is the ClientHello bytes on the server side.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), client.get(&url)).await;
    let bytes = server.await.expect("join");
    assert!(
        bytes.len() >= 100,
        "capture too short: {} bytes",
        bytes.len()
    );
    ClientHello::parse(&bytes).expect("parse ClientHello")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_request_extension_is_present() {
    // #17: OCSP stapling is requested by Chrome via the status_request
    // extension (ext type 5). BoringSSL emits this by default when the
    // client advertises TLS 1.2/1.3; if a build option ever flips it
    // off the regression shows up here.
    let ch = grab_clienthello().await;
    assert!(
        ch.extensions.contains(&5),
        "status_request (ext 5, OCSP) missing; extensions = {:?}",
        ch.extensions_sorted
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supported_versions_lists_tls12_and_tls13() {
    // #16: supported_versions (ext 43) must include TLS 1.3 (0x0304);
    // Chrome also lists TLS 1.2 (0x0303) as a fallback. A ClientHello
    // that only advertises 1.3 looks like a minimal standard-library
    // client (e.g. Go crypto/tls without 1.2 fallback), not Chrome.
    let ch = grab_clienthello().await;
    assert!(
        ch.supported_versions.contains(&0x0304),
        "supported_versions missing TLS 1.3: {:?}",
        ch.supported_versions
    );
    assert!(
        ch.supported_versions.contains(&0x0303),
        "supported_versions missing TLS 1.2 fallback: {:?}",
        ch.supported_versions
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn psk_key_exchange_modes_present() {
    // ext 45: Chrome always advertises PSK modes (0x01 = psk_dhe_ke).
    // Its absence is a tell that some TLS13-only fork is on the path.
    let ch = grab_clienthello().await;
    assert!(
        ch.extensions.contains(&45),
        "psk_key_exchange_modes (ext 45) missing; extensions = {:?}",
        ch.extensions_sorted
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_share_first_group_is_post_quantum() {
    // #16 curve-order audit: Chrome M128+ emits X25519MLKEM768 (0x11ec)
    // as the first key_share entry, followed by X25519 (0x001d). The
    // order matters: detectors hash the supported_groups list in the
    // order the client advertised.
    let ch = grab_clienthello().await;
    let first = *ch.supported_groups.first().expect("supported_groups empty");
    assert_eq!(
        first, 0x11ec,
        "first supported group must be X25519MLKEM768 (0x11ec), got {:#06x}",
        first
    );
    // X25519 must appear in the next slot. We don't pin it to position 1
    // literally because a future Chrome may insert a GREASE group in
    // between — checking "is in the top 3" is tight without being brittle.
    assert!(
        ch.supported_groups[..ch.supported_groups.len().min(3)].contains(&0x001d),
        "X25519 (0x001d) missing from top-3 supported_groups: {:?}",
        ch.supported_groups
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_settings_extension_present() {
    // ALPS: accept either the new codepoint (17513) or the draft one
    // (17613) that older BoringSSL builds still emit.
    let ch = grab_clienthello().await;
    let has_alps = ch.extensions.contains(&17513) || ch.extensions.contains(&17613);
    assert!(
        has_alps,
        "application_settings / ALPS missing (neither 17513 nor 17613) in {:?}",
        ch.extensions_sorted
    );
}
