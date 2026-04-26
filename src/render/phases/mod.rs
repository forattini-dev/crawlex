//! Page-render lifecycle phases.
//!
//! `RenderPhase` is the deepened seam that breaks `pool.rs::render_with_script`
//! out of its 3 200-LOC monolith into discrete steps each with a single
//! responsibility. The pipeline runs them in order:
//!
//!   1. **`PreNavigatePhase`** — fresh page setup that has to happen
//!      *before* the URL load: stealth shim injection, SPA observer
//!      install, viewport pin, custom headers, motion engine warm-up.
//!   2. **`PostNavigatePhase`** — dispatch the actual `Page.navigate`
//!      and resolve the wait strategy (load / DOMContentLoaded /
//!      networkIdle / selector / fixed).
//!   3. **`SettlePhase`** — reading-dwell, post-navigate motion
//!      animations, antibot challenge detection.
//!   4. **`CollectPhase`** — gather post-render artefacts: post-JS HTML,
//!      runtime routes, network endpoints, IndexedDB, CacheStorage,
//!      manifest, service workers, web vitals, resource waterfall.
//!   5. **`CapturePhase`** — last-mile binary capture: screenshot per
//!      mode (viewport / fullpage / element).
//!
//! State flows between phases via [`RenderState`] — a mutable scratch
//! struct that accumulates everything callers eventually want from the
//! `RenderedPage` return shape.
//!
//! ## Status
//!
//! This module lands the trait + state + phase scaffolding. Existing
//! `pool.rs::render_with_script` still does its 5 lifecycle steps inline;
//! migration to the phase composition is incremental and tracked per
//! phase in subsequent commits.

#![cfg(feature = "cdp-backend")]

use async_trait::async_trait;

use crate::error::Result;
use crate::render::session::BrowserSessionLike;

pub mod state;

pub use state::RenderState;

/// One step of the page lifecycle. Phases share state via [`RenderState`]
/// and use the [`BrowserSessionLike`] facade to drive the page — they
/// never reach for `chrome::Page` directly, so test fixtures plug in a
/// mock session without standing up Chrome.
#[async_trait]
pub trait RenderPhase: Send + Sync {
    /// Stable name for tracing / telemetry. Returned via `name()` (not a
    /// const) so wrapper phases (e.g. a `Conditional<Inner>`) can prefix
    /// the inner name when useful.
    fn name(&self) -> &'static str;

    /// Run the phase. Phases mutate `state` and may issue arbitrary
    /// `session` operations; they MUST NOT panic on partial failure —
    /// surface as `Err` and the pipeline decides whether to halt or
    /// continue per phase policy.
    async fn run(&self, session: &dyn BrowserSessionLike, state: &mut RenderState) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::session::tests::MockBrowserSession;
    use parking_lot::Mutex;
    use std::sync::Arc;

    struct RecordingPhase {
        name: &'static str,
        runs: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl RenderPhase for RecordingPhase {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn run(
            &self,
            _session: &dyn BrowserSessionLike,
            _state: &mut RenderState,
        ) -> Result<()> {
            self.runs.lock().push(self.name);
            Ok(())
        }
    }

    #[tokio::test]
    async fn phases_run_in_order_with_mock_session() {
        let runs: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let phases: Vec<Box<dyn RenderPhase>> = vec![
            Box::new(RecordingPhase {
                name: "pre",
                runs: runs.clone(),
            }),
            Box::new(RecordingPhase {
                name: "post",
                runs: runs.clone(),
            }),
            Box::new(RecordingPhase {
                name: "settle",
                runs: runs.clone(),
            }),
        ];
        let session = MockBrowserSession::new();
        let mut state = RenderState::default();
        for phase in &phases {
            phase.run(&session, &mut state).await.unwrap();
        }
        assert_eq!(*runs.lock(), vec!["pre", "post", "settle"]);
    }
}
