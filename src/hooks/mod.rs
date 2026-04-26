//! Lifecycle hooks — pluggable callbacks fired at well-known points in
//! the crawl pipeline.
//!
//! Three integration paths share the same registry:
//!
//! 1. **Rust-native** — `HookRegistry::on_<event>(closure)`. Cheapest
//!    (no IPC, no script eval). Use when embedding `crawlex` as a
//!    library: closures see the typed `HookContext`, return a
//!    `HookDecision`, can mutate `ctx.captured_urls` to enqueue
//!    discovered URLs without going through the discovery pipeline.
//! 2. **Lua scripts** — `--hook-script foo.lua`. Loaded by
//!    [`lua::LuaHookHost`] when the `lua-hooks` feature is on. Same
//!    fire order; lua hooks run AFTER rust-native ones.
//! 3. **JS hook bridge** — `--hook-bridge fd:N` (planned, see
//!    `docs/hooks-js.md`). The SDK's `defineHooks({...})` registers
//!    JS callbacks that talk to the crawler over an IPC channel.
//!
//! ## Decision semantics
//!
//! Hooks fire in registration order per event; the **first non-Continue**
//! decision wins for that event. To run every hook regardless of the
//! result, return `HookDecision::Continue` and use `ctx.user_data` to
//! pass observations between hooks.
//!
//! ## Examples
//!
//! ```no_run
//! use crawlex::{Crawler, ConfigBuilder};
//! use crawlex::hooks::{HookDecision, HookRegistry};
//!
//! # async fn doc() -> crawlex::Result<()> {
//! let hooks = HookRegistry::new();
//!
//! // 429 → ask the pipeline to retry the job.
//! hooks.on_after_first_byte(|ctx| {
//!     let status = ctx.response_status;
//!     Box::pin(async move {
//!         match status {
//!             Some(429) | Some(503) => Ok(HookDecision::Retry),
//!             _ => Ok(HookDecision::Continue),
//!         }
//!     })
//! });
//!
//! // Inject extra URLs into the discovery queue.
//! hooks.on_discovery(|ctx| {
//!     Box::pin(async move {
//!         let extra = url::Url::parse(&format!("{}/sitemap.xml", ctx.url))
//!             .ok();
//!         if let Some(u) = extra {
//!             ctx.captured_urls.push(u);
//!         }
//!         Ok(HookDecision::Continue)
//!     })
//! });
//!
//! let config = ConfigBuilder::new().build();
//! let crawler = Crawler::new(config).await?.with_hooks(hooks);
//! # Ok(()) }
//! ```

pub mod bridge;
pub mod context;
pub mod events;
#[cfg(feature = "lua-hooks")]
pub mod lua;

pub use bridge::{
    event_from_wire, event_wire_name, BridgeChannel, BridgeHookAdapter, BridgeInbound,
    BridgeOutbound, ContextPatch, WireContext, WireDecision, HOOK_BRIDGE_PROTOCOL_VERSION,
};
pub use context::HookContext;
pub use events::HookEvent;

use futures::future::BoxFuture;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::Result;

/// What a hook tells the pipeline to do next.
///
/// Hooks fire in registration order; the first non-`Continue` decision
/// wins. To observe without intervening, return `Continue` and stash
/// observations in `ctx.user_data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookDecision {
    /// Carry on with the pipeline. Subsequent hooks for the same event
    /// still fire.
    Continue,
    /// Drop the current job. The crawler skips remaining steps for
    /// this URL but does not fail the run.
    Skip,
    /// Re-enqueue the job with the next-attempt counter incremented.
    /// Honoured only on events where retry is meaningful
    /// (`OnError`, `AfterFirstByte`, `OnResponseBody`).
    Retry,
    /// Halt the crawl. The current run terminates as if a fatal error
    /// had been raised.
    Abort,
}

/// Boxed hook closure — async, takes `&mut HookContext`, returns a
/// `HookDecision`. The lifetime parameter ties the future to the
/// borrowed context so callers can mutate `ctx` mid-await.
pub type HookFn = Arc<
    dyn for<'a> Fn(&'a mut HookContext) -> futures::future::BoxFuture<'a, Result<HookDecision>>
        + Send
        + Sync,
>;

/// Per-event registry. Cheap to clone (`Arc` inside) so the same
/// instance can be passed to nested components.
#[derive(Default)]
pub struct HookRegistry {
    inner: RwLock<HashMap<HookEvent, Vec<HookFn>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lowest-level register: any closure matching the boxed `HookFn`
    /// signature. The typed helpers below are sugar on top of this and
    /// should be preferred.
    pub fn on(&self, event: HookEvent, f: HookFn) {
        self.inner.write().entry(event).or_default().push(f);
    }

    /// Generic typed register. The closure takes `&mut HookContext`
    /// and returns a `BoxFuture<'a, ...>` — write the body with
    /// `Box::pin(async move { … })`. Most callers prefer one of the
    /// per-event helpers below, which call this internally.
    pub fn register<F>(&self, event: HookEvent, f: F)
    where
        F: for<'a> Fn(&'a mut HookContext) -> BoxFuture<'a, Result<HookDecision>>
            + Send
            + Sync
            + 'static,
    {
        self.on(event, Arc::new(f));
    }

    /// Run every registered hook for `event`. Iterates in registration
    /// order; the first non-`Continue` decision is returned.
    pub async fn fire(&self, event: HookEvent, ctx: &mut HookContext) -> Result<HookDecision> {
        let hooks = self.inner.read().get(&event).cloned().unwrap_or_default();
        for h in hooks {
            match h(ctx).await? {
                HookDecision::Continue => continue,
                other => return Ok(other),
            }
        }
        Ok(HookDecision::Continue)
    }
}

