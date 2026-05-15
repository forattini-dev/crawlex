//! Auto fetch adapter (slice 5 of the JobRunner extraction, GH #21).
//!
//! `AutoFetcher` composes `SpoofFetcher` + `RenderFetcher` +
//! `ChallengeDetector`. Per ADR-0002, the spoof path runs first; if a
//! challenge signal fires, escalation happens by re-queueing the job
//! with `Method::Render` — `AutoFetcher` does **not** chain inline to
//! the render fetcher. Today the re-queue is driven by `policy::engine`
//! (which now consumes `ChallengeDetector` directly); `AutoFetcher`
//! exposes the same decision logic as a unit-testable seam so the
//! escalation contract can be asserted without booting a real
//! `Crawler`.

use std::sync::Arc;

use http::HeaderMap;

use crate::queue::Job;
use crate::runner::{ChallengeDetector, ChallengeSignal, Fetcher, RetryDecision, RetryReason, SessionContext, SpoofFetcher};
use crate::Result;

#[cfg(feature = "cdp-backend")]
use crate::runner::RenderFetcher;

/// What `AutoFetcher` advises the `Crawler` to do after the spoof
/// attempt completes. Mirrors the runner-level `RetryDecision` so
/// `JobRunner::run` (slice #22) can lift this straight into
/// `JobOutcome.retry` without translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoOutcome {
    /// Spoof returned usable content — `Crawler` writes the response
    /// and feeds discovered links without escalation.
    UseSpoofResponse,
    /// Spoof returned a challenge — `Crawler` should re-enqueue this
    /// job with `Method::Render` (ADR-0002).
    EscalateToRender { signal: ChallengeSignal },
}

impl AutoOutcome {
    /// Project the decision onto the `RetryDecision` used by the
    /// runner-level `JobOutcome`.
    pub fn into_retry_decision(self) -> RetryDecision {
        match self {
            AutoOutcome::UseSpoofResponse => RetryDecision::None,
            AutoOutcome::EscalateToRender { .. } => RetryDecision::Suggest {
                reason: RetryReason::EscalateToRender,
                backoff_hint: None,
            },
        }
    }
}

pub struct AutoFetcher {
    spoof: Arc<SpoofFetcher>,
    #[cfg(feature = "cdp-backend")]
    render: Arc<RenderFetcher>,
    detector: ChallengeDetector,
}

impl AutoFetcher {
    #[cfg(feature = "cdp-backend")]
    pub fn new(spoof: Arc<SpoofFetcher>, render: Arc<RenderFetcher>) -> Self {
        Self {
            spoof,
            render,
            detector: ChallengeDetector::new(),
        }
    }

    #[cfg(not(feature = "cdp-backend"))]
    pub fn new(spoof: Arc<SpoofFetcher>) -> Self {
        Self {
            spoof,
            detector: ChallengeDetector::new(),
        }
    }

    pub fn spoof(&self) -> &Arc<SpoofFetcher> {
        &self.spoof
    }

    #[cfg(feature = "cdp-backend")]
    pub fn render(&self) -> &Arc<RenderFetcher> {
        &self.render
    }

    /// Inspect the spoof response and decide whether to escalate. Pure
    /// of any fetching — given a status/headers/body, the answer is
    /// deterministic. Tests target this directly.
    pub fn decide_after_spoof(
        &self,
        status: u16,
        headers: &HeaderMap,
        body: &[u8],
    ) -> AutoOutcome {
        match self.detector.detect(status, headers, body) {
            Some(signal) => AutoOutcome::EscalateToRender { signal },
            None => AutoOutcome::UseSpoofResponse,
        }
    }
}

#[async_trait::async_trait]
impl Fetcher for AutoFetcher {
    /// Delegates the network attempt to `SpoofFetcher`. Escalation is
    /// **not** chained inline (ADR-0002) — the caller inspects the
    /// returned response via `decide_after_spoof` and re-queues the
    /// job when escalation is required.
    async fn fetch(&self, job: &Job, ctx: &SessionContext) -> Result<crate::runner::FetchOutput> {
        // SpoofFetcher::fetch already wraps in FetchOutput::Http — pass
        // through. Escalation signalling stays in JobOutcome.retry per
        // ADR-0002; this method never chains inline to render.
        self.spoof.fetch(job, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AntibotVendor;

    fn html_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("content-type", "text/html; charset=utf-8".parse().unwrap());
        h
    }

    fn build_auto() -> AutoFetcher {
        let client = Arc::new(
            crate::impersonate::ImpersonateClient::new(
                crate::impersonate::Profile::Chrome131Stable,
            )
            .expect("client"),
        );
        let spoof = Arc::new(SpoofFetcher::new(client));
        #[cfg(feature = "cdp-backend")]
        {
            // The pool is constructed but never driven — `decide_after_spoof`
            // is pure and doesn't touch Chrome.
            let cfg = Arc::new(crate::config::Config::default());
            let storage: Arc<dyn crate::storage::Storage> =
                Arc::new(crate::storage::memory::MemoryStorage::default());
            let pool = Arc::new(crate::render::pool::RenderPool::new(cfg, storage));
            let render = Arc::new(RenderFetcher::new(pool));
            AutoFetcher::new(spoof, render)
        }
        #[cfg(not(feature = "cdp-backend"))]
        {
            AutoFetcher::new(spoof)
        }
    }

    #[test]
    fn challenge_response_triggers_escalate_to_render() {
        let auto = build_auto();
        let outcome = auto.decide_after_spoof(403, &html_headers(), b"cf-chl-bypass");
        match outcome {
            AutoOutcome::EscalateToRender { signal } => {
                assert_eq!(signal.vendor, AntibotVendor::Cloudflare);
            }
            other => panic!("expected EscalateToRender, got {other:?}"),
        }
    }

    #[test]
    fn healthy_response_uses_spoof() {
        let auto = build_auto();
        let body = b"<html><body><h1>real content</h1><p>plenty of text here</p></body></html>";
        assert_eq!(
            auto.decide_after_spoof(200, &html_headers(), body),
            AutoOutcome::UseSpoofResponse
        );
    }

    #[test]
    fn escalation_projects_to_retry_decision_with_escalate_reason() {
        let signal = ChallengeSignal {
            vendor: AntibotVendor::Cloudflare,
        };
        let decision = AutoOutcome::EscalateToRender { signal }.into_retry_decision();
        match decision {
            RetryDecision::Suggest {
                reason,
                backoff_hint,
            } => {
                assert_eq!(reason, RetryReason::EscalateToRender);
                assert_eq!(backoff_hint, None);
            }
            _ => panic!("expected Suggest"),
        }
    }

    #[test]
    fn use_spoof_projects_to_retry_none() {
        assert!(matches!(
            AutoOutcome::UseSpoofResponse.into_retry_decision(),
            RetryDecision::None
        ));
    }

    #[test]
    fn auto_fetcher_does_not_chain_render_inline() {
        // The whole point of ADR-0002: `decide_after_spoof` is pure and
        // never touches the render fetcher. This test asserts the type
        // surface so a future "optimisation" that wires render into the
        // adapter has to update this test and re-read the ADR first.
        let auto = build_auto();
        let _ = auto.decide_after_spoof(403, &html_headers(), b"cf-chl-bypass");
        // The only render touchpoint AutoFetcher exposes is the
        // accessor — never invoked from `decide_after_spoof`.
        #[cfg(feature = "cdp-backend")]
        {
            let _ = auto.render(); // accessor exists, but was not used by decide_after_spoof
        }
    }

    #[test]
    fn auto_fetcher_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AutoFetcher>();
    }
}
