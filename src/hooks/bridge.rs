//! JS hook bridge — IPC protocol for `--hook-bridge` callers.
//!
//! When `crawlex crawl --hook-bridge stdio` runs, every lifecycle event
//! the SDK subscribed to is serialised as a JSON envelope on stdout
//! (or a dedicated FD). The SDK responds with a `HookResult` JSON; the
//! crawler applies the patch fields back onto the live `HookContext`
//! and translates the decision into the existing `HookDecision` enum.
//!
//! Three concerns kept separate from the Rust-native registry:
//!
//! 1. **Wire format** — stable, versioned, only carries the subset of
//!    `HookContext` that's safely serialisable (no `Bytes`, no
//!    `HeaderMap` ordering — both arrive as JSON-friendly shapes).
//! 2. **Channel** — abstracted via [`BridgeChannel`] so the IPC fabric
//!    (stdio pipe, unix socket, TCP, in-memory buffer for tests) plugs
//!    in without touching call sites.
//! 3. **Adapter** — [`BridgeHookAdapter`] implements the existing
//!    `HookFn` shape so the bridge appears identical to a native Rust
//!    hook from the registry's POV.
//!
//! The wire envelope is intentionally narrower than `HookContext`:
//! request/response bodies and binary headers are skipped to keep the
//! IPC cheap. Hooks that need raw bytes should run in-process via the
//! native Rust API; the JS bridge is for decision logic on metadata.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::hooks::{HookContext, HookDecision, HookEvent};
use crate::{Error, Result};

/// Wire schema version. Bump when the field set changes in a way the
/// SDK can't ignore (renames, removed required fields). The JS dispatcher
/// echoes this back on every reply so a desync is caught fast.
pub const HOOK_BRIDGE_PROTOCOL_VERSION: u32 = 1;

/// JSON-friendly snapshot of [`HookContext`] sent to the SDK. The body /
/// headers are projected to plain strings + maps; nothing in here is
/// `chromiumoxide`-typed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireContext {
    pub url: String,
    pub depth: u32,
    pub response_status: Option<u16>,
    pub response_headers: Option<HashMap<String, String>>,
    pub html_present: bool,
    pub body_size: Option<usize>,
    pub captured_urls: Vec<String>,
    pub proxy: Option<String>,
    pub retry_count: u32,
    pub allow_retry: bool,
    pub robots_allowed: Option<bool>,
    pub user_data: HashMap<String, serde_json::Value>,
    pub error: Option<String>,
}

impl WireContext {
    /// Build a wire snapshot from a live `HookContext`. Body bytes are
    /// dropped (only the size is sent) so the IPC payload stays small —
    /// JS hooks that need the raw body can stash it via `user_data`.
    pub fn from_context(ctx: &HookContext) -> Self {
        Self {
            url: ctx.url.to_string(),
            depth: ctx.depth,
            response_status: ctx.response_status,
            response_headers: ctx.response_headers.as_ref().map(|h| {
                h.iter()
                    .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
                    .collect()
            }),
            html_present: ctx.html_post_js.is_some(),
            body_size: ctx.body.as_ref().map(|b| b.len()),
            captured_urls: ctx.captured_urls.iter().map(|u| u.to_string()).collect(),
            proxy: ctx.proxy.as_ref().map(|u| u.to_string()),
            retry_count: ctx.retry_count,
            allow_retry: ctx.allow_retry,
            robots_allowed: ctx.robots_allowed,
            user_data: ctx.user_data.clone(),
            error: ctx.error.clone(),
        }
    }
}

/// Stable string discriminator for `HookEvent`. Mirrors the rust enum
/// 1:1; a JS dispatcher that doesn't recognise an incoming kind should
/// reply `decision: "continue"` so the pipeline keeps moving.
pub fn event_wire_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::BeforeEachRequest => "before_each_request",
        HookEvent::AfterDnsResolve => "after_dns_resolve",
        HookEvent::AfterTlsHandshake => "after_tls_handshake",
        HookEvent::AfterFirstByte => "after_first_byte",
        HookEvent::OnResponseBody => "on_response_body",
        HookEvent::AfterLoad => "after_load",
        HookEvent::AfterIdle => "after_idle",
        HookEvent::OnDiscovery => "on_discovery",
        HookEvent::OnJobStart => "on_job_start",
        HookEvent::OnJobEnd => "on_job_end",
        HookEvent::OnError => "on_error",
        HookEvent::OnRobotsDecision => "on_robots_decision",
    }
}

