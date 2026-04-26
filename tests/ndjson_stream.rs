//! End-to-end test for the NDJSON event bus.
//!
//! Drives a real `Crawler::run` against a wiremock server with a
//! `MemorySink` plugged in, then asserts the stream contains the expected
//! lifecycle events in the right order.

use std::sync::Arc;
use std::time::Duration;

use crawlex::config::{Config, QueueBackend, StorageBackend};
use crawlex::events::{EventKind, MemorySink};
use crawlex::Crawler;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http_only_config() -> Config {
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

#[tokio::test]
async fn run_emits_run_started_then_job_started_then_run_completed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>ok</body></html>"))
        .mount(&server)
        .await;

    let cfg = Config {
        queue_backend: QueueBackend::InMemory,
        storage_backend: StorageBackend::Memory,
        ..http_only_config()
    };
    let sink = Arc::new(MemorySink::create());
    let crawler = Crawler::new(cfg).unwrap().with_events(sink.clone());
    crawler
        .seed(vec![format!("{}/page", server.uri())])
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(15), crawler.run()).await;

    let events = sink.take();
    assert!(!events.is_empty(), "expected at least one event, got none");

    // First event must be run.started.
    assert_eq!(events.first().unwrap().event, EventKind::RunStarted);
    // Last event must be run.completed.
    assert_eq!(events.last().unwrap().event, EventKind::RunCompleted);
    // At least one job.started in between.
    assert!(events.iter().any(|e| e.event == EventKind::JobStarted));

    // Every event must carry the run_id.
    let run_id = events.first().unwrap().run_id.unwrap();
    for ev in &events {
        assert_eq!(
            ev.run_id,
            Some(run_id),
            "event {:?} missing run_id",
            ev.event
        );
    }
}

#[tokio::test]
async fn auto_escalation_emits_decision_made_with_vendor_why() {
    let server = MockServer::start().await;
    let cf_body = b"<html><head></head><body>Just a moment... cf-chl-bypass</body></html>";
    Mock::given(method("GET"))
        .and(path("/blocked"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_bytes(&cf_body[..]),
        )
        .mount(&server)
        .await;

    let cfg = Config {
        max_concurrent_render: 1, // allow render so escalation fires
        queue_backend: QueueBackend::InMemory,
        storage_backend: StorageBackend::Memory,
        ..http_only_config()
    };
    let sink = Arc::new(MemorySink::create());
    let crawler = Crawler::new(cfg).unwrap().with_events(sink.clone());
    let url = format!("{}/blocked", server.uri());
    crawler.seed(vec![url.clone()]).await.unwrap();
    // Don't wait for the render path to complete — it'd require a live
    // browser. We only care that the HTTP path emitted decision.made
    // before failing/timing out on render. 60s is generous so the
    // assertion below still meaningfully fires even on a CI runner
    // sharing IO with five other test binaries.
    let _ = tokio::time::timeout(Duration::from_secs(60), crawler.run()).await;

    let events = sink.take();
    let decisions: Vec<_> = events
        .iter()
        .filter(|e| e.event == EventKind::DecisionMade)
        .collect();
    assert!(
        !decisions.is_empty(),
        "expected at least one decision.made, got {} total events",
        events.len()
    );
    let cf_decision = decisions
        .iter()
        .find(|e| {
            e.why
                .as_ref()
                .map(|w| w.contains("cloudflare"))
                .unwrap_or(false)
        })
        .expect("expected a decision.made with why=render:antibot:cloudflare");
    assert!(cf_decision
        .why
        .as_ref()
        .unwrap()
        .starts_with("render:antibot:cloudflare"));
}
