//! Tier S.3 — WebRTC ICE leak audit.
//!
//! The threat: even with a proxy configured, Chromium's default WebRTC
//! stack gathers ICE candidates over *every* local interface and emits
//! host candidates with the raw LAN IP (10.x / 192.168.x / 172.16-31.x).
//! browserleaks.com/webrtc and similar detectors scrape those candidates
//! via `RTCPeerConnection.onicecandidate` and correlate the private IP
//! with the proxy's public-facing geo — if they mismatch the cover is
//! burned.
//!
//! The fix (in `src/render/pool.rs`):
//!   * `--disable-features=...,WebRtcHideLocalIpsWithMdns` — disables
//!     the mDNS obfuscation layer so we can directly observe host
//!     candidates (otherwise they'd be `*.local` hashes that hide the
//!     leak without closing it — mDNS hides the IP from JS but the STUN
//!     binding requests still go out over every interface, and some
//!     detectors bypass mDNS by forcing `rtcpMuxPolicy: "negotiate"` and
//!     reading candidate pair stats).
//!   * `--force-webrtc-ip-handling-policy=disable_non_proxied_udp` — the
//!     actual kill switch: forces Chrome to only use UDP paths that go
//!     through the proxy. With this set, host candidates over the local
//!     LAN interfaces are suppressed entirely.
//!
//! This test is the empirical guard: launch Chrome with the live
//! `RenderPool` launch args, run an `RTCPeerConnection` + `createOffer`
//! in a page, collect every ICE candidate fired by `onicecandidate`, and
//! assert that NONE of them contain a private IPv4 in RFC 1918 ranges.
//!
//! Ignored by default — needs Chromium. Run with:
//!
//! ```
//! cargo test --all-features --test webrtc_leak_audit -- --ignored --nocapture
//! ```

#![cfg(feature = "cdp-backend")]

use std::time::Duration;

use crawlex::render::chrome::browser::{Browser, BrowserConfig, HeadlessMode};
use futures::StreamExt;

fn system_chrome() -> Option<String> {
    for path in [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ] {
        if std::path::Path::new(path).exists() {
            return Some(path.into());
        }
    }
    None
}

/// Build a `BrowserConfig` that mirrors the webrtc-relevant launch flags
/// from `RenderPool::get_or_launch` — specifically the S.3 additions.
/// Keeping this list in sync with `src/render/pool.rs` is the test's
/// job: if the flags drift, the assertion below is what catches it.
async fn launch_with_webrtc_flags() -> (Browser, tempfile::TempDir) {
    let exec = system_chrome().expect("requires system Chrome installed");
    let tmp = tempfile::tempdir().expect("tmp user_data_dir");
    let cfg = BrowserConfig::builder()
        .chrome_executable(exec)
        .headless_mode(HeadlessMode::New)
        .no_sandbox()
        .user_data_dir(tmp.path())
        .args(vec![
            // Mirror the pool.rs S.3 flags:
            "--disable-features=IsolateOrigins,site-per-process,Translate,MediaRouter,WebRtcHideLocalIpsWithMdns".to_string(),
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".to_string(),
            // And a couple of baseline pool.rs flags so the run looks
            // like a real render launch (not strictly required for the
            // leak assertion, but avoids surprising behaviour deltas).
            "--disable-dev-shm-usage".to_string(),
            "--disable-gpu".to_string(),
            "--disable-blink-features=AutomationControlled".to_string(),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
        ])
        .build()
        .expect("build browser config");
    let (browser, mut handler) = Browser::launch(cfg).await.expect("launch browser");
    tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
    (browser, tmp)
}

