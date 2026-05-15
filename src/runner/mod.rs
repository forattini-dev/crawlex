//! Runner module — per-Job execution seam.
//!
//! Slice 0 of the JobRunner extraction (PRD forattini-dev/crawlex#15).
//! Type shells only; no behavior is routed through these yet. Subsequent
//! slices land `Fetcher`/`SpoofFetcher` (#17), `Extractor` (#18),
//! `ChallengeDetector` (#19), `RenderFetcher` (#20), `AutoFetcher` (#21),
//! and finally promote `JobRunner::run` to the per-Job entry point (#22).
//!
//! See ADR-0001 for the rationale behind the value-return `JobOutcome`
//! shape and ADR-0002 for the re-queue escalation contract.
//!
//! Stability rule: `JobRunner` is held as `Arc<JobRunner>` shared across
//! workers. It must stay `Send + Sync` and free of per-call mutable state
//! on `self`. All per-attempt state goes in `SessionContext` (input) or
//! `JobOutcome` (output).

use std::time::Duration;

pub mod challenge;
pub mod extract;
pub mod fetcher;
pub use challenge::{ChallengeDetector, ChallengeSignal};
pub use extract::Extractor;
pub use fetcher::{AutoFetcher, AutoOutcome, Fetcher, SpoofFetcher};
#[cfg(feature = "cdp-backend")]
pub use fetcher::RenderFetcher;

/// Outcome of running one Job. Returned by value; the `Crawler`
/// post-processes storage, frontier feed, retry decision, and session
/// state commit.
#[derive(Debug, Clone, Default)]
pub struct JobOutcome {
    pub result: Option<FetchSuccess>,
    pub error: Option<JobError>,
    pub timings: JobTimings,
    pub retry: RetryDecision,
    pub new_session_state: Option<SessionStatePlaceholder>,
}

/// Success branch of `JobOutcome`.
#[derive(Debug, Clone, Default)]
pub struct FetchSuccess {
    pub status: u16,
    pub body_bytes: usize,
    pub links: Vec<String>,
    pub signals: Vec<ChallengeSignalPlaceholder>,
}

/// Per-attempt timings populated on both success and failure branches.
#[derive(Debug, Clone, Default)]
pub struct JobTimings {
    pub queued_for: Option<Duration>,
    pub ttfb: Option<Duration>,
    pub fetch_ms: Option<Duration>,
    pub render_ms: Option<Duration>,
    pub extract_ms: Option<Duration>,
    pub total_ms: Option<Duration>,
}

/// Per-attempt input bundle passed by the `Crawler` to `JobRunner::run`.
/// `SessionIdentity`, `ProxyLease`, `PolicyProfile`, and `JobBudgets` are
/// placeholders here; slices #17 onward connect them to concrete types.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub identity: SessionIdentityPlaceholder,
    pub proxy: Option<ProxyLeasePlaceholder>,
    pub session_state: SessionStatePlaceholder,
    pub budgets: JobBudgetsPlaceholder,
    pub policy: PolicyProfilePlaceholder,
}

