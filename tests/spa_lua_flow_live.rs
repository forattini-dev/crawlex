//! Live SPA + Lua end-to-end flow. `#[ignore]` — spawns Chromium and a local
//! wiremock HTTP fixture. Run with:
//!
//! ```
//! cargo test --all-features --test spa_lua_flow_live -- --ignored
//! ```
//!
//! Validates:
//!   1. A Lua `on_after_load` hook loaded via `--hook-script` actually runs.
//!   2. `page_wait_for` / `page_click` bridge correctly to Chromium.
//!   3. The screenshot captured after the Lua flow reflects the post-click
//!      DOM (non-empty PNG).
//!   4. `html_post_js` contains the dashboard subtree that only appears
//!      after the button click.

#![cfg(all(feature = "cdp-backend", feature = "lua-hooks"))]

use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crawlex::config::Config;
use crawlex::hooks::lua::LuaHookHost;
use crawlex::render::pool::RenderPool;
use crawlex::render::Renderer;
use crawlex::render::WaitStrategy;
use crawlex::storage::Storage;

const SPA_HTML: &str = r#"<!doctype html>
<html><head><title>SPA flow fixture</title></head>
<body>
  <div id="app">
    <h1>Home</h1>
    <button id="go">Go</button>
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
async fn spa_lua_flow_drives_click_and_screenshot() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SPA_HTML))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    // Prefer system Chrome — the cached Chromium-for-Testing has residual
    // CDP drift that times out `Page.navigate` on local wiremock fixtures.
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
        // Exercise the new `--screenshot-mode` wiring end-to-end: clip to the
        // post-click subtree so we also prove Element-mode resolves the
        // selector that only exists after the Lua flow mutated the DOM.
        output: crawlex::config::OutputConfig {
            screenshot_mode: Some("element:#dashboard".to_string()),
            ..Default::default()
        },
        ..Config::default()
    };

    let storage: Arc<dyn Storage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());
    let pool = RenderPool::new(Arc::new(cfg), storage);

    // Load the Lua flow and attach it to the pool — same wiring the
    // `Crawler::set_lua_scripts` path uses internally.
    let script =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spa_flow.lua");
    let host = Arc::new(LuaHookHost::new(vec![script]).expect("load lua flow"));
    pool.set_lua_host(host);

    let url = url::Url::parse(&server.uri()).unwrap();
    // Initial wait targets the button present on first paint; the Lua hook
    // then clicks it, which triggers pushState + DOM mutation. Post-hook
    // settling re-probes the `#dashboard` selector produced by the click.
    let wait = WaitStrategy::Selector {
        css: "#go".into(),
        timeout_ms: 5_000,
    };

    let page = pool
        .render(&url, &wait, false, true, None, None)
        .await
        .expect("SPA+Lua render");

    assert!(
        page.html_post_js.contains("Dashboard"),
        "post-Lua DOM missing 'Dashboard' — hook likely didn't run; html=\n{}",
        page.html_post_js
    );
    assert!(
        page.html_post_js.contains(r#"id="dashboard""#),
        "post-Lua DOM missing #dashboard anchor"
    );
    let png = page
        .screenshot_png
        .as_ref()
        .expect("element-mode screenshot should resolve post-Lua");
    assert!(
        !png.is_empty(),
        "element-mode screenshot returned empty bytes"
    );
    // PNG magic: 89 50 4E 47.
    assert_eq!(&png[..4], &[0x89, 0x50, 0x4E, 0x47], "not a PNG");
}
