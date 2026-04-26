//! `StateStorage` — browser-session state + archival.
//!
//! Pairs the live `save_state` / `load_state` round-trip used by the
//! pool to resume a session across runs with `archive_session`, the
//! eviction sink that `SessionRegistry` calls when it drops a
//! BrowserContext.
//!
//! All methods default to no-op so backends that don't care about
//! stateful crawls (the in-memory backend) compile clean.

use crate::Result;

/// Browser-session state persistence + archival sink.
#[async_trait::async_trait]
pub trait StateStorage: Send + Sync {
    /// Persist a session's opaque state JSON (cookies + storage +
    /// service worker registrations) keyed by `session_id`. Default
    /// no-op so memory-only backends compile clean.
    async fn save_state(&self, _session_id: &str, _state_json: &str) -> Result<()> {
        Ok(())
    }

    /// Load a previously saved state JSON, or `None` when unknown.
    async fn load_state(&self, _session_id: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Persist an archived session entry on eviction. Default no-op.
    async fn archive_session(
        &self,
        _entry: &crate::identity::SessionEntry,
        _reason: crate::identity::EvictionReason,
    ) -> Result<()> {
        Ok(())
    }
}
