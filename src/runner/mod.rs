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
pub use fetcher::{Fetcher, SpoofFetcher};
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

/// Top-level runner. Slice 0: empty shell. Slice #22 promotes
/// `JobRunner::run` to the per-Job entry point called by `Crawler::process_job`.
///
/// Held as `Arc<JobRunner>` and shared across workers. `Send + Sync`,
/// stateless on `self`.
#[derive(Debug, Default)]
pub struct JobRunner;

impl JobRunner {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

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
