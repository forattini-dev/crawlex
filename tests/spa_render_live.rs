//! Live SPA render smoke test. `#[ignore]` by default — requires Chromium
//! (auto-fetched via `chromium-fetcher` feature) and spins up a local HTTP
//! fixture. Run with:
//!
//! ```
//! cargo test --all-features --test spa_render_live -- --ignored
//! ```
//!
//! Validates the SPA-stateful contract end-to-end:
//!   1. Render navigates, runs initial wait strategy.
//!   2. Action DSL clicks a button that does `history.pushState` and
//!      mutates the DOM.
//!   3. `settle_after_actions` re-probes the new selector.
//!   4. `final_url` reflects the pushState route (not the initial URL).
//!   5. `html_post_js` contains the post-click DOM.

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crawlex::config::Config;
use crawlex::render::actions::Action;
use crawlex::render::pool::RenderPool;
use crawlex::render::Renderer;
use crawlex::render::WaitStrategy;
use crawlex::storage::Storage;

const SPA_HTML: &str = r#"<!doctype html>
<html><head><title>SPA fixture</title></head>
<body>
  <div id="app">
    <h1>Home</h1>
    <button id="go">Go to dashboard</button>
  </div>
  <script>
    document.getElementById('go').addEventListener('click', () => {
      history.pushState({}, '', '/dashboard');
      document.getElementById('app').innerHTML =
        '<h1 id="dashboard">Dashboard</h1><p>Welcome back.</p>';
    });
  </script>
</body></html>"#;

#[tokio::test]
#[ignore = "known-flaky with wiremock+Chromium timing; HN live test is the real-world proof. Tracked."]
async fn spa_click_pushstate_and_selector_reappears() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SPA_HTML))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    // Prefer system Chrome — the cached Chromium-for-Testing build has
    // residual CDP protocol drift that makes `Page.navigate` time out on
    // short-lived local fixtures. System Chrome is happy.
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
    let pool = RenderPool::new(Arc::new(cfg), storage);

    let url = url::Url::parse(&server.uri()).unwrap();
    // Initial wait = plain Load. The action script handles its own
    // `WaitFor(#go)` before clicking; settling after the click re-checks
    // `#dashboard` through its own ready-state probe.
    let wait = WaitStrategy::Load;
    let actions = vec![
        Action::WaitFor {
            selector: "#go".into(),
            timeout_ms: 3_000,
        },
        Action::Click {
            selector: "#go".into(),
        },
    ];

    let page = pool
        .render(&url, &wait, false, false, Some(&actions), None)
        .await
        .expect("SPA render");

    assert!(
        page.final_url.path().contains("/dashboard"),
        "expected pushState route in final_url, got {}",
        page.final_url
    );
    assert!(
        page.html_post_js.contains("Dashboard"),
        "post-click DOM missing 'Dashboard'"
    );
    assert!(
        page.html_post_js.contains(r#"id="dashboard""#),
        "post-click DOM missing #dashboard anchor"
    );
}
