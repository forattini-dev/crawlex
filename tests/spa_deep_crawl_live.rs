//! Live end-to-end test for the Fase 3 SPA/PWA deep-crawl pipeline.
//! `#[ignore]` by default — spawns a real Chrome and a wiremock
//! server. Run with:
//!
//! ```
//! cargo test --all-features --test spa_deep_crawl_live -- --ignored
//! ```
//!
//! Exercises the full observer + collectors + artifact path:
//!   1. Serve an SPA that on button click fires `history.pushState`
//!      to `#/dashboard` and `fetch`es `/api/items`.
//!   2. Drive the interaction via a ScriptSpec (goto → wait_for →
//!      click → wait_for).
//!   3. After render, assert:
//!      - `rendered.is_spa == true`
//!      - `rendered.runtime_routes` contains the pushed route
//!      - `rendered.network_endpoints` contains the `/api/items` URL
//!      - `list_artifacts` surfaces `SnapshotRuntimeRoutes` AND
//!        `SnapshotNetworkEndpoints` rows with non-empty JSON bodies.
//!
//! Prefers system Chrome (same reason as the other live tests: the
//! pinned Chromium-for-Testing has CDP drift on some versions).

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;
use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crawlex::config::Config;
use crawlex::render::pool::RenderPool;
use crawlex::render::WaitStrategy;
use crawlex::script::ScriptSpec;
use crawlex::storage::{ArtifactKind, Storage};

const SPA_HTML: &str = r#"<!doctype html>
<html><head><title>Deep Crawl SPA</title></head>
<body>
  <div id="app">
    <h1 id="home">Home</h1>
    <button id="go">Go</button>
  </div>
  <script>
    document.getElementById('go').addEventListener('click', async () => {
      try {
        history.pushState({ p: 1 }, '', '/dashboard');
        await fetch('/api/items', { method: 'GET' });
      } catch (_) {}
      document.getElementById('app').innerHTML =
        '<h1 id="dashboard">Dashboard</h1>';
    });
  </script>
</body></html>"#;

#[tokio::test]
#[ignore = "spawns Chromium; run with --ignored"]
async fn spa_deep_crawl_emits_runtime_artifacts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SPA_HTML.as_bytes(), "text/html"))
        .mount(&server)
        .await;
    // SPA push target resolves on hard-reload too.
    Mock::given(method("GET"))
        .and(path("/dashboard"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SPA_HTML.as_bytes(), "text/html"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/items"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(br#"{"items":["a","b","c"]}"#, "application/json"),
        )
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
        collect_runtime_routes: true,
        collect_network_endpoints: true,
        ..Config::default()
    };
    let storage: Arc<dyn Storage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());
    let pool = RenderPool::new(Arc::new(cfg), storage.clone());

    let seed = url::Url::parse(&server.uri()).unwrap();
    let spec_json = serde_json::json!({
        "version": 1,
        "defaults": { "timeout_ms": 10000 },
        "steps": [
            { "wait_for": { "locator": "#home" } },
            { "click": { "locator": "#go" } },
            { "wait_for": { "locator": "#dashboard" } }
        ]
    });
    let spec = ScriptSpec::from_json(spec_json.to_string().as_bytes()).expect("valid script-spec");

    let wait = WaitStrategy::DomContentLoaded;

    let (rendered, _outcome) = tokio::time::timeout(
        Duration::from_secs(60),
        pool.render_with_script(&seed, &wait, &spec, None, None, None),
    )
    .await
    .expect("render_with_script timed out")
    .expect("render_with_script failed");

    assert!(rendered.status == 0 || rendered.status == 200);
    assert!(
        rendered.is_spa,
        "expected is_spa=true after pushState; routes={:?}",
        rendered.runtime_routes
    );
    assert!(
        rendered
            .runtime_routes
            .iter()
            .any(|u| u.path() == "/dashboard"),
        "expected /dashboard route in runtime_routes, got {:?}",
        rendered.runtime_routes
    );
    assert!(
        rendered
            .network_endpoints
            .iter()
            .any(|u| u.path() == "/api/items"),
        "expected /api/items in network_endpoints, got {:?}",
        rendered.network_endpoints
    );
    // /api/items MUST also land in captured_urls so the crawler
    // frontier picks it up without any extra wiring.
    assert!(
        rendered
            .captured_urls
            .iter()
            .any(|u| u.path() == "/api/items"),
        "expected /api/items to feed captured_urls, got {:?}",
        rendered.captured_urls
    );

    // Artifacts wired through `save_artifact` must surface for the
    // session. We don't assume a session_id filter — list all and
    // match on kind.
    let rows = storage.list_artifacts(None, None).await.unwrap();
    let kinds: Vec<_> = rows.iter().map(|r| r.kind).collect();
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, ArtifactKind::SnapshotRuntimeRoutes)),
        "expected SnapshotRuntimeRoutes row, got {kinds:?}"
    );
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, ArtifactKind::SnapshotNetworkEndpoints)),
        "expected SnapshotNetworkEndpoints row, got {kinds:?}"
    );
    // Sanity-check at least one row has non-trivial bytes.
    let routes_row = rows
        .iter()
        .find(|r| matches!(r.kind, ArtifactKind::SnapshotRuntimeRoutes))
        .unwrap();
    assert!(
        routes_row.size > 8,
        "routes artifact size={}",
        routes_row.size
    );
    let endpoints_row = rows
        .iter()
        .find(|r| matches!(r.kind, ArtifactKind::SnapshotNetworkEndpoints))
        .unwrap();
    assert!(
        endpoints_row.size > 8,
        "endpoints artifact size={}",
        endpoints_row.size
    );
}
