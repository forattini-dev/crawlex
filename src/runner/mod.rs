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
pub use fetcher::{AutoFetcher, AutoOutcome, FetchOutput, Fetcher, SpoofFetcher};
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
/// Real fields after slice A3 — placeholders dropped (PRD #24).
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub identity: SessionIdentity,
    pub proxy: Option<ProxyLease>,
    pub session_state: crate::antibot::SessionState,
    pub budgets: JobBudgets,
    pub policy: crate::policy::PolicyProfile,
}

/// Per-session browser persona handle. Today this is a thin
/// description (profile name + locale + session id) — the deeper
/// unification of `ImpersonateClient + IdentityBundle + cookies` is
/// out of scope per PRD #15. Slice A3 lands the seam.
#[derive(Debug, Clone, Default)]
pub struct SessionIdentity {
    pub profile_name: String,
    pub locale: Option<String>,
    pub session_id: Option<String>,
}

/// Per-session proxy assignment held by the Crawler's proxy router.
#[derive(Debug, Clone)]
pub struct ProxyLease {
    pub url: url::Url,
    pub score: f32,
}

/// Per-attempt budgets the runner reads to fail-fast.
#[derive(Debug, Clone, Default)]
pub struct JobBudgets {
    pub render_ms_left: Option<u64>,
    pub total_ms_left: Option<u64>,
    pub attempts_remaining: u32,
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

// Remaining placeholders retained until B14 widens JobOutcome.

/// Placeholder retained because `JobOutcome.new_session_state` uses it.
/// Slice B14 widens the outcome to carry the real `antibot::SessionState`.
#[derive(Debug, Clone, Default)]
pub struct SessionStatePlaceholder;

/// Same retention reason — `JobOutcome.signals` continues to carry
/// placeholder Detections until B14 wires real Fingerprinter output.
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
    #[allow(deprecated)]
    detector: ChallengeDetector,
    /// Slice B14 (PRD #25): the unified `Fingerprinter` engine. When
    /// set, `run` calls `analyze_hot` after a successful fetch and
    /// merges the resulting antibot Detections into the retry
    /// decision (replaces the deprecated `ChallengeDetector` path).
    /// Optional today so existing tests that construct
    /// `JobRunner::new(fetcher)` keep working.
    fingerprinter: Option<std::sync::Arc<crate::fingerprint::Fingerprinter>>,
    /// Per-attempt lifecycle events are fired live through this sink
    /// (PRD #15 Q13). Optional so callers can run the runner in tests
    /// or in tools where the global NDJSON wire isn't relevant.
    events: Option<std::sync::Arc<dyn crate::events::EventSink>>,
}

impl JobRunner {
    pub fn new(fetcher: std::sync::Arc<dyn Fetcher>) -> Self {
        Self {
            fetcher,
            extractor: Extractor::new(),
            #[allow(deprecated)]
            detector: ChallengeDetector::new(),
            fingerprinter: None,
            events: None,
        }
    }

    /// Inject a `Fingerprinter` so the runner's antibot detection
    /// uses the unified engine (Hot tier — AntibotMarker + BlockPattern
    /// + the rest) rather than the deprecated `ChallengeDetector`.
    pub fn with_fingerprinter(
        mut self,
        fingerprinter: std::sync::Arc<crate::fingerprint::Fingerprinter>,
    ) -> Self {
        self.fingerprinter = Some(fingerprinter);
        self
    }

    /// Inject an `EventSink` so per-attempt lifecycle events fire live
    /// while `run` executes. Once the `Crawler::process_job` cutover
    /// lands (follow-up PR), the runner's sink becomes the only
    /// emitter of the per-attempt event subset — `Crawler` keeps
    /// run-level / decision events.
    pub fn with_events(mut self, events: std::sync::Arc<dyn crate::events::EventSink>) -> Self {
        self.events = Some(events);
        self
    }

    fn emit(&self, kind: crate::events::EventKind, url: Option<&url::Url>) {
        if let Some(sink) = &self.events {
            let mut ev = crate::events::Event::of(kind);
            if let Some(u) = url {
                ev = ev.with_url(u.as_str());
            }
            sink.emit(&ev);
        }
    }

