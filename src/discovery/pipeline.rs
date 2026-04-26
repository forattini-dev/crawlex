//! Sequential discovery pipeline.
//!
//! `DiscoveryPipeline` is the deepened module that replaces the old
//! "import 19 free functions and remember which ones to call" pattern.
//! Callers hand it a `DiscoveryContext` (target + http + budgets +
//! features), it walks a fixed-order list of `Discoverer` adapters,
//! collects their `Finding`s, and returns the lot.
//!
//! Order is **deterministic** (decided by the pipeline, not the caller):
//! * `Dns` — resolve A/AAAA/CNAME/MX/TXT/NS/CAA. Cheap. Always first.
//! * `Whois` — registrar + nameservers. One HTTP/1 call to RDAP.
//! * `Cert` — TLS peer cert harvest (only emits when `features.peer_cert`).
//! * `CrtSh` — certificate transparency subdomain enumeration. Slow + opt-in.
//! * `RobotsPaths` — fetch + parse robots.txt for sitemap hints.
//! * `Sitemap` — recurse sitemap.xml.
//! * `WellKnown` — `.well-known/*` probes.
//! * `SecurityTxt` — parse the canonical security.txt.
//! * `Pwa` — manifest + service worker probes.
//! * `Wayback` — CDX history (slow + opt-in).
//! * `NetworkProbe` — TCP port scan against resolved IPs (heavy + opt-in).
//!
//! Each step is wrapped in `tokio::time::timeout(ctx.budget, ...)` as
//! defence in depth — a misbehaving discoverer can't stall the whole
//! pipeline. Failures of optional steps emit `tracing::warn!` and continue;
//! failures of mandatory steps surface as `DiscoveryError`.

use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

use crate::discovery::types::{DiscoveryContext, Finding};

/// One step of the host-level discovery pipeline. Each adapter wraps an
/// existing free function in `src/discovery/<topic>.rs`.
#[async_trait]
pub trait Discoverer: Send + Sync {
    /// Stable name for tracing / telemetry. Returned via `name()` (not a
    /// const) so adapters can include parameter info if useful.
    fn name(&self) -> &'static str;

    /// Whether this discoverer should run for the given context. Default
    /// implementation returns `true` so callers can opt out via
    /// `DiscoveryContext::features`.
    fn enabled(&self, _ctx: &DiscoveryContext) -> bool {
        true
    }

    /// Execute the discovery step. Adapters MUST return whatever they
    /// observe even if partial — pipeline cancellation handles the
    /// "gave up after budget" case.
    async fn discover(&self, ctx: &DiscoveryContext) -> Result<Vec<Finding>, DiscoveryError>;
}

/// Errors a discoverer can surface. Most callers downgrade these to
/// `tracing::warn!` and continue; only catastrophic infrastructure
/// failures (panicked task, OOM) bubble out of the pipeline.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("discoverer `{name}` exceeded its budget ({budget_ms}ms)")]
    Timeout { name: &'static str, budget_ms: u64 },

    #[error("discoverer `{name}` returned an error: {message}")]
    Backend {
        name: &'static str,
        message: String,
    },
}

/// Sequential pipeline that walks a fixed list of `Discoverer`s.
///
/// Order is the responsibility of the constructor — callers don't get to
/// reshuffle. To add a new discoverer, edit
/// `DiscoveryPipeline::default_order` (or assemble a custom pipeline via
/// `DiscoveryPipeline::with_discoverers`).
pub struct DiscoveryPipeline {
    discoverers: Vec<Box<dyn Discoverer>>,
}

impl DiscoveryPipeline {
    /// Build the pipeline with a custom discoverer list. Most callers want
    /// `default_order` instead.
    pub fn with_discoverers(discoverers: Vec<Box<dyn Discoverer>>) -> Self {
        Self { discoverers }
    }

