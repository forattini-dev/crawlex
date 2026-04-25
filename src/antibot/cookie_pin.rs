//! Cookie pinning for antibot challenge replay.
//!
//! Vendors like Akamai, DataDome and PerimeterX issue long-lived
//! bot-score cookies (`_abck`, `datadome`, `_px*`) after a successful
//! challenge solve. Reusing those cookies on subsequent requests within
//! the issued TTL frequently lets us skip re-challenge entirely.
//!
//! # Ethics
//!
//! Pinning is **only** valid for cookies harvested from the crawler's
//! own live sessions. Importing cookies from a user's real browser,
//! another operator's jar, or any third party is explicitly out of
//! scope for this module and must not be supported by callers.
//!
//! # Design
//!
//! - Trait-based: [`CookiePinStore`] lets tests swap in the
//!   [`InMemoryCookiePinStore`] while production uses
//!   [`SqliteCookiePinStore`] (table `antibot_cookie_cache`).
//! - Pinning is keyed on `(vendor, origin, cookie_name)`. An origin
//!   mirrors the format emitted by [`crate::antibot::origin_of`].
//! - TTL is stored as `(pinned_at, ttl_secs)` and checked on read.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Conservative defaults derived from vendor docs / observed cookie
/// lifetimes. Callers may override via `pin`'s `ttl_secs` argument.
pub const AKAMAI_ABCK_TTL_SECS: u64 = 24 * 60 * 60;
pub const DATADOME_TTL_SECS: u64 = 6 * 60 * 60;
pub const PERIMETERX_TTL_SECS: u64 = 24 * 60 * 60;

/// A pinned cookie entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedCookie {
    pub vendor: String,
    pub origin: String,
    pub name: String,
    pub value: String,
    pub pinned_at: u64,
    pub ttl_secs: u64,
}

impl PinnedCookie {
    /// `true` when `now` is past `pinned_at + ttl_secs`.
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.pinned_at.saturating_add(self.ttl_secs)
    }
}

/// Minimal pinning-store contract. Implementations must be thread-safe.
pub trait CookiePinStore: Send + Sync {
    fn pin(
        &self,
        vendor: &str,
        origin: &str,
        name: &str,
        value: &str,
        ttl_secs: u64,
    ) -> Result<(), String>;

    fn get_pinned(
        &self,
        vendor: &str,
        origin: &str,
        name: &str,
    ) -> Result<Option<PinnedCookie>, String>;

    /// Drop every expired entry. Stores may implement this lazily.
    fn prune_expired(&self) -> Result<usize, String>;
}

/// In-memory backend. Used in tests and as a drop-in for builds where
/// SQLite is not available.
type PinKey = (String, String, String);

#[derive(Default)]
pub struct InMemoryCookiePinStore {
    inner: Arc<Mutex<HashMap<PinKey, PinnedCookie>>>,
}

impl InMemoryCookiePinStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CookiePinStore for InMemoryCookiePinStore {
    fn pin(
        &self,
        vendor: &str,
        origin: &str,
        name: &str,
        value: &str,
        ttl_secs: u64,
    ) -> Result<(), String> {
        let pinned = PinnedCookie {
            vendor: vendor.to_string(),
            origin: origin.to_string(),
            name: name.to_string(),
            value: value.to_string(),
            pinned_at: now_secs(),
            ttl_secs,
        };
        let key = (
            pinned.vendor.clone(),
            pinned.origin.clone(),
            pinned.name.clone(),
        );
        self.inner.lock().insert(key, pinned);
        Ok(())
    }

    fn get_pinned(
        &self,
        vendor: &str,
        origin: &str,
        name: &str,
    ) -> Result<Option<PinnedCookie>, String> {
        let key = (vendor.to_string(), origin.to_string(), name.to_string());
        let now = now_secs();
        let mut guard = self.inner.lock();
        match guard.get(&key).cloned() {
            Some(entry) if entry.is_expired(now) => {
                guard.remove(&key);
                Ok(None)
            }
            other => Ok(other),
        }
    }

    fn prune_expired(&self) -> Result<usize, String> {
        let now = now_secs();
        let mut guard = self.inner.lock();
        let before = guard.len();
        guard.retain(|_, v| !v.is_expired(now));
        Ok(before - guard.len())
    }
}

/// SQLite-backed pin store. Uses its own rusqlite Connection guarded by
/// a Mutex — the `antibot_cookie_cache` table is also created by the
/// main `SqliteStorage` init so either side can own the schema.
#[cfg(feature = "sqlite")]
pub struct SqliteCookiePinStore {
    conn: Mutex<rusqlite::Connection>,
}