/// Outbound message: rust → SDK.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BridgeOutbound {
    /// Initial handshake. Sent once per process start so the SDK can
    /// pin to a compatible protocol version.
    Hello { v: u32, protocol: String },
    /// Fire a hook on the SDK side. The SDK must reply with a matching
    /// `hook.result { id }` — delays / dropped replies block the
    /// crawl until the bridge timeout fires (default 5s per call).
    HookInvoke {
        id: u64,
        event: String,
        ctx: WireContext,
    },
}

/// JS-supplied decision tag. Mirrors `HookDecision` but as a wire string;
/// `Continue` is the default for forward-compat (an SDK that doesn't
/// know an incoming `event` returns Continue).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireDecision {
    Continue,
    Skip,
    Retry,
    Abort,
}

impl From<WireDecision> for HookDecision {
    fn from(d: WireDecision) -> Self {
        match d {
            WireDecision::Continue => HookDecision::Continue,
            WireDecision::Skip => HookDecision::Skip,
            WireDecision::Retry => HookDecision::Retry,
            WireDecision::Abort => HookDecision::Abort,
        }
    }
}

/// JS-supplied patch applied back onto the live `HookContext`. Every
/// field is `Option`al — `None` means "leave the existing value alone".
/// Captured URLs / user_data are *replacements*, not merges, so the SDK
/// sees the full set on each invocation and decides the final list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_urls: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_data: Option<HashMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robots_allowed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_retry: Option<bool>,
}

impl ContextPatch {
    pub fn apply(self, ctx: &mut HookContext) {
        if let Some(urls) = self.captured_urls {
            ctx.captured_urls = urls
                .into_iter()
                .filter_map(|u| url::Url::parse(&u).ok())
                .collect();
        }
        if let Some(ud) = self.user_data {
            ctx.user_data = ud;
        }
        if let Some(r) = self.robots_allowed {
            ctx.robots_allowed = Some(r);
        }
        if let Some(r) = self.allow_retry {
            ctx.allow_retry = r;
        }
    }
}

/// Inbound message: SDK → rust.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BridgeInbound {
    /// SDK acknowledged the hello and tells the crawler which events
    /// it intends to handle. Events not in `subscribed` skip the bridge
    /// and run only the rust-native registry.
    Subscribe { subscribed: Vec<String> },
    /// Reply to a `hook.invoke`.
    HookResult {
        id: u64,
        decision: WireDecision,
        #[serde(default)]
        patch: ContextPatch,
    },
}

/// Channel abstraction so the bridge can be driven by stdio, FD, unix
/// socket, or an in-memory buffer in tests. Implementors guarantee
/// line-delimited JSON ordering; the bridge dispatch loop reads one
/// envelope per `recv` call and writes one per `send`.
#[async_trait::async_trait]
pub trait BridgeChannel: Send + Sync {
    async fn send(&self, msg: &BridgeOutbound) -> Result<()>;
    async fn recv(&self) -> Result<BridgeInbound>;
}

/// Tracks pending hook invocations awaiting an SDK reply. The bridge
/// dispatch loop pops by `id` when a `hook.result` arrives and
/// completes the matching `oneshot`.
type Pending = HashMap<u64, oneshot::Sender<(WireDecision, ContextPatch)>>;