// ─── Typed per-event registration helpers ───────────────────────────────
//
// One method per `HookEvent` variant. The macro keeps the surface
// uniform — adding a new event = one line in `hook_helpers!` + one
// variant in [`HookEvent`].

macro_rules! hook_helpers {
    ($( ($method:ident, $variant:ident, $doc:expr) ),* $(,)?) => {
        impl HookRegistry {
            $(
                #[doc = $doc]
                pub fn $method<F>(&self, f: F)
                where
                    F: for<'a> Fn(&'a mut HookContext) -> BoxFuture<'a, Result<HookDecision>>
                        + Send
                        + Sync
                        + 'static,
                {
                    self.register(HookEvent::$variant, f);
                }
            )*
        }
    };
}

hook_helpers! {
    (
        on_before_each_request,
        BeforeEachRequest,
        "Fires once per job, before any network activity. Use to mutate \
         `ctx.request_headers`, swap `ctx.proxy`, or short-circuit with \
         `Skip` (e.g. domain-level deny lists)."
    ),
    (
        on_after_dns_resolve,
        AfterDnsResolve,
        "Fires after the system / DoH resolver returns. `ctx.user_data` \
         carries the resolved address record set under `dns`."
    ),
    (
        on_after_tls_handshake,
        AfterTlsHandshake,
        "Fires after the TLS handshake completes. `ctx.user_data` \
         carries `alpn`, `tls_version`, `cipher`, `peer_cert_sha256`."
    ),
    (
        on_after_first_byte,
        AfterFirstByte,
        "Fires once headers have arrived. `ctx.response_status` and \
         `ctx.response_headers` are populated; the body has not been \
         buffered yet. Good place to short-circuit on 4xx/5xx."
    ),
    (
        on_response_body,
        OnResponseBody,
        "Fires after the body has been buffered (`ctx.body` populated). \
         For HTML targets `ctx.html_post_js` is None on the HTTP path \
         and Some on the render path."
    ),
    (
        on_after_load,
        AfterLoad,
        "Render path only — fires when the page's `load` event has \
         resolved. `ctx.html_post_js` is the post-JS DOM serialisation."
    ),
    (
        on_after_idle,
        AfterIdle,
        "Render path only — fires after the wait strategy resolves \
         (network idle / fixed dwell / selector). The DOM is final."
    ),
    (
        on_discovery,
        OnDiscovery,
        "Fires once link extraction has completed. `ctx.captured_urls` \
         carries the harvested URLs and is mutable — push extra URLs \
         to enqueue them, drain the vec to suppress discovery."
    ),
    (
        on_job_start,
        OnJobStart,
        "Fires once per job, immediately after dequeue. Useful for \
         per-job initialisation (e.g. session_id derivation)."
    ),
    (
        on_job_end,
        OnJobEnd,
        "Fires once per job, after every other event has resolved. \
         Always runs, even on early-skip / error paths — use for \
         metric counters and cleanup."
    ),
    (
        on_error,
        OnError,
        "Fires when a job has hit a terminal error. `ctx.error` is set; \
         returning `Retry` resubmits the job (if `ctx.allow_retry`)."
    ),
    (
        on_robots_decision,
        OnRobotsDecision,
        "Fires after `robots.txt` evaluation. `ctx.robots_allowed` is \
         set; you can override the decision by mutating it."
    ),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ctx() -> HookContext {
        HookContext::new(url::Url::parse("https://example.test").unwrap(), 0)
    }

    #[tokio::test]
    async fn typed_helper_registers_on_correct_event() {
        let reg = HookRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        reg.on_after_first_byte(move |_ctx| {
            let c = c.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(HookDecision::Continue)
            })
        });
        let mut cx = ctx();
        // Other events are silent — only AfterFirstByte fires.
        reg.fire(HookEvent::OnDiscovery, &mut cx).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        reg.fire(HookEvent::AfterFirstByte, &mut cx).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn first_non_continue_wins_subsequent_skipped() {
        let reg = HookRegistry::new();
        let later_called = Arc::new(AtomicUsize::new(0));
        reg.on_after_first_byte(|_| Box::pin(async { Ok(HookDecision::Skip) }));
        let l = later_called.clone();
        reg.on_after_first_byte(move |_| {
            let l = l.clone();
            Box::pin(async move {
                l.fetch_add(1, Ordering::SeqCst);
                Ok(HookDecision::Continue)
            })
        });
        let mut cx = ctx();
        let decision = reg.fire(HookEvent::AfterFirstByte, &mut cx).await.unwrap();
        assert_eq!(decision, HookDecision::Skip);
        assert_eq!(
            later_called.load(Ordering::SeqCst),
            0,
            "second hook must not run after a Skip"
        );
    }

    #[tokio::test]
    async fn hook_can_push_extra_captured_urls() {
        let reg = HookRegistry::new();
        reg.on_discovery(|ctx| {
            Box::pin(async move {
                ctx.captured_urls
                    .push(url::Url::parse("https://example.test/extra").unwrap());
                Ok(HookDecision::Continue)
            })
        });
        let mut cx = ctx();
        reg.fire(HookEvent::OnDiscovery, &mut cx).await.unwrap();
        assert_eq!(cx.captured_urls.len(), 1);
        assert_eq!(cx.captured_urls[0].path(), "/extra");
    }
}