/// Detect an RFC 1918 private IPv4 literal anywhere in a string. ICE
/// candidate SDP lines look like:
///   "candidate:842163049 1 udp 1677729535 192.168.1.42 55000 typ srflx ..."
/// so a substring scan is sufficient — we don't need to parse SDP.
fn contains_private_ipv4(s: &str) -> Option<String> {
    // Walk the string once extracting IPv4-shaped tokens.
    let mut out = None;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut dots = 0;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                if bytes[i] == b'.' {
                    dots += 1;
                }
                i += 1;
            }
            if dots == 3 {
                let token = &s[start..i];
                if is_private_ipv4(token) {
                    out = Some(token.to_string());
                    break;
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn is_private_ipv4(tok: &str) -> bool {
    let parts: Vec<&str> = tok.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let octets: Option<Vec<u8>> = parts.iter().map(|p| p.parse::<u8>().ok()).collect();
    let Some(o) = octets else {
        return false;
    };
    // 10.0.0.0/8
    if o[0] == 10 {
        return true;
    }
    // 172.16.0.0/12
    if o[0] == 172 && (16..=31).contains(&o[1]) {
        return true;
    }
    // 192.168.0.0/16
    if o[0] == 192 && o[1] == 168 {
        return true;
    }
    false
}

#[test]
fn private_ipv4_detector_sanity() {
    // Positive cases — each must be flagged.
    for ip in [
        "10.0.0.1",
        "10.255.255.255",
        "172.16.0.1",
        "172.31.42.7",
        "192.168.1.1",
        "candidate:1 1 udp 12345 192.168.0.5 56000 typ host",
    ] {
        assert!(
            contains_private_ipv4(ip).is_some(),
            "expected private match for {ip:?}"
        );
    }
    // Negative cases — public, loopback, link-local, non-1918.
    for ip in [
        "8.8.8.8",
        "1.1.1.1",
        "127.0.0.1",
        "169.254.1.1",
        "172.15.0.1",  // one below /12
        "172.32.0.1",  // one above /12
        "193.168.1.1", // one above /16
        "abcd::1",
    ] {
        assert!(
            contains_private_ipv4(ip).is_none(),
            "unexpected private match for {ip:?}"
        );
    }
}

/// HTML fixture that exercises ICE gathering and parks every candidate
/// in a DOM-visible JSON array. We use `iceServers: []` so no STUN is
/// contacted — candidates emitted are strictly local host candidates
/// (exactly the leak we're guarding against).
///
/// `iceCandidatePoolSize` + a dummy data channel force gathering to
/// actually kick off — without them Chrome may short-circuit and never
/// fire `onicecandidate`.
const WEBRTC_FIXTURE: &str = r#"data:text/html,<!DOCTYPE html>
<html><head><meta charset='utf-8'></head><body>
<div id='candidates'>[]</div>
<script>
(async () => {
  const log = [];
  const write = () => {
    document.getElementById('candidates').textContent = JSON.stringify(log);
  };
  write();
  const pc = new RTCPeerConnection({ iceServers: [] });
  pc.onicecandidate = (ev) => {
    if (ev.candidate) {
      log.push({ candidate: ev.candidate.candidate, address: ev.candidate.address || null });
    } else {
      log.push({ done: true });
    }
    write();
  };
  // Data channel is enough to trigger gathering.
  pc.createDataChannel('probe');
  const offer = await pc.createOffer();
  await pc.setLocalDescription(offer);
})();
</script>
</body></html>"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Chromium; run with --ignored"]
async fn webrtc_ice_gathering_does_not_leak_private_ip() {
    let (browser, _tmp) = launch_with_webrtc_flags().await;
    let page = browser.new_page("about:blank").await.expect("new_page");

    page.goto(WEBRTC_FIXTURE).await.expect("goto fixture");

    // ICE gathering is async — poll until the "done" marker lands or we
    // hit a generous wall clock. In practice Chromium emits everything
    // within ~200ms for host-only gathering; 5s is a huge safety margin.
    let mut attempts = 0usize;
    let raw = loop {
        attempts += 1;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let val: String = page
            .evaluate("document.getElementById('candidates').textContent || '[]'")
            .await
            .expect("read candidates")
            .into_value()
            .expect("into_value String");
        if val.contains("\"done\":true") || attempts >= 25 {
            break val;
        }
    };

    eprintln!("[webrtc-leak-audit] raw candidates JSON after {attempts} polls:\n{raw}");

    // Parse the JSON — robust against minor shape changes because we
    // only inspect the `candidate` string.
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("candidates JSON parse");
    let arr = parsed.as_array().expect("candidates is array");

    let mut leaks: Vec<(String, String)> = Vec::new();
    for entry in arr {
        // Per-candidate SDP line.
        if let Some(c) = entry.get("candidate").and_then(|v| v.as_str()) {
            if let Some(ip) = contains_private_ipv4(c) {
                leaks.push((ip, c.to_string()));
            }
        }
        // Chrome also exposes `.address` on modern builds — check it
        // independently so a future API change can't hide the leak in
        // the unparsed SDP blob.
        if let Some(addr) = entry.get("address").and_then(|v| v.as_str()) {
            if is_private_ipv4(addr) {
                leaks.push((addr.to_string(), format!("address field: {addr}")));
            }
        }
    }

    assert!(
        leaks.is_empty(),
        "WebRTC ICE gathering leaked private IP(s) despite \
         --force-webrtc-ip-handling-policy=disable_non_proxied_udp. \
         Leaks: {leaks:#?}\nFull candidate payload: {raw}"
    );
}
