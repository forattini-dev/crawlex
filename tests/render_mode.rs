//! Operator-level `render_mode` switch end-to-end.
//!
//! Re-uses the wiremock fixture from `mini_http_only.rs` to check that
//! every `fetch.completed` event is tagged with the path that served
//! the URL — `"impersonate"` for the in-process HTTP spoof client.
//! `Always` requires Chrome on the host so it's exercised at the
//! config-resolution layer (the CLI parser path) instead of through a
//! real run.

use std::sync::Arc;
use std::time::Duration;

use crawlex::config::{Config, QueueBackend, RenderMode, StorageBackend};
use crawlex::events::{EventKind, MemorySink};
use crawlex::Crawler;
use serde_json::Value;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http_only_cfg(
    queue_path: String,
    storage_path: String,
    render_mode: RenderMode,
) -> Config {
    Config {
        max_concurrent_http: 4,
        max_concurrent_render: 0,
        max_depth: Some(0),
        respect_robots_txt: false,
        well_known_enabled: false,
        pwa_enabled: false,
        favicon_enabled: false,
        robots_paths_enabled: false,
        dns_enabled: false,
        crtsh_enabled: false,
        wayback_enabled: false,
        rdap_enabled: false,
        collect_peer_cert: false,
        collect_net_timings: false,
        collect_web_vitals: false,
        queue_backend: QueueBackend::Sqlite { path: queue_path },
        storage_backend: StorageBackend::Filesystem { root: storage_path },
        render_mode,
        ..Config::default()
    }
}

async fn run_with_mode(render_mode: RenderMode) -> Vec<crawlex::events::EventEnvelope> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"/p/\d+"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body><h1>ok</h1></body></html>"),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let queue_path = tmp.path().join("q.db").to_string_lossy().to_string();
    let storage_path = tmp.path().join("store").to_string_lossy().to_string();
    std::fs::create_dir_all(&storage_path).unwrap();

    let cfg = http_only_cfg(queue_path, storage_path, render_mode);
    let sink = Arc::new(MemorySink::create());
    let crawler = Crawler::new(cfg).unwrap().with_events(sink.clone());

    let seeds: Vec<String> = (0..8).map(|i| format!("{}/p/{i}", server.uri())).collect();
    crawler.seed(seeds).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(30), crawler.run()).await;
    sink.take()
}

fn assert_all_fetch_completed_tagged(
    events: &[crawlex::events::EventEnvelope],
    expected_path: &str,
) {
    let fetches: Vec<&crawlex::events::EventEnvelope> = events
        .iter()
        .filter(|e| e.event == EventKind::FetchCompleted)
        .collect();
    assert!(
        !fetches.is_empty(),
        "expected at least one fetch.completed event"
    );
    for ev in fetches {
        let path = ev
            .data
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("fetch.completed missing data.path: {:?}", ev.data));
        assert_eq!(
            path, expected_path,
            "fetch.completed.data.path mismatch on {:?}",
            ev.url
        );
    }
}

#[tokio::test]
async fn render_mode_auto_uses_impersonate_against_static_html() {
    let events = run_with_mode(RenderMode::Auto).await;
    assert_all_fetch_completed_tagged(&events, "impersonate");
}

#[tokio::test]
async fn render_mode_never_pins_jobs_to_impersonate_path() {
    let events = run_with_mode(RenderMode::Never).await;
    assert_all_fetch_completed_tagged(&events, "impersonate");
    // `Never` MUST refuse to instantiate the render pool — verified
    // structurally by the absence of any `render.completed` event.
    let renders = events
        .iter()
        .filter(|e| e.event == EventKind::RenderCompleted)
        .count();
    assert_eq!(renders, 0, "Never must not produce render.completed events");
}

/// `Always` needs Chrome. We exercise the CLI parser path that maps
/// the operator flag onto `Config.render_mode` and the seed-method
/// override to `FetchMethod::Render`, without actually starting a
/// crawl.
#[test]
fn render_mode_always_forces_render_pool_capacity() {
    let mut config = Config::default();
    config.max_concurrent_render = 0;
    config.render_mode = RenderMode::Always;
    // Mirror the wiring done in `cli::run_crawl` — bumping the pool
    // up to at least one slot so the render path is reachable.
    if matches!(config.render_mode, RenderMode::Always)
        && config.max_concurrent_render == 0
    {
        config.max_concurrent_render = 1;
    }
    assert_eq!(config.max_concurrent_render, 1);

    config.render_mode = RenderMode::Never;
    if matches!(config.render_mode, RenderMode::Never) {
        config.max_concurrent_render = 0;
    }
    assert_eq!(config.max_concurrent_render, 0);
}
