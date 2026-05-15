//! Fetcher seam (slice 1 of the JobRunner extraction, GH #17).
//!
//! `Fetcher` is the trait that later slices use to swap between the
//! spoof, render, and auto paths without touching `Crawler::process_job`.
//! Slice 1 ships the trait and the `SpoofFetcher` implementation that
//! wraps the existing `ImpersonateClient`; slices #20–#21 add the render
//! and auto adapters.
//!
//! See ADR-0001 for the value-return outcome rule and ADR-0002 for the
//! re-queue escalation contract.

use std::sync::Arc;

use async_trait::async_trait;
use url::Url;

use crate::discovery::assets::SecFetchDest;
use crate::impersonate::{ImpersonateClient, Response};
use crate::queue::Job;
use crate::Result;

use super::SessionContext;

/// One adapter per fetch path. Slice 1 has a single concrete impl
/// (`SpoofFetcher`); render and auto join in later slices.
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, job: &Job, ctx: &SessionContext) -> Result<Response>;
}

/// HTTP impersonation adapter. Thin wrapper over `ImpersonateClient`
/// that funnels every "spoof" fetch through one type, so later slices
/// can change behavior here without touching call sites in `crawler.rs`.
pub struct SpoofFetcher {
    client: Arc<ImpersonateClient>,
}

impl SpoofFetcher {
    pub fn new(client: Arc<ImpersonateClient>) -> Self {
        Self { client }
    }

    /// Underlying client — exposed so the per-attempt dispatch (timed,
    /// via proxy, with explicit `Sec-Fetch-Dest`) can stay in `crawler.rs`
    /// for now and be folded into the trait surface in later slices.
    pub fn client(&self) -> &ImpersonateClient {
        &self.client
    }

    /// Per-attempt fetch with the full set of knobs the spoof path
    /// currently honours: explicit `Sec-Fetch-Dest`, optional proxy
    /// override, and whether to collect per-phase network timings.
    pub async fn fetch_with(
        &self,
        url: &Url,
        dest: SecFetchDest,
        proxy: Option<&Url>,
        timed: bool,
    ) -> Result<Response> {
        match (proxy, timed) {
            (Some(p), true) => self.client.get_timed_via(url, Some(p), dest).await,
            (Some(p), false) => self.client.get_via(url, Some(p), dest).await,
            (None, true) => self.client.get_timed_with_dest(url, dest).await,
            (None, false) => {
                if matches!(dest, SecFetchDest::Document) {
                    self.client.get(url).await
                } else {
                    self.client.get_with_dest(url, dest).await
                }
            }
        }
    }
}

#[async_trait]
impl Fetcher for SpoofFetcher {
    async fn fetch(&self, job: &Job, _ctx: &SessionContext) -> Result<Response> {
        // Default trait impl picks Document dest with no proxy override.
        // The richer dispatch surface lives on `fetch_with`; later slices
        // promote it into the trait signature once `SessionContext` grows
        // proxy and timing fields.
        self.client.get(&job.url).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn fetcher_trait_is_object_safe() {
        // If this ever stops compiling, the trait grew a generic — that
        // breaks `Arc<dyn Fetcher>` and the swap-by-Method routing.
        fn _accepts(_f: Arc<dyn Fetcher>) {}
    }

    #[test]
    fn spoof_fetcher_is_send_sync() {
        assert_send_sync::<SpoofFetcher>();
    }

    #[test]
    fn spoof_fetcher_constructs() {
        let profile = crate::impersonate::Profile::Chrome131Stable;
        let client = Arc::new(ImpersonateClient::new(profile).expect("client builds"));
        let f = SpoofFetcher::new(client.clone());
        // Identity assertion via pointer equality — confirms the wrapper
        // holds the same Arc rather than rebuilding the client.
        assert!(std::ptr::eq(&*f.client as *const _, &*client as *const _));
    }
}