/// Adapter turning a [`BridgeChannel`] into a per-event [`HookFn`]. The
/// crawler's existing `HookRegistry::on(...)` is the only registration
/// surface — we just plug in a closure that funnels through the
/// channel.
pub struct BridgeHookAdapter {
    channel: Arc<dyn BridgeChannel>,
    pending: Arc<Mutex<Pending>>,
    next_id: AtomicU64,
    /// SDK declared subscriptions. Events not in this set short-circuit
    /// to `Continue` without an IPC round-trip.
    subscribed: parking_lot::RwLock<Vec<HookEvent>>,
    /// Per-call timeout. Defaults to 5s; configurable so tests can
    /// pin a tight bound.
    timeout: std::time::Duration,
}

impl BridgeHookAdapter {
    pub fn new(channel: Arc<dyn BridgeChannel>) -> Self {
        Self {
            channel,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            subscribed: parking_lot::RwLock::new(Vec::new()),
            timeout: std::time::Duration::from_secs(5),
        }
    }

    pub fn with_timeout(mut self, d: std::time::Duration) -> Self {
        self.timeout = d;
        self
    }

    /// Send the protocol hello. Idempotent — safe to call multiple
    /// times if a caller wants to re-handshake.
    pub async fn handshake(&self) -> Result<()> {
        self.channel
            .send(&BridgeOutbound::Hello {
                v: HOOK_BRIDGE_PROTOCOL_VERSION,
                protocol: "crawlex.hooks".into(),
            })
            .await
    }

    /// Drain one inbound message, dispatching subscription updates +
    /// results to their pending oneshots. Loop callers run this in a
    /// background task.
    pub async fn pump_once(&self) -> Result<()> {
        let msg = self.channel.recv().await?;
        match msg {
            BridgeInbound::Subscribe { subscribed } => {
                let parsed: Vec<HookEvent> = subscribed
                    .into_iter()
                    .filter_map(|s| event_from_wire(&s))
                    .collect();
                *self.subscribed.write() = parsed;
            }
            BridgeInbound::HookResult {
                id,
                decision,
                patch,
            } => {
                if let Some(tx) = self.pending.lock().remove(&id) {
                    let _ = tx.send((decision, patch));
                } else {
                    tracing::debug!(id, "hook bridge: no pending entry for id");
                }
            }
        }
        Ok(())
    }

