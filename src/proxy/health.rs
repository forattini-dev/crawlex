//! Proxy health checker.
//!
//! Walks every proxy in the pool at `interval` and probes a canary URL via
//! `ImpersonateClient::get_via`. Each probe feeds a `ProxyOutcome` back into
//! the router so the score-driven pick logic sees health signals on the same
//! axis as real traffic — a slow proxy quarantines itself by accumulating
//! `Timeout` / `ConnectFailed` outcomes, no special ban boolean required.

use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

use crate::impersonate::{ImpersonateClient, Profile};
use crate::proxy::{ProxyOutcome, ProxyRouter};

const CANARY_URL: &str = "https://tls.peet.ws/api/clean";
const CANARY_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawn the health loop as a tokio task. The task loops forever (until the
/// process exits or the router is dropped), re-checks every proxy every
/// `interval`. Each probe result is reported via `record_outcome` — the
/// router's EWMA + consecutive-failure heuristics take it from there.
pub fn spawn(router: Arc<ProxyRouter>, proxies: Vec<Url>, interval: Duration) {
    if proxies.is_empty() || interval.is_zero() {
        return;
    }
    tokio::spawn(async move {
        // One client is enough: health checks are sequential per-proxy, and
        // the client doesn't cache anything keyed on proxy.
        let client = match ImpersonateClient::new(Profile::Chrome131Stable) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(?e, "proxy health: client init failed; aborting");
                return;
            }
        };
        let canary = match Url::parse(CANARY_URL) {
            Ok(u) => u,
            Err(_) => return,
        };
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // First tick completes immediately — we still want an initial probe
        // at startup so unhealthy proxies get quarantined before the first
        // real crawl request.
        tick.tick().await;
        loop {
            for proxy in &proxies {
                let started = Instant::now();
                let res = tokio::time::timeout(
                    CANARY_TIMEOUT,
                    client.get_via(
                        &canary,
                        Some(proxy),
                        crate::discovery::assets::SecFetchDest::Document,
                    ),
                )
                .await;
                let outcome = match res {
                    Ok(Ok(_)) => ProxyOutcome::Success {
                        latency_ms: started.elapsed().as_secs_f64() * 1_000.0,
                    },
                    Ok(Err(_)) => ProxyOutcome::ConnectFailed,
                    Err(_) => ProxyOutcome::Timeout,
                };
                router.record_outcome(proxy, outcome);
            }
            tick.tick().await;
        }
    });
}
