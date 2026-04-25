//! HTTP/2 connection pool.
//!
//! hyper's `SendRequest<B>` is cheaply cloneable and shares the underlying
//! multiplexed h2 connection: every clone can submit streams concurrently.
//! We cache one `SendRequest` per `(scheme, host, port)` key, drop it when
//! the backing task dies (detected via `is_ready` / `ready().await` errors),
//! and reopen on demand.
//!
//! Plain HTTP/1 lives outside this pool — pipelining is rarely worth the
//! complexity versus opening a fresh socket.

use bytes::Bytes;
use dashmap::DashMap;
use http::Request;
use http_body_util::Empty;
use hyper::client::conn::http2::SendRequest;
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ConnKey {
    pub scheme: &'static str,
    pub host: String,
    pub port: u16,
    /// Empty string when no proxy is in use. When set, the pooled
    /// connection was opened *via* this proxy — re-using it for a job
    /// that wants a different proxy (or none) would silently leak the
    /// upstream IP. Including the proxy in the key gives us a per-proxy
    /// sub-pool for free, so concurrent jobs sharing a proxy still hit
    /// the keep-alive cache.
    pub proxy_key: String,
}

impl ConnKey {
    /// Build a key from the request and an optional proxy. Pass `None`
    /// for `proxy` to indicate a direct connection.
    pub fn new(
        scheme: &'static str,
        host: impl Into<String>,
        port: u16,
        proxy: Option<&url::Url>,
    ) -> Self {
        Self {
            scheme,
            host: host.into(),
            port,
            proxy_key: proxy.map(|u| u.as_str().to_string()).unwrap_or_default(),
        }
    }
}

#[derive(Clone)]
pub struct PooledH2 {
    pub sender: SendRequest<Empty<Bytes>>,
}

/// HTTP/1.1 keep-alive state. Unlike h2, h1 only allows one in-flight request
/// per socket — we guard the sender with a Mutex so tasks serialize on the
/// same connection. If you want parallelism over h1 to the same host, open
/// multiple sockets (the current strategy: 1 conn per host, reused).
pub struct PooledH1 {
    pub sender: Arc<tokio::sync::Mutex<hyper::client::conn::http1::SendRequest<Empty<Bytes>>>>,
}

impl Clone for PooledH1 {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

#[derive(Clone, Default)]
pub struct ConnPool {
    /// Per-key slot so two tasks racing a miss don't both open a socket.
    slots: Arc<DashMap<ConnKey, Arc<Mutex<Option<PooledH2>>>>>,
    h1_slots: Arc<DashMap<ConnKey, Arc<Mutex<Option<PooledH1>>>>>,
}

impl ConnPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn slot(&self, key: ConnKey) -> Arc<Mutex<Option<PooledH2>>> {
        self.slots
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    }

    /// Returns a live sender if one exists AND the underlying connection is
    /// still healthy enough to accept at least one more stream.
    pub fn get_live(&self, key: &ConnKey) -> Option<PooledH2> {
        let slot = self.slots.get(key)?.clone();
        let guard = slot.lock();
        let p = guard.as_ref()?;
        if p.sender.is_closed() {
            return None;
        }
        Some(p.clone())
    }

    pub fn store(&self, key: ConnKey, p: PooledH2) {
        let slot = self.slot(key);
        *slot.lock() = Some(p);
    }

    pub fn invalidate(&self, key: &ConnKey) {
        if let Some(slot) = self.slots.get(key) {
            *slot.lock() = None;
        }
    }

    /// h1 analogue: fetch a live keep-alive connection for the key, if any.
    pub fn h1_get_live(&self, key: &ConnKey) -> Option<PooledH1> {
        let slot = self.h1_slots.get(key)?.clone();
        let guard = slot.lock();
        let p = guard.as_ref()?;
        if p.sender.try_lock().is_ok_and(|s| s.is_closed()) {
            return None;
        }
        Some(p.clone())
    }

    pub fn h1_store(&self, key: ConnKey, p: PooledH1) {
        let slot = self
            .h1_slots
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone();
        *slot.lock() = Some(p);
    }

    pub fn h1_invalidate(&self, key: &ConnKey) {
        if let Some(slot) = self.h1_slots.get(key) {
            *slot.lock() = None;
        }
    }
}

/// Convenience: build the base request pointed at the right scheme+authority
/// before handing it off to `SendRequest::send_request`. (Kept here so other
/// callers can reuse if they want.)
pub fn build_base_request(scheme: &str, authority: &str, path: &str) -> http::request::Builder {
    Request::builder()
        .method(http::Method::GET)
        .uri(format!("{scheme}://{authority}{path}"))
}
