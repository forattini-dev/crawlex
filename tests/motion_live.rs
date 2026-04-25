//! Live event-sequence integrity test for the motion engine.
//!
//! Serves a tiny page that logs every mouse event into `window.__events`.
//! Drives a ScriptSpec click through RenderPool, then reads the event log
//! back via `eval_js`. Asserts that the click was preceded by several
//! mousemove events and that mouseover + mousedown + mouseup + click fired
//! in the right order — the exact sequence modern antibot ML expects.
//!
//! `#[ignore]` by default — spawns Chromium + wiremock. Run with:
//!
//! ```
//! cargo test --all-features --test motion_live -- --ignored
//! ```

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crawlex::config::Config;
use crawlex::render::motion::MotionProfile;
use crawlex::render::pool::RenderPool;
use crawlex::render::WaitStrategy;
use crawlex::script::ScriptSpec;
use crawlex::storage::Storage;

const PAGE: &str = r#"<!doctype html>
<html><head><title>motion-live</title></head>
<body>
  <button id="target" style="position:absolute; left:300px; top:250px; width:120px; height:40px;">Click me</button>
  <pre id="log"></pre>
  <script>
    window.__events = [];
    const t = document.getElementById('target');
    for (const ty of ['mousemove','mouseover','mouseenter','mousedown','mouseup','click']) {
      t.addEventListener(ty, (e) => {
        window.__events.push({ type: ty, x: e.clientX, y: e.clientY, t: performance.now() });
      });
    }
    // Also track moves landing anywhere on the document so we can count
    // the full trajectory (not just the ones that hit the button).
    document.addEventListener('mousemove', (e) => {
      window.__events.push({ type: 'doc_mousemove', x: e.clientX, y: e.clientY, t: performance.now() });
    });
  </script>
</body></html>"#;

#[tokio::test]
#[ignore = "spawns Chromium; run with --ignored"]
async fn click_emits_full_event_sequence() {
    // Install Balanced so trajectory is long enough to assert "multiple
    // moves precede the click" — Fast would only emit a handful.
    MotionProfile::Balanced.set_active();

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(PAGE.as_bytes(), "text/html"))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let system_chrome = [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ]
    .iter()
    .find(|p| std::path::Path::new(p).exists())
    .map(|s| s.to_string());
    let cfg = Config {
        max_concurrent_render: 1,
        auto_fetch_chromium: system_chrome.is_none(),
        chrome_path: system_chrome,
        motion_profile: MotionProfile::Balanced,
        ..Config::default()
    };

    let storage: Arc<dyn Storage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());
    let pool = RenderPool::new(Arc::new(cfg), storage.clone());

    let seed = url::Url::parse(&server.uri()).unwrap();
    // Click #target then pull window.__events into a dom dump we can
    // parse post-hoc through an extract step.
    let spec_json = serde_json::json!({
        "version": 1,
        "defaults": { "timeout_ms": 10000 },
        "steps": [
            { "wait_for": { "locator": "#target" } },
            { "click": { "locator": "#target" } },
            { "extract": { "fields": { "events_json": { "locator": "JSON.stringify(window.__events)", "kind": "script" } } } }
        ]
    });
    let spec = ScriptSpec::from_json(spec_json.to_string().as_bytes()).expect("valid spec");

    let wait = WaitStrategy::DomContentLoaded;
    let (_rendered, outcome) = tokio::time::timeout(
        Duration::from_secs(60),
        pool.render_with_script(&seed, &wait, &spec, None, None, None),
    )
    .await
    .expect("render timed out")
    .expect("render failed");

    assert!(
        outcome.failed_assertion.is_none(),
        "assertion failure: {:?}",
        outcome.failed_assertion
    );

    // Pull the extract step's captured value from RunOutcome.captures.
    let events_json = outcome
        .captures
        .get("events_json")
        .and_then(|v| v.as_str())
        .expect("events_json capture missing");
    let events: Vec<serde_json::Value> =
        serde_json::from_str(events_json).expect("events_json is not a JSON array");

    let types: Vec<&str> = events
        .iter()
        .filter_map(|e| e.get("type").and_then(|t| t.as_str()))
        .collect();

    // 1. At least one mousemove fired (doc or target) before the first
    //    click — the load-bearing antibot assertion.
    let first_click = types.iter().position(|t| *t == "click");
    let first_click = first_click.expect("expected a click event");
    let move_count_before_click = types[..first_click]
        .iter()
        .filter(|t| matches!(**t, "mousemove" | "doc_mousemove"))
        .count();
    assert!(
        move_count_before_click >= 3,
        "click without enough preceding mousemoves (got {move_count_before_click}): types={types:?}"
    );

    // 2. Ordered mousedown → mouseup → click.
    let down = types
        .iter()
        .position(|t| *t == "mousedown")
        .expect("mousedown missing");
    let up = types
        .iter()
        .position(|t| *t == "mouseup")
        .expect("mouseup missing");
    assert!(down < up, "mousedown must precede mouseup");
    assert!(
        up < first_click || up == first_click - 1 || up == first_click,
        "mouseup must precede/precede-equal click"
    );

    // 3. A mouseover fired on the target before mousedown.
    let over = types.iter().position(|t| *t == "mouseover");
    assert!(
        over.is_some(),
        "mouseover missing — event sequence integrity broken"
    );
    assert!(
        over.unwrap() <= down,
        "mouseover must fire before mousedown (over={}, down={down})",
        over.unwrap()
    );
}