/// Runner advises; the `Crawler` decides (retry caps, host cooldowns,
/// budget accounting). See PRD #15 Q11.
#[derive(Debug, Clone, Default)]
pub enum RetryDecision {
    #[default]
    None,
    Suggest {
        reason: RetryReason,
        backoff_hint: Option<Duration>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryReason {
    EscalateToRender,
    Timeout,
    Network,
    ChallengeRecoverable,
}

/// Failure modes the `JobRunner` reports to the `Crawler`.
#[derive(Debug, Clone)]
pub enum JobError {
    Network(String),
    Timeout,
    RenderFailed(String),
    ChallengeUnrecoverable(String),
    BudgetExhausted,
    Cancelled,
}

// Placeholder types — replaced with concrete shapes in later slices.

#[derive(Debug, Clone, Default)]
pub struct SessionIdentityPlaceholder;

#[derive(Debug, Clone, Default)]
pub struct ProxyLeasePlaceholder;

#[derive(Debug, Clone, Default)]
pub struct SessionStatePlaceholder;

#[derive(Debug, Clone, Default)]
pub struct JobBudgetsPlaceholder;

#[derive(Debug, Clone, Default)]
pub struct PolicyProfilePlaceholder;

#[derive(Debug, Clone, Default)]
pub struct ChallengeSignalPlaceholder;

/// Top-level runner. Slice #22 implements `run` for the spoof path and
/// the rest of `process_job`-style post-processing stays with the
/// `Crawler`. Render and Auto paths flow through `Fetcher`-trait impls
/// added in #20/#21; `run` delegates to whichever fetcher matches the
/// `Method` on the job.
///
/// Held as `Arc<JobRunner>` and shared across workers. `Send + Sync`,
/// stateless on `self`.
pub struct JobRunner {
    fetcher: std::sync::Arc<dyn Fetcher>,
    extractor: Extractor,
    detector: ChallengeDetector,
}

impl JobRunner {
    pub fn new(fetcher: std::sync::Arc<dyn Fetcher>) -> Self {
        Self {
            fetcher,
            extractor: Extractor::new(),
            detector: ChallengeDetector::new(),
        }
    }

    /// Execute one job. Pure of queue, storage, admission, and frontier
    /// concerns — the `Crawler` post-processes those.
    ///
    /// Outcome is returned by value (ADR-0001). Live events fire from
    /// the injected `EventSink` held by the fetcher / future runner
    /// deps — slice #23 moves per-attempt event emission into this
    /// function explicitly.
    pub async fn run(&self, job: &crate::queue::Job, ctx: &SessionContext) -> JobOutcome {
        let start = std::time::Instant::now();
        let fetch_started = std::time::Instant::now();
        let fetch_result = self.fetcher.fetch(job, ctx).await;
        let fetch_ms = Some(fetch_started.elapsed());

        match fetch_result {
            Ok(resp) => {
                let status = resp.status.as_u16();
                let body_bytes = resp.body.len();
                let signal = self.detector.detect(status, &resp.headers, &resp.body);
                let extract_started = std::time::Instant::now();
                // Body decoded once; extract reuses the same slice.
                let body_str = String::from_utf8_lossy(&resp.body).into_owned();
                let links = self
                    .extractor
                    .extract_links(&resp.final_url, &body_str)
                    .into_iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>();
                let extract_ms = Some(extract_started.elapsed());

                let retry = match signal {
                    Some(_) => RetryDecision::Suggest {
                        reason: RetryReason::EscalateToRender,
                        backoff_hint: None,
                    },
                    None => RetryDecision::None,
                };
                let signals = signal
                    .map(|_| vec![ChallengeSignalPlaceholder])
                    .unwrap_or_default();

                JobOutcome {
                    result: Some(FetchSuccess {
                        status,
                        body_bytes,
                        links,
                        signals,
                    }),
                    error: None,
                    timings: JobTimings {
                        fetch_ms,
                        extract_ms,
                        total_ms: Some(start.elapsed()),
                        ..Default::default()
                    },
                    retry,
                    new_session_state: None,
                }
            }
            Err(e) => {
                let (error, retry) = classify_error(e);
                JobOutcome {
                    result: None,
                    error: Some(error),
                    timings: JobTimings {
                        fetch_ms,
                        total_ms: Some(start.elapsed()),
                        ..Default::default()
                    },
                    retry,
                    new_session_state: None,
                }
            }
        }
    }
}

/// Map an `Error` from the fetch layer to a `JobError` + advised
/// `RetryDecision`. The mapping is the contract the `Crawler` reads:
/// transient network problems advise retry; render failures and
/// unrecoverable challenges do not.
fn classify_error(err: crate::Error) -> (JobError, RetryDecision) {
    use crate::Error;
    match err {
        Error::Io(io) => {
            let msg = io.to_string();
            let is_timeout = matches!(io.kind(), std::io::ErrorKind::TimedOut)
                || msg.contains("timed out");
            if is_timeout {
                (
                    JobError::Timeout,
                    RetryDecision::Suggest {
                        reason: RetryReason::Timeout,
                        backoff_hint: None,
                    },
                )
            } else {
                (
                    JobError::Network(msg),
                    RetryDecision::Suggest {
                        reason: RetryReason::Network,
                        backoff_hint: None,
                    },
                )
            }
        }
        Error::Http(s) | Error::Tls(s) | Error::Decompression(s) => (
            JobError::Network(s),
            RetryDecision::Suggest {
                reason: RetryReason::Network,
                backoff_hint: None,
            },
        ),
        Error::Render(s) | Error::RenderDisabled(s) => {
            (JobError::RenderFailed(s), RetryDecision::None)
        }
        Error::AntibotChallenge { vendor, .. } => (
            JobError::ChallengeUnrecoverable(format!("{vendor:?}")),
            RetryDecision::None,
        ),
        other => (JobError::Network(other.to_string()), RetryDecision::None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use async_trait::async_trait;
    use bytes::Bytes;
    use http::{HeaderMap, StatusCode};
    use url::Url;

    fn assert_send_sync<T: Send + Sync>() {}

    /// Fake fetcher whose response is whatever the test sets up. Used
    /// to drive `JobRunner::run` without touching the network or
    /// Chrome.
    struct FakeFetcher {
        response: parking_lot::Mutex<Option<crate::Result<crate::impersonate::Response>>>,
    }
    impl FakeFetcher {
        fn ok(status: u16, headers: HeaderMap, body: &[u8], final_url: Url) -> Arc<Self> {
            Arc::new(Self {
                response: parking_lot::Mutex::new(Some(Ok(crate::impersonate::Response {
                    status: StatusCode::from_u16(status).unwrap(),
                    headers,
                    body: Bytes::copy_from_slice(body),
                    final_url,
                    alpn: None,
                    tls_version: None,
                    cipher: None,
                    timings: crate::metrics::NetworkTimings::default(),
                    peer_cert: None,
                    body_truncated: false,
                }))),
            })
        }
        fn err(err: crate::Error) -> Arc<Self> {
            Arc::new(Self {
                response: parking_lot::Mutex::new(Some(Err(err))),
            })
        }
    }
    #[async_trait]
    impl Fetcher for FakeFetcher {
        async fn fetch(
            &self,
            _job: &crate::queue::Job,
            _ctx: &SessionContext,
        ) -> crate::Result<crate::impersonate::Response> {
            self.response.lock().take().expect("fake response set")
        }
    }

    fn dummy_job(url: &str) -> crate::queue::Job {
        crate::queue::Job {
            id: 1,
            crawl_id: 0,
            url: Url::parse(url).unwrap(),
            depth: 0,
            priority: 0,
            method: crate::queue::FetchMethod::HttpSpoof,
            attempts: 0,
            last_error: None,
        }
    }

    fn html_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("content-type", "text/html; charset=utf-8".parse().unwrap());
        h
    }

    #[tokio::test]
    async fn run_ok_emits_fetch_success_with_links() {
        let url: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><body><a href=\"/a\">a</a><a href=\"/b\">b</a></body></html>";
        let fake = FakeFetcher::ok(200, html_headers(), body, url.clone());
        let runner = JobRunner::new(fake as Arc<dyn Fetcher>);
        let outcome = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        let success = outcome.result.expect("ok branch");
        assert_eq!(success.status, 200);
        assert!(success.links.iter().any(|u| u.ends_with("/a")));
        assert!(success.links.iter().any(|u| u.ends_with("/b")));
        assert!(success.signals.is_empty());
        assert!(matches!(outcome.retry, RetryDecision::None));
        assert!(outcome.timings.total_ms.is_some());
        assert!(outcome.timings.fetch_ms.is_some());
        assert!(outcome.timings.extract_ms.is_some());
        assert!(outcome.error.is_none());
    }

    #[tokio::test]
    async fn run_detects_challenge_and_suggests_escalate() {
        let url: Url = "https://example.com/".parse().unwrap();
        let body = b"cf-chl-bypass";
        let fake = FakeFetcher::ok(403, html_headers(), body, url);
        let runner = JobRunner::new(fake as Arc<dyn Fetcher>);
        let outcome = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        let success = outcome.result.expect("ok branch");
        assert_eq!(success.status, 403);
        assert_eq!(success.signals.len(), 1);
        assert!(matches!(
            outcome.retry,
            RetryDecision::Suggest {
                reason: RetryReason::EscalateToRender,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn run_io_timeout_maps_to_retry_timeout() {
        let err = crate::Error::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"));
        let fake = FakeFetcher::err(err);
        let runner = JobRunner::new(fake as Arc<dyn Fetcher>);
        let outcome = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        assert!(outcome.result.is_none());
        assert!(matches!(outcome.error, Some(JobError::Timeout)));
        assert!(matches!(
            outcome.retry,
            RetryDecision::Suggest {
                reason: RetryReason::Timeout,
                ..
            }
        ));
        // Timings populated even on failure.
        assert!(outcome.timings.total_ms.is_some());
        assert!(outcome.timings.fetch_ms.is_some());
    }

    #[tokio::test]
    async fn run_render_failure_does_not_suggest_retry() {
        let err = crate::Error::Render("nav".into());
        let fake = FakeFetcher::err(err);
        let runner = JobRunner::new(fake as Arc<dyn Fetcher>);
        let outcome = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        assert!(matches!(
            outcome.error,
            Some(JobError::RenderFailed(_))
        ));
        assert!(matches!(outcome.retry, RetryDecision::None));
    }

    #[tokio::test]
    async fn run_antibot_challenge_error_is_unrecoverable_no_retry() {
        let err = crate::Error::AntibotChallenge {
            vendor: crate::error::AntibotVendor::Cloudflare,
            status: 403,
            note: "x".into(),
        };
        let fake = FakeFetcher::err(err);
        let runner = JobRunner::new(fake as Arc<dyn Fetcher>);
        let outcome = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        assert!(matches!(
            outcome.error,
            Some(JobError::ChallengeUnrecoverable(_))
        ));
        assert!(matches!(outcome.retry, RetryDecision::None));
    }

    #[test]
    fn runner_is_send_sync() {
        assert_send_sync::<JobRunner>();
    }

    #[test]
    fn outcome_defaults_are_neutral() {
        let outcome = JobOutcome::default();
        assert!(outcome.result.is_none());
        assert!(outcome.error.is_none());
        assert!(matches!(outcome.retry, RetryDecision::None));
        assert!(outcome.new_session_state.is_none());
        assert!(outcome.timings.total_ms.is_none());
    }

    #[test]
    fn retry_suggest_carries_reason_and_backoff() {
        let r = RetryDecision::Suggest {
            reason: RetryReason::EscalateToRender,
            backoff_hint: Some(Duration::from_millis(250)),
        };
        match r {
            RetryDecision::Suggest {
                reason,
                backoff_hint,
            } => {
                assert_eq!(reason, RetryReason::EscalateToRender);
                assert_eq!(backoff_hint, Some(Duration::from_millis(250)));
            }
            _ => panic!("expected Suggest"),
        }
    }

    #[test]
    fn job_error_variants_compile() {
        let _ = JobError::Network("dns".into());
        let _ = JobError::Timeout;
        let _ = JobError::RenderFailed("nav".into());
        let _ = JobError::ChallengeUnrecoverable("cf".into());
        let _ = JobError::BudgetExhausted;
        let _ = JobError::Cancelled;
    }

    #[test]
    fn session_context_default_compiles() {
        let _ = SessionContext::default();
    }
}
