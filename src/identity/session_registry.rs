//! Central session lifecycle registry — Phase 6.
//!
//! The registry is the single source of truth for "which render-session
//! ids exist, what state are they in, when were they last used." It was
//! introduced in Fase 6 (Session Isolation) to give the crawler a place
//! to:
//!
//! * attach a TTL + cleanup story to BrowserContexts,
//! * record `SessionState` transitions (Clean/Warm/Contaminated/Blocked)
//!   from a single hook instead of dispersed ad-hoc updates,
//! * answer `sessions list --state <filter>` queries for operators.
//!
//! The registry does **not** own the browser contexts themselves — it
//! merely tracks ids + metadata. `RenderPool::drop_session` is the
//! component that actually tears down cookies, Pages and BrowserContexts.
//!
//! # Shape
//!
//! `SessionEntry` stores the scope, its derived scope_key (e.g.
//! `"example.com"` for `RegistrableDomain`), the current `SessionState`,
//! how many urls have been routed through it and which proxies it saw.
//!
//! The registry is `Send + Sync` via `DashMap`, which is the pattern the
//! pool and router already use elsewhere.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::antibot::SessionState;
use crate::config::RenderSessionScope;

/// Default TTL applied when the config field is unset — 1 hour of
/// inactivity is long enough to survive a paused operator and short
/// enough that an abandoned crawl stops holding BrowserContexts.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 3600;

/// Per-entry metadata. `created_at` / `last_used` are `Instant`s for
/// cheap TTL arithmetic on the hot path; the `created_unix` /
/// `last_used_unix` pair is also stored so archive rows land with wall-
/// clock timestamps.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub scope: RenderSessionScope,
    pub scope_key: String,
    pub bundle_id: Option<u64>,
    pub state: SessionState,
    pub created_at: Instant,
    pub last_used: Instant,
    pub created_unix: i64,
    pub last_used_unix: i64,
    pub ttl_override: Option<Duration>,
    pub urls_visited: u32,
    pub challenges_seen: u32,
    pub proxy_history: Vec<Url>,
}

impl SessionEntry {
    /// Effective TTL: override if set, else the registry-wide default.
    pub fn effective_ttl(&self, default: Duration) -> Duration {
        self.ttl_override.unwrap_or(default)
    }

    /// Snapshot suited for JSON emission (CLI `sessions list`, events).
    pub fn as_snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            id: self.id.clone(),
            scope: self.scope,
            scope_key: self.scope_key.clone(),
            bundle_id: self.bundle_id,
            state: self.state,
            created_unix: self.created_unix,
            last_used_unix: self.last_used_unix,
            urls_visited: self.urls_visited,
            challenges_seen: self.challenges_seen,
            proxy_history: self.proxy_history.iter().map(|u| u.to_string()).collect(),
        }
    }
}

/// Serialisable view of a `SessionEntry` — omits `Instant` which isn't
/// serde-friendly and keeps the proxy history as strings for humans.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub scope: RenderSessionScope,
    pub scope_key: String,
    pub bundle_id: Option<u64>,
    pub state: SessionState,
    pub created_unix: i64,
    pub last_used_unix: i64,
    pub urls_visited: u32,
    pub challenges_seen: u32,
    pub proxy_history: Vec<String>,
}

/// Central registry. `Arc<SessionRegistry>` is shared by `Crawler`,
/// `RenderPool`, and the cleanup task.
pub struct SessionRegistry {
    entries: DashMap<String, SessionEntry>,
    default_ttl: Duration,
}

impl SessionRegistry {
    pub fn new(ttl_secs: u64) -> Self {
        let ttl = ttl_secs.max(1);
        Self {
            entries: DashMap::new(),
            default_ttl: Duration::from_secs(ttl),
        }
    }

    pub fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    /// Derive the scope key for a url under the requested scope — used
    /// both to compute session ids (render pool) and to surface a
    /// human-readable handle in `list`.
    pub fn scope_key_for(scope: RenderSessionScope, url: &Url) -> String {
        let host = url.host_str().unwrap_or_default();
        match scope {
            RenderSessionScope::RegistrableDomain => {
                crate::discovery::subdomains::registrable_domain(host)
                    .unwrap_or_else(|| host.to_string())
            }
            RenderSessionScope::Host => {
                if let Some(port) = url.port() {
                    format!("{host}:{port}")
                } else {
                    host.to_string()
                }
            }
            RenderSessionScope::Origin => url.origin().ascii_serialization(),
            RenderSessionScope::Url => url.as_str().to_string(),
        }
    }

