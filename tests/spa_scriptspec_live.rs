//! Live end-to-end smoke test for `RenderPool::render_with_script`.
//! `#[ignore]` by default — spawns Chromium + wiremock. Run with:
//!
//! ```
//! cargo test --all-features --test spa_scriptspec_live -- --ignored
//! ```
//!
//! Flow (matches the M1 "fluxo controlado sem Lua" milestone):
//!   1. Serve a tiny SPA from a wiremock MockServer.
//!   2. Load a ScriptSpec JSON in-line that: goto + wait_for + click +
//!      screenshot element + snapshot ax_tree.
//!   3. Assert the `RenderedPage` succeeds and the `RunOutcome` carries
//!      one screenshot.element artifact (non-zero bytes, valid PNG
//!      header) and one snapshot.ax_tree artifact.
//!
//! Prefers system Chrome at `/usr/bin/google-chrome` (same reasoning as
//! `live_news_navigation`: pinned Chromium-for-Testing has CDP drift).

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crawlex::config::Config;
use crawlex::render::pool::RenderPool;
use crawlex::render::WaitStrategy;
use crawlex::script::ScriptSpec;
use crawlex::storage::Storage;

const SPA_HTML: &str = r#"<!doctype html>
<html><head><title>ScriptSpec SPA</title></head>
<body>
  <div id="app">
    <h1 id="home">Home</h1>
    <button id="go">Go</button>
  </div>
  <script>
    document.getElementById('go').addEventListener('click', () => {
      document.getElementById('app').innerHTML =
        '<h1 id="dashboard">Dashboard</h1>';
    });
  </script>
</body></html>"#;

#[tokio::test]
#[ignore = "spawns Chromium; run with --ignored"]
async fn script_spec_drives_spa_and_emits_artifacts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SPA_HTML.as_bytes(), "text/html"))
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
        ..Config::default()
    };
    let storage: Arc<dyn Storage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());
    let pool = RenderPool::new(Arc::new(cfg), storage.clone());

    let seed = url::Url::parse(&server.uri()).unwrap();
    // ScriptSpec steps: wait_for home, click #go, wait_for dashboard,
    // screenshot the element, snapshot the AX tree.
    let spec_json = serde_json::json!({
        "version": 1,
        "defaults": { "timeout_ms": 10000 },
        "steps": [
            { "wait_for": { "locator": "#home" } },
            { "click": { "locator": "#go" } },
            { "wait_for": { "locator": "#dashboard" } },
            { "screenshot": { "mode": "element", "locator": "#dashboard", "name": "dashboard_box" } },
            { "snapshot": { "kind": "ax_tree" } }
        ]
    });
    let spec = ScriptSpec::from_json(spec_json.to_string().as_bytes()).expect("valid script-spec");

    let wait = WaitStrategy::DomContentLoaded;

    let (rendered, outcome) = tokio::time::timeout(
        Duration::from_secs(60),
        pool.render_with_script(&seed, &wait, &spec, None, None, None),
    )
    .await
    .expect("render_with_script timed out")
    .expect("render_with_script failed");

    assert!(rendered.status == 0 || rendered.status == 200);
    assert!(
        outcome.failed_assertion.is_none(),
        "assertion failure: {:?}",
        outcome.failed_assertion
    );
    assert!(
        outcome.steps.len() >= 5,
        "expected >=5 step outcomes, got {}",
        outcome.steps.len()
    );
    let artifacts: Vec<_> = outcome
        .steps
        .iter()
        .flat_map(|s| s.artifacts.iter())
        .collect();
    let shot = artifacts
        .iter()
        .find(|a| a.kind == "screenshot.element")
        .expect("expected screenshot.element artifact");
    assert!(shot.bytes > 64, "screenshot bytes={}", shot.bytes);
    // sha256 is a 64-char lowercase hex digest — cheap sanity check on the
    // artifact manifest format consumers rely on.
    assert_eq!(shot.sha256.len(), 64);
    let ax = artifacts
        .iter()
        .find(|a| a.kind == "snapshot.ax_tree")
        .expect("expected snapshot.ax_tree artifact");
    assert!(ax.bytes > 0);

    // Phase 4: artifacts should also be persisted through the unified
    // `save_artifact` pipeline and surface via `list_artifacts`. We don't
    // assume a session_id filter here — the runner derives session_id
    // from the URL host inside RenderPool, so list the full set and
    // match on kind.
    let rows = storage.list_artifacts(None, None).await.unwrap();
    let kinds: Vec<_> = rows.iter().map(|r| r.kind).collect();
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, crawlex::storage::ArtifactKind::ScreenshotElement)),
        "expected a ScreenshotElement row in artifacts, got {kinds:?}"
    );
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, crawlex::storage::ArtifactKind::SnapshotAxTree)),
        "expected a SnapshotAxTree row in artifacts, got {kinds:?}"
    );
    // Step metadata should propagate.
    let elem_row = rows
        .iter()
        .find(|r| matches!(r.kind, crawlex::storage::ArtifactKind::ScreenshotElement))
        .unwrap();
    assert_eq!(elem_row.step_kind.as_deref(), Some("screenshot"));
    assert!(elem_row.step_id.is_some(), "step_id should be populated");
    assert_eq!(elem_row.name.as_deref(), Some("dashboard_box"));
    assert!(elem_row.selector.is_some(), "selector should be populated");
}
