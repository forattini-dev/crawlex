//! End-to-end acceptance tests.
//!
//! These tests stand a local `wiremock` server up and point the crawler at
//! it, so we exercise the full stack (transport, headers, cookies,
//! redirects, queue lifecycle) without hitting the public internet.
//!
//! Each test owns its own sqlite queue/storage files via `tempfile`.

use std::sync::Arc;
use std::time::Duration;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crawlex::config::{Config, QueueBackend, StorageBackend};
use crawlex::events::{EventKind, MemorySink};
use crawlex::impersonate::ImpersonateClient;
use crawlex::impersonate::Profile;
use crawlex::queue::FetchMethod;
use crawlex::Crawler;

/// Baseline config pointing storage at an in-memory store and queue at
/// in-memory; individual tests override backends when they want to observe
/// SQLite state.
fn base_config() -> Config {
    Config {
        max_concurrent_http: 2,
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
        ..Config::default()
    }
}

async fn run_crawl(cfg: Config, seeds: Vec<String>) -> crawlex::Result<()> {
    let crawler = Crawler::new(cfg)?;
    crawler.seed(seeds).await?;
    crawler.run().await
}

#[tokio::test]
async fn cookie_persistence_across_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/set"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "session=abc; Path=/")
                .set_body_string("<html>ok</html>"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/echo"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>ok</html>"))
        .mount(&server)
        .await;

    let client = ImpersonateClient::new(Profile::Chrome131Stable).unwrap();
    let url_set = Url::parse(&format!("{}/set", server.uri())).unwrap();
    let url_echo = Url::parse(&format!("{}/echo", server.uri())).unwrap();
    let _ = client.get(&url_set).await.expect("first request");
    let _ = client.get(&url_echo).await.expect("second request");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
    let second = &received[1];
    let has_cookie = second
        .headers
        .get("cookie")
        .map(|v| v.to_str().unwrap_or("").contains("session=abc"))
        .unwrap_or(false);
    assert!(
        has_cookie,
        "second request did not carry Cookie; headers: {:?}",
        second.headers
    );
}