    /// Return the existing entry for `id` or create a fresh `Clean`
    /// entry. Idempotent — repeat calls with the same id just `touch`.
    pub fn get_or_create(&self, id: &str, scope: RenderSessionScope, url: &Url) -> SessionEntry {
        let now = Instant::now();
        let now_unix = now_unix();
        let scope_key = Self::scope_key_for(scope, url);
        let entry = self
            .entries
            .entry(id.to_string())
            .and_modify(|e| {
                e.last_used = now;
                e.last_used_unix = now_unix;
                e.urls_visited = e.urls_visited.saturating_add(1);
            })
            .or_insert_with(|| SessionEntry {
                id: id.to_string(),
                scope,
                scope_key: scope_key.clone(),
                bundle_id: None,
                state: SessionState::Clean,
                created_at: now,
                last_used: now,
                created_unix: now_unix,
                last_used_unix: now_unix,
                ttl_override: None,
                urls_visited: 1,
                challenges_seen: 0,
                proxy_history: Vec::new(),
            })
            .clone();
        entry
    }

    /// Update only `last_used`. No-op if `id` not registered.
    pub fn touch(&self, id: &str) {
        if let Some(mut e) = self.entries.get_mut(id) {
            let now = Instant::now();
            e.last_used = now;
            e.last_used_unix = now_unix();
        }
    }

    /// Set the state, returning `(from, to)` when the id is known. When
    /// the id is unknown the call is a no-op — callers always create
    /// via `get_or_create` before marking.
    pub fn mark(&self, id: &str, state: SessionState) -> Option<(SessionState, SessionState)> {
        let mut e = self.entries.get_mut(id)?;
        let from = e.state;
        if from == state {
            return None;
        }
        e.state = state;
        Some((from, state))
    }

    pub fn bump_challenge(&self, id: &str) {
        if let Some(mut e) = self.entries.get_mut(id) {
            e.challenges_seen = e.challenges_seen.saturating_add(1);
        }
    }

    pub fn set_bundle(&self, id: &str, bundle_id: u64) {
        if let Some(mut e) = self.entries.get_mut(id) {
            e.bundle_id = Some(bundle_id);
        }
    }

    pub fn record_proxy(&self, id: &str, proxy: &Url) {
        if let Some(mut e) = self.entries.get_mut(id) {
            if !e.proxy_history.iter().any(|p| p == proxy) {
                e.proxy_history.push(proxy.clone());
            }
        }
    }

    pub fn set_ttl_override(&self, id: &str, ttl: Option<Duration>) {
        if let Some(mut e) = self.entries.get_mut(id) {
            e.ttl_override = ttl;
        }
    }