    /// Issue one hook invocation, waiting for the SDK reply. Returns
    /// `Continue` when the SDK isn't subscribed to the event so the
    /// crawler doesn't pay an IPC RTT for events the SDK doesn't care
    /// about.
    pub async fn invoke(&self, event: HookEvent, ctx: &mut HookContext) -> Result<HookDecision> {
        let subscribed_to = self.subscribed.read().contains(&event);
        if !subscribed_to {
            return Ok(HookDecision::Continue);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        self.channel
            .send(&BridgeOutbound::HookInvoke {
                id,
                event: event_wire_name(event).to_string(),
                ctx: WireContext::from_context(ctx),
            })
            .await?;

        let resp = tokio::time::timeout(self.timeout, rx)
            .await
            .map_err(|_| {
                self.pending.lock().remove(&id);
                Error::Hook(format!(
                    "hook bridge timeout: event={} id={id} budget_ms={}",
                    event_wire_name(event),
                    self.timeout.as_millis()
                ))
            })?
            .map_err(|_| Error::Hook("hook bridge channel closed before reply".into()))?;

        let (decision, patch) = resp;
        patch.apply(ctx);
        Ok(decision.into())
    }
}

/// Convert a wire event name back to the typed enum. Returns `None`
/// for unknown names so the SDK can subscribe to events the rust side
/// hasn't shipped yet without a hard error.
pub fn event_from_wire(s: &str) -> Option<HookEvent> {
    Some(match s {
        "before_each_request" => HookEvent::BeforeEachRequest,
        "after_dns_resolve" => HookEvent::AfterDnsResolve,
        "after_tls_handshake" => HookEvent::AfterTlsHandshake,
        "after_first_byte" => HookEvent::AfterFirstByte,
        "on_response_body" => HookEvent::OnResponseBody,
        "after_load" => HookEvent::AfterLoad,
        "after_idle" => HookEvent::AfterIdle,
        "on_discovery" => HookEvent::OnDiscovery,
        "on_job_start" => HookEvent::OnJobStart,
        "on_job_end" => HookEvent::OnJobEnd,
        "on_error" => HookEvent::OnError,
        "on_robots_decision" => HookEvent::OnRobotsDecision,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use tokio::sync::Mutex as TokioMutex;

    /// In-memory channel — a pair of mpsc queues. Two ends:
    /// `Channel` is what the rust adapter writes to / reads from;
    /// the test acts as the SDK and reads outbounds, writes inbounds.
    struct ChannelPair {
        outbound_tx: mpsc::UnboundedSender<BridgeOutbound>,
        inbound_rx: TokioMutex<mpsc::UnboundedReceiver<BridgeInbound>>,
    }

    #[async_trait::async_trait]
    impl BridgeChannel for ChannelPair {
        async fn send(&self, msg: &BridgeOutbound) -> Result<()> {
            self.outbound_tx
                .send(msg.clone())
                .map_err(|e| Error::Hook(format!("test send: {e}")))
        }
        async fn recv(&self) -> Result<BridgeInbound> {
            self.inbound_rx
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| Error::Hook("test channel closed".into()))
        }
    }

    fn ctx() -> HookContext {
        HookContext::new(url::Url::parse("https://example.test/p").unwrap(), 0)
    }

    #[tokio::test]
    async fn invoke_short_circuits_unsubscribed_events() {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (_in_tx, in_rx) = mpsc::unbounded_channel();
        let channel = Arc::new(ChannelPair {
            outbound_tx: out_tx,
            inbound_rx: TokioMutex::new(in_rx),
        });
        let adapter = BridgeHookAdapter::new(channel);
        let mut cx = ctx();
        // No subscription yet — invoke must short-circuit to Continue
        // and emit no IPC traffic.
        let d = adapter
            .invoke(HookEvent::AfterFirstByte, &mut cx)
            .await
            .unwrap();
        assert_eq!(d, HookDecision::Continue);
        assert!(out_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn invoke_round_trip_applies_patch_and_decision() {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let channel = Arc::new(ChannelPair {
            outbound_tx: out_tx,
            inbound_rx: TokioMutex::new(in_rx),
        });
        let adapter = Arc::new(BridgeHookAdapter::new(channel));
        // Subscribe + spawn a pump task in the background.
        in_tx
            .send(BridgeInbound::Subscribe {
                subscribed: vec!["on_discovery".into()],
            })
            .unwrap();
        let pump = adapter.clone();
        tokio::spawn(async move {
            for _ in 0..10 {
                if pump.pump_once().await.is_err() {
                    break;
                }
            }
        });
        // Invoke OnDiscovery. The "SDK" side reads the outbound and
        // crafts a reply that swaps captured_urls + flips robots.
        let invoke_adapter = adapter.clone();
        let invoke_task = tokio::spawn(async move {
            let mut cx = ctx();
            cx.captured_urls
                .push(url::Url::parse("https://example.test/keep").unwrap());
            let d = invoke_adapter
                .invoke(HookEvent::OnDiscovery, &mut cx)
                .await
                .unwrap();
            (d, cx)
        });

        // Wait until the rust side issued the invoke envelope.
        let outbound = loop {
            if let Ok(msg) = out_rx.try_recv() {
                break msg;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        let id = match outbound {
            BridgeOutbound::HookInvoke { id, .. } => id,
            _ => panic!("expected hook.invoke first"),
        };
        in_tx
            .send(BridgeInbound::HookResult {
                id,
                decision: WireDecision::Skip,
                patch: ContextPatch {
                    captured_urls: Some(vec!["https://example.test/swap".into()]),
                    robots_allowed: Some(false),
                    ..Default::default()
                },
            })
            .unwrap();

        let (decision, cx) = invoke_task.await.unwrap();
        assert_eq!(decision, HookDecision::Skip);
        assert_eq!(cx.captured_urls.len(), 1);
        assert_eq!(cx.captured_urls[0].path(), "/swap");
        assert_eq!(cx.robots_allowed, Some(false));
    }
}