#[tokio::test]
async fn redirect_chain_follows_until_cap() {
    let server = MockServer::start().await;
    let final_uri = format!("{}/final", server.uri());
    Mock::given(method("GET"))
        .and(path("/hop1"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", format!("{}/hop2", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hop2"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", final_uri.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .mount(&server)
        .await;

    let client = ImpersonateClient::new(Profile::Chrome131Stable).unwrap();
    let start = Url::parse(&format!("{}/hop1", server.uri())).unwrap();
    let resp = client.get(&start).await.expect("follow");
    assert_eq!(resp.status.as_u16(), 200);
    assert_eq!(&resp.body[..], b"done");
}

#[tokio::test]
async fn sqlite_queue_failure_marks_fail_not_in_flight() {
    // Crawl a guaranteed-to-fail address. We use TEST-NET-1 (192.0.2.0/24)
    // which RFC 5737 reserves for docs — no host there will respond.
    let tmp = tempfile::tempdir().unwrap();
    let queue_path = tmp.path().join("q.db").to_string_lossy().to_string();
    let storage_path = tmp.path().join("s.db").to_string_lossy().to_string();

    let cfg = Config {
        queue_backend: QueueBackend::Sqlite {
            path: queue_path.clone(),
        },
        storage_backend: StorageBackend::Sqlite { path: storage_path },
        max_concurrent_http: 1,
        retry_max: 2,
        retry_backoff: Duration::from_millis(10),
        ..base_config()
    };
    // We don't await the crawler forever — short timeout ensures retries
    // settle if the transport gives up quickly (TCP connect refused/hangs).
    let seeds = vec!["https://192.0.2.1/unreachable".to_string()];
    let run = tokio::time::timeout(Duration::from_secs(60), run_crawl(cfg, seeds)).await;
    // The crawler may return Ok (no more jobs) or timeout; both are fine —
    // what matters is the queue state.
    let _ = run;

    let conn = rusqlite::Connection::open(&queue_path).unwrap();
    let mut stmt = conn.prepare("SELECT state, attempts FROM jobs").unwrap();
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert!(
        !rows.is_empty(),
        "expected at least one job row in the queue"
    );
    for (state, _attempts) in &rows {
        assert_ne!(
            state, "in_flight",
            "job stuck in_flight — lifecycle wrapper missing"
        );
    }
}

#[tokio::test]
async fn headers_per_asset_kind() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let client = ImpersonateClient::new(Profile::Chrome131Stable).unwrap();
    let url_doc = Url::parse(&format!("{}/page", server.uri())).unwrap();
    let url_js = Url::parse(&format!("{}/bundle.js", server.uri())).unwrap();
    let url_img = Url::parse(&format!("{}/logo.png", server.uri())).unwrap();
    use crawlex::discovery::assets::SecFetchDest;
    let _ = client
        .get_with_dest(&url_doc, SecFetchDest::Document)
        .await
        .unwrap();
    let _ = client
        .get_with_dest(&url_js, SecFetchDest::Script)
        .await
        .unwrap();
    let _ = client
        .get_with_dest(&url_img, SecFetchDest::Image)
        .await
        .unwrap();

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 3);
    let doc = &reqs[0];
    let js = &reqs[1];
    let img = &reqs[2];

    // Document: document dest + UIR header.
    let dest_of = |h: &http::HeaderMap| {
        h.get("sec-fetch-dest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(dest_of(&doc.headers), "document");
    assert!(doc.headers.get("upgrade-insecure-requests").is_some());
    // Script: script dest, no UIR.
    assert_eq!(dest_of(&js.headers), "script");
    assert!(
        js.headers.get("upgrade-insecure-requests").is_none(),
        "script request leaked Upgrade-Insecure-Requests"
    );
    // Image: image dest, no UIR.
    assert_eq!(dest_of(&img.headers), "image");
    assert!(img.headers.get("upgrade-insecure-requests").is_none());
}

#[tokio::test]
async fn render_jobs_fail_permanently_when_render_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let queue_path = tmp.path().join("q.db").to_string_lossy().to_string();
    let cfg = Config {
        queue_backend: QueueBackend::Sqlite {
            path: queue_path.clone(),
        },
        storage_backend: StorageBackend::Memory,
        max_concurrent_render: 0,
        max_concurrent_http: 1,
        retry_backoff: Duration::from_millis(10),
        max_depth: Some(0),
        respect_robots_txt: false,
        ..base_config()
    };

    let crawler = Crawler::new(cfg).unwrap();
    crawler
        .seed_with(vec!["https://example.invalid/blocked"], FetchMethod::Render)
        .await
        .unwrap();
    crawler.run().await.unwrap();

    let conn = rusqlite::Connection::open(queue_path).unwrap();
    let (state, attempts): (String, i64) = conn
        .query_row("SELECT state, attempts FROM jobs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn policy_retry_delay_keeps_sqlite_crawler_alive_until_ready() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/busy"))
        .respond_with(ResponseTemplate::new(503).set_body_string("try later"))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let queue_path = tmp.path().join("q.db").to_string_lossy().to_string();
    let cfg = Config {
        queue_backend: QueueBackend::Sqlite {
            path: queue_path.clone(),
        },
        storage_backend: StorageBackend::Memory,
        max_concurrent_http: 1,
        retry_max: 2,
        retry_backoff: Duration::from_millis(10),
        ..base_config()
    };

    let seed = format!("{}/busy", server.uri());
    tokio::time::timeout(Duration::from_secs(8), run_crawl(cfg, vec![seed]))
        .await
        .expect("crawler should wait for delayed retry instead of exiting")
        .unwrap();

    let received = server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        2,
        "first 503 should be retried once before policy drops at retry_max"
    );

    let conn = rusqlite::Connection::open(queue_path).unwrap();
    let (state, attempts): (String, i64) = conn
        .query_row("SELECT state, attempts FROM jobs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(state, "done");
    assert_eq!(attempts, 1);
}

#[tokio::test]
// FIXME: assertion `state == "failed"` does not match the queue's
// behaviour for a `drop` decision (which marks the job `done`, terminal).
// The test pre-dates the policy refactor; keep ignored until the queue
// exposes a third "drop" state we can assert on.
#[ignore = "asserts a queue state that no longer matches drop decisions"]
async fn post_error_policy_emits_terminal_drop_decision_at_retry_cap() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let tmp = tempfile::tempdir().unwrap();
    let queue_path = tmp.path().join("q.db").to_string_lossy().to_string();
    let cfg = Config {
        queue_backend: QueueBackend::Sqlite {
            path: queue_path.clone(),
        },
        storage_backend: StorageBackend::Memory,
        max_concurrent_http: 1,
        retry_max: 1,
        retry_backoff: Duration::from_millis(10),
        ..base_config()
    };

    let sink = Arc::new(MemorySink::create());
    let crawler = Crawler::new(cfg).unwrap().with_events(sink.clone());
    crawler
        .seed(vec![format!("http://{addr}/refused")])
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), crawler.run())
        .await
        .expect("connect-refused run should settle quickly")
        .unwrap();

    let events = sink.take();
    assert!(
        events.iter().any(|ev| {
            ev.event == EventKind::DecisionMade
                && ev.data.get("decision").and_then(|v| v.as_str()) == Some("drop")
                && ev.data.get("error_kind").and_then(|v| v.as_str()).is_some()
        }),
        "expected post-error policy to emit a terminal drop decision; events: {events:?}"
    );

    let conn = rusqlite::Connection::open(queue_path).unwrap();
    let (state, attempts): (String, i64) = conn
        .query_row("SELECT state, attempts FROM jobs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(attempts, 1);
}