    /// Run every enabled discoverer in order, returning the concatenated
    /// `Finding` list. Failures of individual discoverers are logged and
    /// skipped — the pipeline always returns a (possibly partial) result.
    pub async fn run(&self, ctx: &DiscoveryContext) -> Vec<Finding> {
        let mut findings: Vec<Finding> = Vec::new();
        for discoverer in &self.discoverers {
            if !discoverer.enabled(ctx) {
                tracing::trace!(
                    target: "crawlex::discovery",
                    name = discoverer.name(),
                    "discoverer disabled by feature gate"
                );
                continue;
            }
            let name = discoverer.name();
            let budget_ms = ctx.budget.as_millis() as u64;
            let started = std::time::Instant::now();
            match tokio::time::timeout(ctx.budget, discoverer.discover(ctx)).await {
                Ok(Ok(mut more)) => {
                    let count = more.len();
                    findings.append(&mut more);
                    tracing::debug!(
                        target: "crawlex::discovery",
                        name,
                        count,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "discoverer ok"
                    );
                }
                Ok(Err(err)) => {
                    tracing::warn!(
                        target: "crawlex::discovery",
                        name,
                        ?err,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "discoverer failed; continuing"
                    );
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        target: "crawlex::discovery",
                        name,
                        budget_ms,
                        "discoverer timed out; continuing"
                    );
                }
            }
        }
        findings
    }

    /// Default per-module budget when callers don't override.
    pub const DEFAULT_BUDGET: Duration = Duration::from_secs(30);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::types::{DiscoveryContext, DiscoveryFeatures};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    /// Dummy discoverer for ordering / cancellation tests. Records the
    /// call order in a shared counter and emits a `Fact` finding so the
    /// caller can verify what ran.
    struct Probe {
        name: &'static str,
        order: StdArc<AtomicUsize>,
        recorded_at: StdArc<parking_lot::Mutex<Vec<(usize, &'static str)>>>,
        delay: Duration,
        fail: bool,
    }

    #[async_trait]
    impl Discoverer for Probe {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn discover(
            &self,
            _ctx: &DiscoveryContext,
        ) -> Result<Vec<Finding>, DiscoveryError> {
            let pos = self.order.fetch_add(1, Ordering::SeqCst);
            self.recorded_at.lock().push((pos, self.name));
            if self.delay > Duration::ZERO {
                tokio::time::sleep(self.delay).await;
            }
            if self.fail {
                return Err(DiscoveryError::Backend {
                    name: self.name,
                    message: "synthetic".into(),
                });
            }
            Ok(vec![Finding::Fact {
                key: self.name.into(),
                value: serde_json::json!(true),
            }])
        }
    }

    fn fake_ctx(budget: Duration) -> DiscoveryContext {
        // Tests don't actually issue HTTP — but ImpersonateClient::new
        // is cheap enough we just construct it. If a future probe truly
        // needs network, isolate it under `#[tokio::test] #[ignore]`.
        let http = StdArc::new(
            crate::impersonate::ImpersonateClient::new(crate::impersonate::Profile::Chrome149Stable)
                .expect("ImpersonateClient builds in tests"),
        );
        DiscoveryContext {
            target: "example.com".into(),
            host: None,
            http,
            budget,
            features: DiscoveryFeatures::default(),
        }
    }

    #[tokio::test]
    async fn pipeline_runs_in_order() {
        let order = StdArc::new(AtomicUsize::new(0));
        let recorded = StdArc::new(parking_lot::Mutex::new(Vec::new()));
        let pipeline = DiscoveryPipeline::with_discoverers(vec![
            Box::new(Probe {
                name: "alpha",
                order: order.clone(),
                recorded_at: recorded.clone(),
                delay: Duration::ZERO,
                fail: false,
            }),
            Box::new(Probe {
                name: "beta",
                order: order.clone(),
                recorded_at: recorded.clone(),
                delay: Duration::ZERO,
                fail: false,
            }),
            Box::new(Probe {
                name: "gamma",
                order: order.clone(),
                recorded_at: recorded.clone(),
                delay: Duration::ZERO,
                fail: false,
            }),
        ]);
        let _ = pipeline.run(&fake_ctx(Duration::from_secs(5))).await;
        let recorded = recorded.lock().clone();
        assert_eq!(
            recorded,
            vec![(0, "alpha"), (1, "beta"), (2, "gamma")],
            "pipeline must invoke discoverers in order"
        );
    }

    #[tokio::test]
    async fn pipeline_continues_after_failure() {
        let order = StdArc::new(AtomicUsize::new(0));
        let recorded = StdArc::new(parking_lot::Mutex::new(Vec::new()));
        let pipeline = DiscoveryPipeline::with_discoverers(vec![
            Box::new(Probe {
                name: "first",
                order: order.clone(),
                recorded_at: recorded.clone(),
                delay: Duration::ZERO,
                fail: true,
            }),
            Box::new(Probe {
                name: "second",
                order: order.clone(),
                recorded_at: recorded.clone(),
                delay: Duration::ZERO,
                fail: false,
            }),
        ]);
        let findings = pipeline.run(&fake_ctx(Duration::from_secs(5))).await;
        // First failed → no finding from it. Second ran normally → 1 fact.
        assert_eq!(findings.len(), 1, "pipeline should keep going after err");
        let recorded = recorded.lock().clone();
        assert_eq!(recorded.len(), 2, "both probes executed");
    }

    #[tokio::test]
    async fn pipeline_enforces_budget() {
        let order = StdArc::new(AtomicUsize::new(0));
        let recorded = StdArc::new(parking_lot::Mutex::new(Vec::new()));
        let pipeline = DiscoveryPipeline::with_discoverers(vec![
            Box::new(Probe {
                name: "slow",
                order: order.clone(),
                recorded_at: recorded.clone(),
                delay: Duration::from_millis(500),
                fail: false,
            }),
            Box::new(Probe {
                name: "fast",
                order: order.clone(),
                recorded_at: recorded.clone(),
                delay: Duration::ZERO,
                fail: false,
            }),
        ]);
        // Budget < slow's delay → slow must time out, fast must still run.
        let findings = pipeline.run(&fake_ctx(Duration::from_millis(50))).await;
        assert_eq!(
            findings.len(),
            1,
            "only the fast probe should produce a finding"
        );
    }
}