    /// Ids whose `last_used + ttl` is in the past.
    pub fn expired(&self) -> Vec<String> {
        let now = Instant::now();
        self.entries
            .iter()
            .filter_map(|r| {
                let ttl = r.effective_ttl(self.default_ttl);
                if now.duration_since(r.last_used) >= ttl {
                    Some(r.id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Remove and return the entry for `id`, or `None` if unknown.
    pub fn evict(&self, id: &str) -> Option<SessionEntry> {
        self.entries.remove(id).map(|(_, v)| v)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    pub fn get(&self, id: &str) -> Option<SessionEntry> {
        self.entries.get(id).map(|e| e.clone())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Snapshot every entry, optionally filtered by state. Callers
    /// that want to page should do so client-side — the expected cardi-
    /// nality is low (a handful per run, not millions).
    pub fn list(&self, filter: Option<SessionState>) -> Vec<SessionSnapshot> {
        self.entries
            .iter()
            .filter(|e| filter.map(|f| f == e.state).unwrap_or(true))
            .map(|e| e.as_snapshot())
            .collect()
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Reason carried on `SessionEvicted` events and archive rows. Stable
/// wire strings so downstream consumers can filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionReason {
    /// TTL cleanup task expired the entry.
    Ttl,
    /// Policy decided the session was Blocked and `drop_session_on_block`
    /// is enabled.
    Blocked,
    /// Operator-triggered via CLI or programmatic call.
    Manual,
    /// Run shutdown: flushes every live entry to the archive.
    RunEnded,
}

impl EvictionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ttl => "ttl",
            Self::Blocked => "blocked",
            Self::Manual => "manual",
            Self::RunEnded => "run_ended",
        }
    }
}

/// Extension hook used by the cleanup task to reach the pool. Kept as a
/// trait so `tests/session_registry.rs` can exercise expiry without
/// dragging in a real RenderPool.
#[async_trait::async_trait]
pub trait SessionDropTarget: Send + Sync {
    async fn drop_session(&self, id: &str);
}

/// Periodic task: every `tick` seconds, pull expired ids, ask the
/// drop-target to release BrowserContexts, and archive via the optional
/// sink. Returns a `JoinHandle` so callers can abort on shutdown.
pub fn spawn_cleanup_task(
    registry: Arc<SessionRegistry>,
    drop_target: Arc<dyn SessionDropTarget>,
    archive: Option<Arc<dyn SessionArchive>>,
    tick: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tick).await;
            let expired = registry.expired();
            for id in expired {
                drop_target.drop_session(&id).await;
                if let Some(entry) = registry.evict(&id) {
                    if let Some(sink) = archive.as_ref() {
                        let _ = sink.archive_session(&entry, EvictionReason::Ttl).await;
                    }
                }
            }
        }
    })
}

/// Optional archival sink. `SqliteStorage` implements it; other back-
/// ends return `Ok(())` by default.
#[async_trait::async_trait]
pub trait SessionArchive: Send + Sync {
    async fn archive_session(
        &self,
        entry: &SessionEntry,
        reason: EvictionReason,
    ) -> crate::Result<()>;
}

/// Thin adapter over `Arc<dyn Storage>` so the cleanup task can call
/// `archive_session` without pulling the `Storage` trait into every
/// dependent module. Storage's trait method returns `Ok(())` for
/// backends that don't implement archival.
pub struct StorageArchive(pub std::sync::Arc<dyn crate::storage::Storage>);

#[async_trait::async_trait]
impl SessionArchive for StorageArchive {
    async fn archive_session(
        &self,
        entry: &SessionEntry,
        reason: EvictionReason,
    ) -> crate::Result<()> {
        self.0.archive_session(entry, reason).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn get_or_create_is_idempotent() {
        let reg = SessionRegistry::new(60);
        let u = url("https://example.com/a");
        let a = reg.get_or_create("s1", RenderSessionScope::RegistrableDomain, &u);
        let b = reg.get_or_create("s1", RenderSessionScope::RegistrableDomain, &u);
        assert_eq!(a.id, b.id);
        assert_eq!(b.urls_visited, 2);
    }

    #[test]
    fn mark_transitions_state() {
        let reg = SessionRegistry::new(60);
        let _ = reg.get_or_create(
            "s2",
            RenderSessionScope::RegistrableDomain,
            &url("https://x.test/"),
        );
        let change = reg.mark("s2", SessionState::Contaminated);
        assert_eq!(
            change,
            Some((SessionState::Clean, SessionState::Contaminated))
        );
        // idempotent on same state
        assert_eq!(reg.mark("s2", SessionState::Contaminated), None);
    }

    #[test]
    fn expired_detects_ttl() {
        let reg = SessionRegistry::new(1);
        let _ = reg.get_or_create("s3", RenderSessionScope::Url, &url("https://y.test/"));
        // Force-expire by overriding the ttl to zero.
        reg.set_ttl_override("s3", Some(Duration::from_millis(0)));
        std::thread::sleep(std::time::Duration::from_millis(5));
        let expired = reg.expired();
        assert!(expired.iter().any(|id| id == "s3"));
    }

    #[test]
    fn scope_key_for_picks_right_granularity() {
        let u = url("https://www.example.com:8443/path?q=1");
        let dom = SessionRegistry::scope_key_for(RenderSessionScope::RegistrableDomain, &u);
        assert!(dom.ends_with("example.com"));
        let host = SessionRegistry::scope_key_for(RenderSessionScope::Host, &u);
        assert_eq!(host, "www.example.com:8443");
        let origin = SessionRegistry::scope_key_for(RenderSessionScope::Origin, &u);
        assert!(origin.starts_with("https://www.example.com"));
        let full = SessionRegistry::scope_key_for(RenderSessionScope::Url, &u);
        assert_eq!(full, u.as_str());
    }

    #[test]
    fn list_filters_by_state() {
        let reg = SessionRegistry::new(60);
        let _ = reg.get_or_create("a", RenderSessionScope::Url, &url("https://a.test/"));
        let _ = reg.get_or_create("b", RenderSessionScope::Url, &url("https://b.test/"));
        reg.mark("b", SessionState::Blocked);
        let blocked = reg.list(Some(SessionState::Blocked));
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].id, "b");
        let all = reg.list(None);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn drop_removes_entry() {
        let reg = SessionRegistry::new(60);
        let _ = reg.get_or_create("k", RenderSessionScope::Url, &url("https://k.test/"));
        assert!(reg.contains("k"));
        let removed = reg.evict("k");
        assert!(removed.is_some());
        assert!(!reg.contains("k"));
    }
}