#[cfg(feature = "sqlite")]
impl SqliteCookiePinStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let conn = rusqlite::Connection::open(path.as_ref()).map_err(|e| format!("open: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("schema: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn =
            rusqlite::Connection::open_in_memory().map_err(|e| format!("open_in_memory: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("schema: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

#[cfg(feature = "sqlite")]
impl CookiePinStore for SqliteCookiePinStore {
    fn pin(
        &self,
        vendor: &str,
        origin: &str,
        name: &str,
        value: &str,
        ttl_secs: u64,
    ) -> Result<(), String> {
        let pinned_at = now_secs() as i64;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO antibot_cookie_cache (vendor, origin, cookie_name, value, pinned_at, ttl_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(vendor, origin, cookie_name) DO UPDATE SET
                value=excluded.value,
                pinned_at=excluded.pinned_at,
                ttl_secs=excluded.ttl_secs",
            rusqlite::params![vendor, origin, name, value, pinned_at, ttl_secs as i64],
        )
        .map_err(|e| format!("pin: {e}"))?;
        Ok(())
    }

    fn get_pinned(
        &self,
        vendor: &str,
        origin: &str,
        name: &str,
    ) -> Result<Option<PinnedCookie>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT value, pinned_at, ttl_secs FROM antibot_cookie_cache
                 WHERE vendor=?1 AND origin=?2 AND cookie_name=?3",
            )
            .map_err(|e| format!("prepare: {e}"))?;
        let row = stmt
            .query_row(rusqlite::params![vendor, origin, name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => String::new(),
                other => format!("query: {other}"),
            });
        let (value, pinned_at, ttl_secs) = match row {
            Ok(v) => v,
            Err(e) if e.is_empty() => return Ok(None),
            Err(e) => return Err(e),
        };
        let pinned = PinnedCookie {
            vendor: vendor.into(),
            origin: origin.into(),
            name: name.into(),
            value,
            pinned_at: pinned_at.max(0) as u64,
            ttl_secs: ttl_secs.max(0) as u64,
        };
        if pinned.is_expired(now_secs()) {
            drop(stmt);
            let _ = conn.execute(
                "DELETE FROM antibot_cookie_cache WHERE vendor=?1 AND origin=?2 AND cookie_name=?3",
                rusqlite::params![vendor, origin, name],
            );
            return Ok(None);
        }
        Ok(Some(pinned))
    }

    fn prune_expired(&self) -> Result<usize, String> {
        let now = now_secs() as i64;
        let conn = self.conn.lock();
        let n = conn
            .execute(
                "DELETE FROM antibot_cookie_cache WHERE pinned_at + ttl_secs <= ?1",
                rusqlite::params![now],
            )
            .map_err(|e| format!("prune: {e}"))?;
        Ok(n)
    }
}

#[cfg(feature = "sqlite")]
pub(crate) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS antibot_cookie_cache (
        vendor      TEXT NOT NULL,
        origin      TEXT NOT NULL,
        cookie_name TEXT NOT NULL,
        value       TEXT NOT NULL,
        pinned_at   INTEGER NOT NULL,
        ttl_secs    INTEGER NOT NULL,
        PRIMARY KEY (vendor, origin, cookie_name)
    );
    CREATE INDEX IF NOT EXISTS idx_antibot_cookie_cache_origin
        ON antibot_cookie_cache(origin);
"#;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_pin_and_get_roundtrip() {
        let store = InMemoryCookiePinStore::new();
        store
            .pin("akamai", "https://a.test", "_abck", "xyz", 60)
            .unwrap();
        let got = store
            .get_pinned("akamai", "https://a.test", "_abck")
            .unwrap();
        assert!(got.is_some());
        let pinned = got.unwrap();
        assert_eq!(pinned.value, "xyz");
        assert_eq!(pinned.vendor, "akamai");
    }

    #[test]
    fn memory_expiry_drops_entry() {
        let store = InMemoryCookiePinStore::new();
        // ttl=0 means expired immediately (pinned_at + 0 <= now)
        store
            .pin("datadome", "https://b.test", "datadome", "abc", 0)
            .unwrap();
        let got = store
            .get_pinned("datadome", "https://b.test", "datadome")
            .unwrap();
        assert!(got.is_none(), "ttl=0 must be treated as expired");
    }

    #[test]
    fn memory_prune_removes_expired() {
        let store = InMemoryCookiePinStore::new();
        store.pin("v", "o", "live", "a", 3600).unwrap();
        store.pin("v", "o", "dead", "b", 0).unwrap();
        let removed = store.prune_expired().unwrap();
        assert_eq!(removed, 1);
        assert!(store.get_pinned("v", "o", "live").unwrap().is_some());
        assert!(store.get_pinned("v", "o", "dead").unwrap().is_none());
    }
}
