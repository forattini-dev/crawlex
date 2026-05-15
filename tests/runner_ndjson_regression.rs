//! NDJSON event-stream regression trip wire (slice 0 of the JobRunner
//! extraction, GH forattini-dev/crawlex#16).
//!
//! Drives a small deterministic crawl against a wiremock fixture and
//! asserts that the sequence of emitted event kinds matches the
//! checked-in golden file under `tests/fixtures/`.
//!
//! This test is the contract every later slice must keep green. Event
//! payloads (`ts`, `run_id`, `url`, `data`) are intentionally NOT part of
//! the assertion — those carry non-deterministic content (timestamps,
//! random IDs, wiremock host port). Only the ordered list of
//! `EventKind` discriminants is compared.
//!
//! Update protocol:
//!   1. Confirm the new event sequence is intentional (per PRD #15, the
//!      NDJSON contract — names and order — must stay stable across the
//!      refactor).
//!   2. Run `UPDATE_GOLDEN=1 cargo test --test runner_ndjson_regression --all-features`
//!      to regenerate `tests/fixtures/runner_ndjson_golden.txt`.
//!   3. Commit the golden file diff alongside the code change.

use std::sync::Arc;
use std::time::Duration;

use crawlex::config::{Config, QueueBackend, StorageBackend};
use crawlex::events::MemorySink;
use crawlex::Crawler;
use serde::Serialize;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn golden_path() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.join("tests").join("fixtures").join("runner_ndjson_golden.txt")
}

fn deterministic_cfg(queue_path: String, storage_path: String) -> Config {
    Config {
        max_concurrent_http: 1,
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
        ..Config::default()
    }
}

#[tokio::test]
async fn runner_ndjson_event_sequence_matches_golden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/p/0"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string("<html><body><h1>ok</h1></body></html>"),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let queue_path = tmp.path().join("q.db").to_string_lossy().to_string();
    let storage_path = tmp.path().join("store").to_string_lossy().to_string();
    std::fs::create_dir_all(&storage_path).unwrap();

    let cfg = deterministic_cfg(queue_path, storage_path);
    let sink = Arc::new(MemorySink::create());
    let crawler = Crawler::new(cfg).unwrap().with_events(sink.clone());

    let seeds: Vec<String> = vec![format!("{}/p/0", server.uri())];
    crawler.seed(seeds).await.unwrap();

    let _ = tokio::time::timeout(Duration::from_secs(30), crawler.run()).await;

    let events = sink.take();
    let observed: Vec<String> = events
        .iter()
        .map(|ev| event_kind_wire_name(&ev.event))
        .collect();
    let observed_joined = observed.join("\n") + "\n";

    let golden = golden_path();

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&golden, &observed_joined).expect("write golden");
        eprintln!("UPDATE_GOLDEN=1 → wrote {}", golden.display());
        return;
    }

    let expected =
        std::fs::read_to_string(&golden).expect("golden file missing; run with UPDATE_GOLDEN=1");

    assert_eq!(
        observed_joined, expected,
        "NDJSON event-kind sequence drifted from golden. \
         If the change is intentional, re-record with \
         UPDATE_GOLDEN=1 cargo test --test runner_ndjson_regression --all-features"
    );
}

/// Serialize a single `EventKind` to its wire string (`run.started`,
/// `job.failed`, …). Goes through `serde_json` so the source of truth
/// stays the `#[serde(rename = ...)]` attributes on the enum — same
/// strings consumers see on the NDJSON wire.
fn event_kind_wire_name<K: Serialize>(kind: &K) -> String {
    let s = serde_json::to_string(kind).expect("event kind serializes");
    s.trim_matches('"').to_string()
}
