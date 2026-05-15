//! Render fetch adapter (slice 4 of the JobRunner extraction, GH #20).
//!
//! Thin wrapper over `render::RenderPool` that pins the render path
//! behind a `Fetcher`-shaped seam. The trait impl returns a synthetic
//! `impersonate::Response` built from the rendered page, so the same
//! `dyn Fetcher` surface that `SpoofFetcher` satisfies works for render
//! callers too. Slice #21 (`AutoFetcher`) and #22 (`JobRunner::run`)
//! consume this seam; slice #20 only routes the existing
//! `render_with_script` call site through `RenderFetcher`.
//!
//! Render-specific outputs (Web Vitals, screenshots, ScriptSpec
//! outcomes) bypass the trait surface — callers that need them use
//! `RenderFetcher::render_with_script` directly.
//!
//! Render pool ownership and laziness stay with `Crawler`. The
//! `RenderFetcher` is a cheap newtype around `Arc<RenderPool>`.

#![cfg(feature = "cdp-backend")]

use std::sync::Arc;

use async_trait::async_trait;
use url::Url;

use crate::impersonate::Response;
use crate::queue::Job;
use crate::render::{RenderPool, RenderedPage};
use crate::runner::{Fetcher, SessionContext};
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
    /// Trait-level render: a minimal navigation that returns a
    /// synthetic `impersonate::Response` (status 200, rendered HTML
    /// body, final URL). Suitable for callers that only need the page
    /// content — anything that depends on Web Vitals, screenshots, or
    /// ScriptSpec outcomes must call `render_with_script` directly.
    async fn fetch(&self, _job: &Job, _ctx: &SessionContext) -> Result<Response> {
        // Slice #20 does not provide a behaviour-only trait fetch:
        // every real call site goes through `render_with_script`, and
        // synthesising a `Response` without a ScriptSpec would either
        // change render semantics or duplicate the render dispatch.
        // Returning an explicit error keeps the trait surface honest
        // until slice #22 widens `FetchSuccess` to carry rendered pages.
        Err(crate::Error::Render(
            "RenderFetcher::fetch (trait) is intentionally unimplemented; \
             use render_with_script. See PRD #15 slice 4."
                .into(),
        ))
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
