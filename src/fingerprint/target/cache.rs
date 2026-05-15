//! Per-`host:port` cache for Warm-tier results.
//!
//! Slice B8 of PRD forattini-dev/crawlex#25. TTL-based with a manual
//! flush API. Defaults: 24h TTL. Cache is a `DashMap` so reads do not
//! contend across workers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;

use crate::fingerprint::detection::Detection;

/// One cached set of Detections per host. Owns the snapshot timestamp
/// for TTL decisions.
#[derive(Debug, Clone)]
pub struct CachedFingerprint {
    pub detections: Vec<Detection>,
    pub captured_at: Instant,
}

impl CachedFingerprint {
    pub fn is_fresh(&self, ttl: Duration) -> bool {
        self.captured_at.elapsed() < ttl
    }
}

pub struct WarmCache {
    inner: Arc<DashMap<String, CachedFingerprint>>,
    ttl: Duration,
}

impl WarmCache {
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub fn default_ttl() -> Self {
        Self::with_ttl(Duration::from_secs(24 * 3600))
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns the cached detections for `host` if still fresh.
    pub fn get(&self, host: &str) -> Option<Vec<Detection>> {
        let entry = self.inner.get(host)?;
        if entry.is_fresh(self.ttl) {
            Some(entry.detections.clone())
        } else {
            None
        }
    }

    /// Replace (or insert) the cache entry for `host`.
    pub fn put(&self, host: impl Into<String>, detections: Vec<Detection>) {
        self.inner.insert(
            host.into(),
            CachedFingerprint {
                detections,
                captured_at: Instant::now(),
            },
        );
    }

    /// Drop any cached entry for `host`. Next call to `get` returns
    /// `None`, forcing a Warm-tier re-run.
    pub fn invalidate(&self, host: &str) {
        self.inner.remove(host);
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for WarmCache {
    fn default() -> Self {
        Self::default_ttl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::detection::{
        Category, Detection, Evidence, EvidenceSource, Vendor,
    };

    fn one_detection() -> Vec<Detection> {
        vec![Detection::from_single(
            Category::Cdn,
            Vendor::Cloudflare,
            Evidence::new(EvidenceSource::Header, "cf-ray", 10),
        )]
    }

    #[test]
    fn put_then_get_returns_detections() {
        let c = WarmCache::default_ttl();
        c.put("example.com:443", one_detection());
        let got = c.get("example.com:443").unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn get_unknown_host_returns_none() {
        let c = WarmCache::default_ttl();
        assert!(c.get("nope:443").is_none());
    }

    #[test]
    fn invalidate_removes_entry() {
        let c = WarmCache::default_ttl();
        c.put("example.com:443", one_detection());
        c.invalidate("example.com:443");
        assert!(c.get("example.com:443").is_none());
    }

    #[test]
    fn expired_entry_returns_none() {
        let c = WarmCache::with_ttl(Duration::from_nanos(1));
        c.put("example.com:443", one_detection());
        std::thread::sleep(Duration::from_millis(2));
        assert!(c.get("example.com:443").is_none());
    }

    #[test]
    fn len_tracks_entries() {
        let c = WarmCache::default_ttl();
        assert!(c.is_empty());
        c.put("a:443", one_detection());
        c.put("b:443", one_detection());
        assert_eq!(c.len(), 2);
    }
}
