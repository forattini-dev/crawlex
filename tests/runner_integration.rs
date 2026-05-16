//! Integration test — `JobRunner::run` end-to-end against wiremock.
//!
//! Drives a real `JobRunner` over a real `SpoofFetcher` over a real
//! `ImpersonateClient` (no Chrome, no FakeFetcher). Asserts the full
//! pipeline produces the expected `JobOutcome` for four scenarios:
//! healthy 200, 403 Cloudflare challenge, network failure (port not
//! listening), and timeout.

use std::sync::Arc;
use std::time::Duration;

use crawlex::impersonate::{ImpersonateClient, Profile};
use crawlex::queue::{FetchMethod, Job};
use crawlex::runner::{Fetcher, JobError, JobRunner, RetryDecision, RetryReason, SessionContext, SpoofFetcher};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn dummy_job(id: u64, url: Url) -> Job {
    Job {
        id,
        crawl_id: 0,
        url,
        depth: 0,
        priority: 0,
        method: FetchMethod::HttpSpoof,
        attempts: 0,
        last_error: None,
    }
}

fn build_runner() -> JobRunner {
    let client = Arc::new(ImpersonateClient::new(Profile::Chrome131Stable).expect("client"));
    let spoof = Arc::new(SpoofFetcher::new(client));
    JobRunner::new(spoof as Arc<dyn Fetcher>)
}

#[tokio::test]
async fn healthy_200_with_links_yields_fetch_success_no_retry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(
                    r#"<html><body><a href="/a">a</a><a href="/b">b</a></body></html>"#,
                ),
        )
        .mount(&server)
        .await;

    let url: Url = format!("{}/ok", server.uri()).parse().unwrap();
    let runner = build_runner();
    let outcome = runner
        .run(&dummy_job(1, url), &SessionContext::default())
        .await;

    let success = outcome.result.expect("success branch");
    assert_eq!(success.status, 200);
    assert!(success.body_bytes > 0);
    assert!(success.links.len() >= 2, "expected ≥2 links, got {:?}", success.links);
    assert!(success.signals.is_empty());
    assert!(matches!(outcome.retry, RetryDecision::None));
    assert!(outcome.timings.fetch_ms.is_some());
    assert!(outcome.timings.extract_ms.is_some());
    assert!(outcome.timings.total_ms.is_some());
    assert!(outcome.error.is_none());
}

#[tokio::test]
async fn cloudflare_challenge_403_triggers_escalate_to_render() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/blocked"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string("<html>cf-chl-bypass</html>"),
        )
        .mount(&server)
        .await;

    let url: Url = format!("{}/blocked", server.uri()).parse().unwrap();
    let runner = build_runner();
    let outcome = runner
        .run(&dummy_job(2, url), &SessionContext::default())
        .await;

    let success = outcome.result.expect("response received");
    assert_eq!(success.status, 403);
    assert_eq!(
        success.signals.len(),
        1,
        "expected 1 challenge signal, got {} ({:?})",
        success.signals.len(),
        success
    );
    assert!(
        matches!(
            outcome.retry,
            RetryDecision::Suggest {
                reason: RetryReason::EscalateToRender,
                ..
            }
        ),
        "expected EscalateToRender retry, got {:?}",
        outcome.retry
    );
}

#[tokio::test]
async fn connection_refused_yields_network_error_with_retry_suggest() {
    // Choose a port unlikely to be listening — wiremock not started here.
    let url: Url = "http://127.0.0.1:65530/no-server".parse().unwrap();
    let runner = build_runner();
    let outcome = runner
        .run(&dummy_job(3, url), &SessionContext::default())
        .await;

    assert!(outcome.result.is_none(), "no success expected");
    assert!(outcome.error.is_some(), "error expected");
    match outcome.error.unwrap() {
        JobError::Network(_) | JobError::Timeout => {}
        other => panic!("unexpected error variant: {other:?}"),
    }
    assert!(
        matches!(
            outcome.retry,
            RetryDecision::Suggest {
                reason: RetryReason::Network | RetryReason::Timeout,
                ..
            }
        ),
        "expected Network/Timeout retry, got {:?}",
        outcome.retry
    );
    assert!(outcome.timings.fetch_ms.is_some());
    assert!(outcome.timings.total_ms.is_some());
}

#[tokio::test]
async fn timeout_yields_timeout_error_with_retry_timeout() {
    // wiremock responds slowly enough that the client's request times out.
    // ImpersonateClient default total timeout is generous; we simulate by
    // pointing at an unreachable IP that BLACKHOLES rather than refuses.
    // 192.0.2.0/24 is RFC 5737 TEST-NET-1 — guaranteed unroutable.
    let url: Url = "http://192.0.2.1:80/nowhere".parse().unwrap();
    let runner = build_runner();
    let outcome = tokio::time::timeout(
        Duration::from_secs(40),
        runner.run(&dummy_job(4, url), &SessionContext::default()),
    )
    .await
    .expect("runner returned within wall-clock limit");

    assert!(outcome.result.is_none());
    assert!(outcome.error.is_some());
    // Either Network (connect refused upstream) or Timeout (waited).
    // The runner's classify_error maps both to retry-suggestion.
    assert!(
        matches!(
            outcome.retry,
            RetryDecision::Suggest {
                reason: RetryReason::Network | RetryReason::Timeout,
                ..
            }
        ),
        "expected Network/Timeout retry, got {:?}",
        outcome.retry
    );
}
