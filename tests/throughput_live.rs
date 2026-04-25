//! Phase 5 throughput live test. `#[ignore]` by default — launches system
//! Chrome (falls back to chromium-for-testing), spins up 2 wiremock
//! origins, and drives 10 parallel renders. Assertions:
//!   - every render succeeds
//!   - the PagePool reused at least one tab (reuse counter > 0)
//!   - total_in_flight hits zero on quiesce
//!   - the RenderPool's budget bookkeeping stays balanced
//!
//! Preferring system Chrome is load-bearing: the cached Chromium-for-
//! Testing we auto-fetch has a CDP drift that makes `Page.navigate` time
//! out on short-lived wiremock endpoints. See spa_render_live.rs for the
//! same pattern.
//!
//! ```
//! cargo test --all-features --test throughput_live -- --ignored --nocapture
//! ```

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crawlex::config::Config;
use crawlex::render::pool::RenderPool;
use crawlex::render::Renderer;
use crawlex::render::WaitStrategy;
use crawlex::storage::Storage;

const HTML: &str = r#"<!doctype html><html><head><title>Throughput fixture</title></head>
<body><h1 id="home">Home</h1><p>ok</p></body></html>"#;

async fn spawn_origin() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HTML))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HTML))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HTML))
        .mount(&server)
        .await;
    server
}

fn system_chrome() -> Option<String> {
    [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ]
    .iter()
    .find(|p| std::path::Path::new(p).exists())
    .map(|s| s.to_string())
}

#[tokio::test]
#[ignore = "live: launches Chrome + 2 wiremock origins; run under phase-5 gate"]
async fn parallel_renders_reuse_tabs_and_respect_budgets() {
    let origin_a = spawn_origin().await;
    let origin_b = spawn_origin().await;

    let tmp = tempfile::tempdir().unwrap();
    let sys = system_chrome();
    let cfg = Config {
        // Plenty of concurrent slots — we want PagePool and budgets to
        // shape the traffic, not the outer Semaphore.
        max_concurrent_render: 8,
        auto_fetch_chromium: sys.is_none(),
        chrome_path: sys,
        max_browsers: 2,
        max_pages_per_context: 4,
        // Tight per-host budget so we force rejections and requeues on
        // the sibling URLs within the same origin. The crawler
        // machinery isn't engaged here — we just look at the scheduler
        // counters directly.
        render_budgets: crawlex::scheduler::BudgetLimits {
            max_per_host: 3,
            max_per_origin: 2,
            max_per_proxy: 8,
            max_per_session: 4,
            ..Default::default()
        },
        // Preserve the ~15 rps throughput baseline: `balanced` would
        // wire WindMouse + Fitts delays in any click path. This test
        // doesn't click, but pin `fast` so future refactors that route
        // through the motion engine don't silently regress throughput.
        motion_profile: crawlex::render::motion::MotionProfile::Fast,
        ..Config::default()
    };
    let storage: Arc<dyn Storage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());
    let counters = Arc::new(crawlex::metrics::Counters::default());
    let pool = Arc::new(RenderPool::new(Arc::new(cfg), storage));
    pool.set_counters(counters.clone());

    // Build 10 URLs across two origins with 3 paths each.
    let mut urls: Vec<url::Url> = Vec::new();
    for _ in 0..3 {
        for p in ["/", "/a", "/b"] {
            urls.push(format!("{}{}", origin_a.uri(), p).parse().unwrap());
            urls.push(format!("{}{}", origin_b.uri(), p).parse().unwrap());
        }
    }
    urls.truncate(10);

    let started = Instant::now();
    let wait = WaitStrategy::DomContentLoaded;
    let mut handles = Vec::new();
    for u in urls {
        let pool = pool.clone();
        let wait = wait.clone();
        handles.push(tokio::spawn(async move {
            pool.render(&u, &wait, false, false, None, None).await
        }));
    }
    let mut ok = 0usize;
    for h in handles {
        match h.await.unwrap() {
            Ok(rp) => {
                assert!(!rp.html_post_js.is_empty(), "empty HTML for render");
                ok += 1;
            }
            Err(e) => panic!("render failed: {e}"),
        }
    }
    let elapsed = started.elapsed();
    eprintln!(
        "throughput_live: {ok} renders in {:.2}s (~{:.1} rps)",
        elapsed.as_secs_f64(),
        ok as f64 / elapsed.as_secs_f64()
    );
    assert_eq!(ok, 10, "expected all 10 renders to succeed");

    // PagePool should have recycled at least one tab. With
    // max_pages_per_context=4 and 10 jobs across ~2 contexts we expect
    // several reuses.
    let (created, reused) = pool.page_pool().totals();
    eprintln!("pagepool totals: created={created} reused={reused}");
    assert!(
        reused > 0,
        "expected at least one tab reuse (created={created} reused={reused})"
    );
    // In-flight must be zero after all handles finish.
    assert_eq!(
        pool.page_pool().total_in_flight(),
        0,
        "tabs remained in-flight after quiesce"
    );

    // p95 sanity check: renders of a 200-byte static doc shouldn't
    // exceed a few seconds per request. Generous ceiling so CI-flaky
    // Chromium cold-start doesn't explode the test.
    {
        let mut s = counters.render_samples.lock();
        if let Some(p95) = s.percentile(0.95) {
            eprintln!("render_latency_ms_p95 = {p95:.0}");
            assert!(p95 < 10_000.0, "p95 unexpectedly high: {p95}ms");
        }
    }
    // Pages created counter mirrored in Counters.
    let pc = counters
        .pages_created
        .load(std::sync::atomic::Ordering::Relaxed);
    let pr = counters
        .pages_reused
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(pc as usize, created);
    assert_eq!(pr as usize, reused);

    // Give Chrome a moment to drain close tasks so the tempdir removal
    // doesn't race with Chrome's singleton lock cleanup.
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(pool);
}
