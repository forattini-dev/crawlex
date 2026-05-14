//! Session registry + routing for the v2 scraping framework.
//!
//! A [`SessionManager`] keeps a `session_id -> SessionEntry` map. Each
//! entry pins a [`BackendKind`] (HTTP, render, stealth) and owns a
//! private [`CookieJar`]. Two sessions of the same backend kind share
//! no cookie state — that's the isolation guarantee tested here.
//!
//! Routing rules (slice 16 scope):
//! * `Request.session_id = Some(id)` known => use that entry's backend
//!   and jar.
//! * `Request.session_id = Some(id)` unknown => `warn!` + fall back to
//!   the default backend (no jar attached, since unknown ids are not
//!   silently created — that would defeat the warning).
//! * `Request.session_id = None` => default backend, no jar.
//!
//! The default backend is configured at construction
//! (`SessionManager::new(default_backend)`). Later slices will replace
//! the placeholder `CookieJar` with the real per-backend session state.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::warn;

use super::request::Request;

/// Which engine backend a session is bound to. Cookies, identity
/// bundles and (eventually) browser contexts are partitioned by both
/// session id *and* backend kind — two sessions on different backends
/// never share state, two sessions on the same backend still don't.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    /// Plain reqwest / hyper HTTP path.
    Http,
    /// CDP render path (headless browser).
    Render,
    /// Stealth/impersonate path — TLS + JA3 + UA spoofing.
    Stealth,
}

/// Minimal in-memory cookie jar. Slice 16 only needs name->value
/// isolation between sessions; richer attributes live in
/// `impersonate::cookies` and `http::cookies` and will be wired in once
/// the dispatcher lands.
#[derive(Debug, Default, Clone)]
pub struct CookieJar {
    inner: Arc<Mutex<HashMap<String, String>>>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, name: impl Into<String>, value: impl Into<String>) {
        self.inner.lock().insert(name.into(), value.into());
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.inner.lock().get(name).cloned()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }
}

/// One row in the registry. Cloning is cheap — the jar is `Arc`-shared
/// so a clone observes future writes from any holder.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub backend: BackendKind,
    pub jar: CookieJar,
}

/// Per-id session registry. `Send + Sync` via a `Mutex<HashMap>`.
/// Cardinality is expected to stay low (a handful per run), so the lock
/// is fine — the per-session jar uses its own lock so hot-path cookie
/// writes don't contend with `register`/`resolve`.
pub struct SessionManager {
    entries: Mutex<HashMap<String, SessionEntry>>,
    default_backend: BackendKind,
}

/// Outcome of routing a [`Request`].
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// Backend the request should run on.
    pub backend: BackendKind,
    /// Cookie jar attached to the resolved session, if any. `None` for
    /// the default-backend fallback (unknown id or no id supplied).
    pub jar: Option<CookieJar>,
    /// Whether the session id was supplied but unknown. Surfaced for
    /// tests and to let callers double-check the warning path.
    pub fallback: bool,
}

