//! Hash-only SPA routing live test. `#[ignore]` — spawns Chromium +
//! wiremock. Run with:
//!
//! ```
//! cargo test --all-features --test spa_hash_routing_live -- --ignored
//! ```
//!
//! Validates that `RenderedPage::final_url` reflects hash routing
//! (`#/dashboard`) after a client-side `location.hash` transition.
//!
//! Regression: CDP `page.url()` pulls from targetInfo which *does not*
//! update on hash changes — only full navigations trigger targetInfo
//! updates. A SPA that swaps routes via `#/route` would keep reporting
//! the seed URL as `final_url`. `RenderPool::render` therefore evaluates
//! `window.location.href` and prefers it.

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

// Hash-only SPA: the button assigns `location.hash = '#/dashboard'`,
// which does NOT trigger a navigation or targetInfo update. The DOM
// mutates under the existing document, and `final_url` must still end
// with `#/dashboard`.
const HASH_SPA_HTML: &str = r#"<!doctype html>
<html><head><title>Hash SPA</title></head>
<body>
  <div id="app">
    <h1>Home</h1>
    <button id="go">Go</button>
  </div>
  <script>
    function render() {
      if (location.hash === '#/dashboard') {
        document.getElementById('app').innerHTML =
          '<h1 id="dashboard">Dashboard</h1>';
      }
    }
    window.addEventListener('hashchange', render);
    document.getElementById('go').addEventListener('click', () => {
      location.hash = '#/dashboard';
    });
    render();
  </script>
</body></html>"#;

#[tokio::test]
#[ignore = "spawns Chromium; run with --ignored"]
async fn hash_only_route_updates_final_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HASH_SPA_HTML))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        max_concurrent_render: 1,
        auto_fetch_chromium: true,
        ..Config::default()
    };
    let storage: Arc<dyn Storage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());
    let pool = RenderPool::new(Arc::new(cfg), storage);

    let url = url::Url::parse(&server.uri()).unwrap();
    let wait = WaitStrategy::Selector {
        css: "#dashboard".into(),
        timeout_ms: 8_000,
    };
    let actions = vec![
        Action::WaitFor {
            selector: "#go".into(),
            timeout_ms: 5_000,
        },
        Action::Click {
            selector: "#go".into(),
        },
    ];

    let page = pool
        .render(&url, &wait, false, false, Some(&actions), None)
        .await
        .expect("SPA render");

    let fu = page.final_url.as_str();
    assert!(
        fu.contains("#/dashboard"),
        "expected hash route '#/dashboard' in final_url, got {fu}"
    );
}