    /// Execute one job. Pure of queue, storage, admission, and frontier
    /// concerns — the `Crawler` post-processes those.
    ///
    /// Outcome is returned by value (ADR-0001). Per-attempt lifecycle
    /// events fire live through the injected `EventSink`:
    /// `JobStarted` → `FetchCompleted` → `ChallengeDetected?` →
    /// `ExtractCompleted` on the success path, or `JobFailed` on the
    /// error path. The wire names match `EventKind` exactly, so the
    /// NDJSON contract is preserved when `Crawler::process_job` calls
    /// `runner.run` instead of inlining the dispatch (PRD #15 Q13).
    pub async fn run(&self, job: &crate::queue::Job, ctx: &SessionContext) -> JobOutcome {
        let start = std::time::Instant::now();
        self.emit(crate::events::EventKind::JobStarted, Some(&job.url));
        let fetch_started = std::time::Instant::now();
        let fetch_result = self.fetcher.fetch(job, ctx).await;
        let fetch_ms = Some(fetch_started.elapsed());

        match fetch_result {
            Ok(output) => {
                let status = output.status();
                let body_bytes = output.body().len();
                let final_url_owned = output.final_url().clone();
                self.emit(crate::events::EventKind::FetchCompleted, Some(&final_url_owned));
                let headers_cow = output.headers();
                // B14: Fingerprinter Hot tier is the new antibot
                // detection path. Falls back to legacy ChallengeDetector
                // when no Fingerprinter is injected so existing unit
                // tests keep working.
                let signal: Option<()> = if let Some(fp) = &self.fingerprinter {
                    let ctx = crate::fingerprint::TargetContext::http_only(
                        &final_url_owned,
                        status,
                        headers_cow.as_ref(),
                        output.body(),
                    );
                    let report = fp.analyze_hot(&ctx);
                    if !report.antibot.is_empty() {
                        Some(())
                    } else {
                        None
                    }
                } else {
                    #[allow(deprecated)]
                    self.detector
                        .detect(status, headers_cow.as_ref(), output.body())
                        .map(|_| ())
                };
                if signal.is_some() {
                    self.emit(
                        crate::events::EventKind::ChallengeDetected,
                        Some(&final_url_owned),
                    );
                }
                let extract_started = std::time::Instant::now();
                // Body decoded once; extract reuses the same slice.
                let body_str = String::from_utf8_lossy(output.body()).into_owned();
                let links = self
                    .extractor
                    .extract_links(&final_url_owned, &body_str)
                    .into_iter()
                    .map(|u| u.to_string())
                    .collect::<Vec<_>>();
                let extract_ms = Some(extract_started.elapsed());
                self.emit(
                    crate::events::EventKind::ExtractCompleted,
                    Some(&final_url_owned),
                );

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
                self.emit(crate::events::EventKind::JobFailed, Some(&job.url));
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
        response: parking_lot::Mutex<Option<crate::Result<FetchOutput>>>,
    }
    impl FakeFetcher {
        fn ok(status: u16, headers: HeaderMap, body: &[u8], final_url: Url) -> Arc<Self> {
            let resp = crate::impersonate::Response {
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
            };
            Arc::new(Self {
                response: parking_lot::Mutex::new(Some(Ok(FetchOutput::Http(resp)))),
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
        ) -> crate::Result<FetchOutput> {
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

    #[tokio::test]
    async fn run_emits_lifecycle_events_in_order_on_success() {
        let url: Url = "https://example.com/".parse().unwrap();
        let body = b"<html><body>ok</body></html>";
        let fake = FakeFetcher::ok(200, html_headers(), body, url.clone());
        let sink = std::sync::Arc::new(crate::events::MemorySink::create());
        let runner =
            JobRunner::new(fake as Arc<dyn Fetcher>).with_events(sink.clone() as Arc<dyn crate::events::EventSink>);
        let _ = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        let kinds: Vec<_> = sink.take().into_iter().map(|e| e.event).collect();
        assert_eq!(
            kinds,
            vec![
                crate::events::EventKind::JobStarted,
                crate::events::EventKind::FetchCompleted,
                crate::events::EventKind::ExtractCompleted,
            ]
        );
    }

    #[tokio::test]
    async fn run_emits_challenge_detected_before_extract() {
        let url: Url = "https://example.com/".parse().unwrap();
        let fake = FakeFetcher::ok(403, html_headers(), b"cf-chl-bypass", url);
        let sink = std::sync::Arc::new(crate::events::MemorySink::create());
        let runner =
            JobRunner::new(fake as Arc<dyn Fetcher>).with_events(sink.clone() as Arc<dyn crate::events::EventSink>);
        let _ = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        let kinds: Vec<_> = sink.take().into_iter().map(|e| e.event).collect();
        assert_eq!(
            kinds,
            vec![
                crate::events::EventKind::JobStarted,
                crate::events::EventKind::FetchCompleted,
                crate::events::EventKind::ChallengeDetected,
                crate::events::EventKind::ExtractCompleted,
            ]
        );
    }

    #[tokio::test]
    async fn run_emits_job_failed_on_error() {
        let err = crate::Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"));
        let fake = FakeFetcher::err(err);
        let sink = std::sync::Arc::new(crate::events::MemorySink::create());
        let runner =
            JobRunner::new(fake as Arc<dyn Fetcher>).with_events(sink.clone() as Arc<dyn crate::events::EventSink>);
        let _ = runner
            .run(&dummy_job("https://example.com/"), &SessionContext::default())
            .await;
        let kinds: Vec<_> = sink.take().into_iter().map(|e| e.event).collect();
        assert_eq!(
            kinds,
            vec![
                crate::events::EventKind::JobStarted,
                crate::events::EventKind::JobFailed,
            ]
        );
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
