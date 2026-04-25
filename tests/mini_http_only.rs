//! End-to-end "mini as production HTTP worker" scenario.
//!
//! Drives a real `Crawler::run` with:
//!   * SQLite queue (the writer-thread path) against a wiremock server,
//!   * Filesystem storage,
//!   * NDJSON sink collected in memory.
//!
//! Asserts the full lifecycle holds up at 100 URLs + the SQLite queue
//! ends drained. This is the baseline scale-crawl test — when this
//! breaks, the mini is broken as a worker.

use std::sync::Arc;
use std::time::Duration;

use crawlex::config::{Config, QueueBackend, StorageBackend};
use crawlex::events::{EventKind, MemorySink};
use crawlex::queue::JobQueue;
use crawlex::Crawler;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http_only_cfg(queue_path: String, storage_path: String) -> Config {
    Config {
        max_concurrent_http: 8,
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
async fn mini_crawls_100_urls_http_only_with_sqlite_queue() {
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

    let cfg = http_only_cfg(queue_path.clone(), storage_path);
    let sink = Arc::new(MemorySink::create());
    let crawler = Crawler::new(cfg).unwrap().with_events(sink.clone());

    let seeds: Vec<String> = (0..100)
        .map(|i| format!("{}/p/{i}", server.uri()))
        .collect();
    crawler.seed(seeds).await.unwrap();

    // 30 s ceiling — a clean 100-URL crawl against wiremock completes in
    // under 5 s on a warm machine. Timeout here means the writer thread
    // or scheduler is deadlocked.
    let _ = tokio::time::timeout(Duration::from_secs(30), crawler.run()).await;

    let events = sink.take();
    assert_eq!(
        events.first().unwrap().event,
        EventKind::RunStarted,
        "first event must be run.started"
    );
    assert_eq!(
        events.last().unwrap().event,
        EventKind::RunCompleted,
        "last event must be run.completed"
    );
    // At least 100 job.started events (one per seed). Session-policy
    // additions in Wave 1 (session depth caps + end-session requeue) may
    // legitimately emit a handful of extras when a session depth cap fires
    // and the job is re-scheduled under a fresh session; tolerate up to
    // 20% churn so the core "every seed ran" invariant stays green.
    let job_starts = events
        .iter()
        .filter(|e| e.event == EventKind::JobStarted)
        .count();
    assert!(
        (100..=120).contains(&job_starts),
        "expected 100..=120 job.started events, got {job_starts}"
    );

    // Queue drained: SELECT from SQLite confirms every row `done`.
    let conn = rusqlite::Connection::open(&queue_path).unwrap();
    let (done, pending, in_flight): (i64, i64, i64) = conn
        .query_row(
            "SELECT
                SUM(CASE WHEN state='done' THEN 1 ELSE 0 END),
                SUM(CASE WHEN state='pending' THEN 1 ELSE 0 END),
                SUM(CASE WHEN state='in_flight' THEN 1 ELSE 0 END)
             FROM jobs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    // Session-policy requeues can duplicate job rows (see job.started
    // tolerance above). Every original seed landed `done`, plus extras
    // from requeues — all `done` too, nothing pending.
    assert!(
        (100..=120).contains(&done),
        "expected 100..=120 done rows; actual done={done}"
    );
    assert_eq!(pending, 0, "expected 0 pending; actual={pending}");
    assert_eq!(in_flight, 0, "expected 0 in_flight; actual={in_flight}");
}

#[tokio::test]
async fn queue_batches_push_amplification() {
    // Push 500 jobs rapid-fire through the writer thread and confirm they
    // all land. This exercises the "drain into a single transaction" path
    // (up to 256 per batch) that makes discovery bursts cheap.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("q.db").to_string_lossy().to_string();
    let queue = Arc::new(crawlex::queue::sqlite::SqliteQueue::open(&path).unwrap());

    let mut handles = Vec::with_capacity(500);
    for i in 0..500 {
        let q = queue.clone();
        handles.push(tokio::spawn(async move {
            q.push(crawlex::queue::Job {
                id: 0,
                url: url::Url::parse(&format!("https://example.com/p/{i}")).unwrap(),
                depth: 0,
                priority: 0,
                method: crawlex::queue::FetchMethod::HttpSpoof,
                attempts: 0,
                last_error: None,
            })
            .await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    // Confirm count via a fresh connection, bypassing the writer thread
    // (so we're not asking the same code that wrote the data).
    let conn = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 500);
}