impl SessionManager {
    pub fn new(default_backend: BackendKind) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            default_backend,
        }
    }

    pub fn default_backend(&self) -> BackendKind {
        self.default_backend
    }

    /// Register a fresh isolated session. Returns the entry. If `id`
    /// already exists the existing entry is returned unchanged — registration
    /// is idempotent so recipe re-runs don't clobber live state.
    pub fn register(&self, id: impl Into<String>, backend: BackendKind) -> SessionEntry {
        let id = id.into();
        let mut map = self.entries.lock();
        map.entry(id.clone())
            .or_insert_with(|| SessionEntry {
                id: id.clone(),
                backend,
                jar: CookieJar::new(),
            })
            .clone()
    }

    pub fn get(&self, id: &str) -> Option<SessionEntry> {
        self.entries.lock().get(id).cloned()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.lock().contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Resolve a request to the backend + jar it should run against.
    /// Unknown session ids fall back to the default backend with a
    /// `warn!` log. `None` session id silently uses the default.
    pub fn route(&self, req: &Request) -> RouteDecision {
        match req.session_id.as_deref() {
            None => RouteDecision {
                backend: self.default_backend,
                jar: None,
                fallback: false,
            },
            Some(id) => {
                if let Some(entry) = self.get(id) {
                    RouteDecision {
                        backend: entry.backend,
                        jar: Some(entry.jar),
                        fallback: false,
                    }
                } else {
                    warn!(
                        target: "scraping::session",
                        session_id = %id,
                        default_backend = ?self.default_backend,
                        "unknown session_id — falling back to default backend"
                    );
                    RouteDecision {
                        backend: self.default_backend,
                        jar: None,
                        fallback: true,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_creates_isolated_entry() {
        let mgr = SessionManager::new(BackendKind::Http);
        let a = mgr.register("a", BackendKind::Stealth);
        assert_eq!(a.id, "a");
        assert_eq!(a.backend, BackendKind::Stealth);
        assert!(mgr.contains("a"));
    }

    #[test]
    fn register_is_idempotent() {
        let mgr = SessionManager::new(BackendKind::Http);
        let a = mgr.register("dup", BackendKind::Render);
        a.jar.set("k", "v1");
        let b = mgr.register("dup", BackendKind::Stealth); // backend ignored
        // Existing entry preserved, including backend and jar contents.
        assert_eq!(b.backend, BackendKind::Render);
        assert_eq!(b.jar.get("k").as_deref(), Some("v1"));
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn cookies_isolated_between_same_backend_sessions() {
        let mgr = SessionManager::new(BackendKind::Http);
        let a = mgr.register("sess-a", BackendKind::Stealth);
        let b = mgr.register("sess-b", BackendKind::Stealth);
        a.jar.set("session", "AAA");
        b.jar.set("session", "BBB");
        assert_eq!(a.jar.get("session").as_deref(), Some("AAA"));
        assert_eq!(b.jar.get("session").as_deref(), Some("BBB"));
        // Independent maps — no leak.
        assert_eq!(a.jar.len(), 1);
        assert_eq!(b.jar.len(), 1);
    }

    #[test]
    fn jar_clone_shares_state_within_same_session() {
        // Routing returns a cloned `CookieJar`; mutations must still be
        // visible to the canonical entry, otherwise the dispatcher's
        // writes vanish after the request finishes.
        let mgr = SessionManager::new(BackendKind::Http);
        let entry = mgr.register("s", BackendKind::Http);
        let req = Request::new("https://x.test/").with_session("s");
        let route = mgr.route(&req);
        route.jar.as_ref().unwrap().set("k", "v");
        assert_eq!(entry.jar.get("k").as_deref(), Some("v"));
    }

    #[test]
    fn route_default_when_no_session_id() {
        let mgr = SessionManager::new(BackendKind::Render);
        let req = Request::new("https://x.test/");
        let route = mgr.route(&req);
        assert_eq!(route.backend, BackendKind::Render);
        assert!(route.jar.is_none());
        assert!(!route.fallback);
    }

    #[test]
    fn route_known_session_uses_its_backend() {
        let mgr = SessionManager::new(BackendKind::Http);
        mgr.register("s1", BackendKind::Stealth);
        let req = Request::new("https://x.test/").with_session("s1");
        let route = mgr.route(&req);
        assert_eq!(route.backend, BackendKind::Stealth);
        assert!(route.jar.is_some());
        assert!(!route.fallback);
    }

    #[test]
    fn route_unknown_session_falls_back_to_default() {
        let mgr = SessionManager::new(BackendKind::Http);
        let req = Request::new("https://x.test/").with_session("ghost");
        let route = mgr.route(&req);
        assert_eq!(route.backend, BackendKind::Http);
        assert!(route.jar.is_none());
        assert!(route.fallback);
        // Unknown id must NOT have been silently registered.
        assert!(!mgr.contains("ghost"));
    }
}
