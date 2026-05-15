//! Render fetch adapter (slice 4 of the JobRunner extraction, GH #20;
//! widened to honest `FetchOutput::Rendered` in A1, GH #26).
//!
//! Thin wrapper over `render::RenderPool`. The trait impl now returns
//! `FetchOutput::Rendered(Box<RenderedPage>)` honestly — no more
//! "intentionally unimplemented" guard. The basic render path uses a
//! default `ScriptSpec` (no steps); callers that need scripted render
//! continue to use `render_with_script` directly for the full
//! `(RenderedPage, RunOutcome)` tuple.
//!
//! Render pool ownership and laziness stay with `Crawler`.

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;

use async_trait::async_trait;
use url::Url;

use crate::queue::Job;
use crate::render::{RenderPool, RenderedPage};
use crate::runner::{FetchOutput, Fetcher, SessionContext};
use crate::Result;

pub struct RenderFetcher {
    pool: Arc<RenderPool>,
}

impl RenderFetcher {
    pub fn new(pool: Arc<RenderPool>) -> Self {
        Self { pool }
    }

    /// Underlying pool — exposed so callers that need render-only data
    /// (Vitals, screenshots, ScriptSpec outcomes) can reach past the
    /// `Fetcher` surface while we incrementally narrow the seam.
    pub fn pool(&self) -> &Arc<RenderPool> {
        &self.pool
    }

    /// Direct passthrough to `RenderPool::render_with_script`. Slices
    /// after #21 progressively pull this surface up to the
    /// `Fetcher` trait once `JobOutcome` / `FetchSuccess` can carry the
    /// richer render payload without lossy conversions.
    pub async fn render_with_script(
        &self,
        url: &Url,
        wait: &crate::wait_strategy::WaitStrategy,
        script: &crate::script::ScriptSpec,
        events: Option<Arc<dyn crate::events::EventSink>>,
        run_id: Option<u64>,
        proxy: Option<&Url>,
    ) -> Result<(RenderedPage, crate::script::RunOutcome)> {
        self.pool
            .render_with_script(url, wait, script, events, run_id, proxy)
            .await
    }
}

#[async_trait]
impl Fetcher for RenderFetcher {
    /// Trait-level render: drives `render_with_script` with a default
    /// (empty) `ScriptSpec` and returns the resulting `RenderedPage`
    /// wrapped in `FetchOutput::Rendered`. Callers that need scripted
    /// render plus the per-step `RunOutcome` use `render_with_script`
    /// directly to get the richer tuple.
    async fn fetch(&self, job: &Job, _ctx: &SessionContext) -> Result<FetchOutput> {
        let script = empty_script_spec();
        let wait = crate::wait_strategy::WaitStrategy::default();
        let (page, _outcome) = self
            .pool
            .render_with_script(&job.url, &wait, &script, None, None, None)
            .await?;
        Ok(FetchOutput::Rendered(Box::new(page)))
    }
}

/// Empty `ScriptSpec` — version-correct, no steps. Used by the trait
/// `fetch` to drive a basic render without scripted automation.
fn empty_script_spec() -> crate::script::ScriptSpec {
    crate::script::ScriptSpec {
        version: crate::script::SCRIPT_SPEC_VERSION,
        defaults: Default::default(),
        selectors: Default::default(),
        steps: Vec::new(),
        captures: Vec::new(),
        assertions: Vec::new(),
        exports: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_fetcher_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RenderFetcher>();
    }

    #[test]
    fn render_fetcher_is_object_safe() {
        fn _accepts(_f: Arc<dyn Fetcher>) {}
    }
}
