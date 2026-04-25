//! Live verification that the stealth shim flows into Web Worker scopes
//! (Camoufox port Sprint 3 S3.1).
//!
//! The fingerprint vector: detector spawns `new Worker(...)` and queries
//! `navigator.userAgent` (or `AudioContext.sampleRate`, etc.) from inside.
//! Without S3.1 the worker reports the **native** UA — i.e. the unpatched
//! Chrome string the binary actually has on disk — while the main thread
//! reports our persona-chosen UA. The mismatch is a single-bit detector.
//!
//! S3.1 wires `Target.attachedToTarget` → `Runtime.evaluate(WORKER_SHIM)`
//! → `Runtime.runIfWaitingForDebugger` so the worker's `navigator.userAgent`
//! getter is overridden before any user script in the worker runs.
//!
//! This test launches a real Chrome via `BrowserConfig`, configures the
//! worker shim through `stealth_worker_shim`, navigates to a data URL
//! that spawns a Worker, and asserts the worker's reported UA matches
//! the persona we injected.
//!
//! Ignored by default — needs Chromium. Run with:
//!
//! ```
//! cargo test --all-features --test worker_shim_live -- --ignored --nocapture
//! ```

#![cfg(feature = "cdp-backend")]

use std::time::Duration;

use crawlex::identity::IdentityBundle;
use crawlex::render::chrome::browser::{Browser, BrowserConfig, HeadlessMode};
use crawlex::render::stealth::render_worker_shim_from_bundle;
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

async fn launch_with_worker_shim(worker_src: String) -> (Browser, tempfile::TempDir) {
    let exec = system_chrome().expect("requires system Chrome installed");
    let tmp = tempfile::tempdir().expect("tmp user_data_dir");
    let cfg = BrowserConfig::builder()
        .chrome_executable(exec)
        .headless_mode(HeadlessMode::New)
        .no_sandbox()
        .user_data_dir(tmp.path())
        .stealth_worker_shim(worker_src)
        .build()
        .expect("build browser config");
    let (browser, mut handler) = Browser::launch(cfg).await.expect("launch browser");
    tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
    (browser, tmp)
}

/// Build a data URL HTML page that spawns a dedicated Web Worker and
/// posts the worker-side `navigator.userAgent` back to the main thread.
/// The result lands on `document.title` so the Rust side can poll it
/// without a CDP runtime binding.
fn data_url_with_worker_probe() -> String {
    // Worker source: report navigator.userAgent + AudioContext.sampleRate.
    // We base64-encode the worker via a Blob URL constructed in main HTML.
    let html = r#"<!doctype html>
<meta charset="utf-8">
<title>pending</title>
<script>
  const code = `
    self.onmessage = function () {
      try {
        const ua = (typeof navigator !== 'undefined' && navigator.userAgent) || '';
        let sr = 0;
        try {
          const AC = self.OfflineAudioContext || self.webkitOfflineAudioContext;
          if (AC) sr = new AC(1, 1, 44100).sampleRate;
        } catch (e) {}
        self.postMessage(JSON.stringify({ ua: ua, sr: sr }));
      } catch (e) {
        self.postMessage(JSON.stringify({ err: String(e) }));
      }
    };
  `;
  const blob = new Blob([code], { type: 'application/javascript' });
  const url = URL.createObjectURL(blob);
  const w = new Worker(url);
  w.onmessage = (ev) => { document.title = ev.data || 'empty'; };
  w.postMessage('go');
</script>
"#;
    let mut encoded = String::with_capacity(html.len() * 3);
    for byte in html.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                encoded.push(*byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{:02X}", byte));
            }
        }
    }
    format!("data:text/html;charset=utf-8,{encoded}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Chromium; run with --ignored"]
async fn worker_navigator_user_agent_matches_persona() {
    let bundle = IdentityBundle::from_chromium(131, 0xc0_ffee_dead_beef);
    let worker_src = render_worker_shim_from_bundle(&bundle);
    let persona_ua = bundle.ua.clone();
    let persona_sr = bundle.audio_sample_rate;

    let (browser, _tmp) = launch_with_worker_shim(worker_src).await;
    let target = data_url_with_worker_probe();
    let page = browser.new_page(target.as_str()).await.expect("new_page");

    // Poll document.title until the worker has replied (or 10s ceiling).
    let mut payload: Option<String> = None;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(eval) = page.evaluate("document.title").await {
            if let Some(t) = eval.into_value::<String>().ok() {
                if t != "pending" && !t.is_empty() {
                    payload = Some(t);
                    break;
                }
            }
        }
    }
    let payload = payload.expect("worker did not reply within 10s");
    assert!(
        payload.contains(&persona_ua),
        "worker navigator.userAgent must equal persona UA. \
         persona = {persona_ua:?} ; worker payload = {payload:?}"
    );
    assert!(
        payload.contains(&persona_sr.to_string()),
        "worker AudioContext.sampleRate must equal persona ({persona_sr}). \
         worker payload = {payload:?}"
    );
}
