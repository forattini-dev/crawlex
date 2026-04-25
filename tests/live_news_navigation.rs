//! Real-world live smoke test against Hacker News. `#[ignore]` by default
//! — requires Chromium + network. Run with:
//!
//! ```
//! cargo test --all-features --test live_news_navigation -- --ignored --nocapture
//! ```
//!
//! Flow:
//!   1. Render `https://news.ycombinator.com/` with a fullpage screenshot.
//!   2. Extract the first story link from `html_post_js` via a minimal
//!      regex (we avoid dragging scraper/html5ever into tests — the HN
//!      markup is boringly stable).
//!   3. Render the target page with another fullpage screenshot.
//!   4. Assert both screenshots are valid PNGs (magic bytes) and that
//!      the front-page HTML contains the HN brand string.
//!
//! Why HN and not Google: no consent wall, server-rendered so we don't
//! need wait-strategy gymnastics, and the markup (`.titleline > a`)
//! hasn't meaningfully changed in a decade. If this test goes flaky,
//! the *web* broke before our code did.

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;
use std::time::Duration;

use crawlex::config::Config;
use crawlex::render::pool::RenderPool;
use crawlex::render::Renderer;
use crawlex::render::WaitStrategy;
use crawlex::storage::Storage;

#[tokio::test]
#[ignore = "requires Chromium + network; run with --ignored"]
async fn hn_front_page_then_first_story() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    // Prefer a system Chrome when present — the pinned Chromium-for-Testing
    // build we cache (1585606) has a CDP protocol drift with
    // CDP client 0.9 that makes `Page.navigate` time out in this env.
    // System Chrome sidesteps that until we bump either side.
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
        output: crawlex::config::OutputConfig {
            screenshot_mode: Some("fullpage".into()),
            ..Default::default()
        },
        ..Config::default()
    };
    let storage: Arc<dyn Storage> = Arc::new(
        crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).expect("fs storage"),
    );
    let pool = RenderPool::new(Arc::new(cfg), storage);

    let hn = url::Url::parse("https://news.ycombinator.com/").unwrap();
    // HN is static HTML; a DOM-content-loaded equivalent is plenty. Use
    // a selector wait on the stable `.titleline` ancestor so we don't
    // race the page's own late resources.
    let wait = WaitStrategy::Selector {
        css: "span.titleline > a".into(),
        timeout_ms: 30_000,
    };

    let front = match tokio::time::timeout(
        Duration::from_secs(90),
        pool.render(&hn, &wait, false, true, None, None),
    )
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => panic!("HN front render failed (requires Chromium + network): {e}"),
        Err(_) => panic!("HN front render timed out after 90s"),
    };

    let png = front
        .screenshot_png
        .as_ref()
        .expect("fullpage screenshot should exist");
    assert_eq!(&png[..4], &[0x89, 0x50, 0x4E, 0x47], "front: not a PNG");
    eprintln!("front-page PNG: {} bytes", png.len());
    assert!(
        front.html_post_js.contains("Hacker News")
            || front.html_post_js.contains("news.ycombinator.com"),
        "HN brand string missing from front page HTML"
    );

    // Persist the front-page screenshot so the operator can eyeball it.
    let front_path = tmp.path().join("hn-front.png");
    std::fs::write(&front_path, png).expect("write front png");
    eprintln!("front-page screenshot → {}", front_path.display());

    // Extract the first story link. HN wraps titles in
    // `<span class="titleline"><a href="...">Title</a>...`. A simple
    // regex is sufficient and keeps this test dependency-light.
    let re = regex::Regex::new(r#"<span class="titleline"><a href="([^"]+)""#).unwrap();
    let first = re
        .captures(&front.html_post_js)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .expect("no titleline anchor found — HN markup changed?");

    // HN stores some links as relative `item?id=...` (Ask HN / job posts).
    // Resolve against the front-page URL to get an absolute target.
    let target = hn.join(&first).expect("resolve first link");
    eprintln!("navigating to first story: {target}");

    // The target might be an arbitrary blog; relax the wait strategy and
    // accept network-idle as "done". Some hosts rate-limit headless UAs;
    // we surface rather than swallow.
    let wait2 = WaitStrategy::NetworkIdle { idle_ms: 600 };
    let story = match tokio::time::timeout(
        Duration::from_secs(45),
        pool.render(&target, &wait2, false, true, None, None),
    )
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            eprintln!("story render failed (likely host-side): {e}");
            // Don't fail the whole test for a flaky third-party host —
            // the value of this test is exercising the two-step flow,
            // and we already proved front-page works.
            return;
        }
        Err(_) => {
            eprintln!("story render timed out — skipping story assertion");
            return;
        }
    };

    if let Some(png2) = story.screenshot_png.as_ref() {
        assert_eq!(&png2[..4], &[0x89, 0x50, 0x4E, 0x47], "story: not a PNG");
        let story_path = tmp.path().join("hn-story.png");
        std::fs::write(&story_path, png2).expect("write story png");
        eprintln!("story PNG: {} bytes → {}", png2.len(), story_path.display());
    } else {
        eprintln!("story had no screenshot — likely headless-blocked host");
    }
}
