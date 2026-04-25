//! Per-process DNS resolution cache with TTL.
//!
//! Backed by `hickory-resolver` (pure-Rust async DNS) instead of
//! `tokio::net::lookup_host`. The latter dispatches to glibc's
//! `getaddrinfo` via tokio's blocking pool — at high concurrency that
//! pool saturates and DNS becomes the dominant tail-latency source.
//!
//! Cache TTL falls back to the configured value when the resolver returns
//! an answer without TTL info; otherwise we honour the smallest TTL of
//! the returned record set so we don't keep a stale rotation.

use dashmap::DashMap;
use hickory_resolver::TokioResolver;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::{Error, Result};

#[derive(Clone)]
pub struct DnsCache {
    inner: Arc<DashMap<String, Entry>>,
    fallback_ttl: Duration,
    resolver: Arc<OnceLock<TokioResolver>>,
}

impl Default for DnsCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

#[derive(Clone)]
struct Entry {
    addrs: Vec<SocketAddr>,
    inserted: Instant,
    ttl: Duration,
}

impl DnsCache {
    pub fn new(fallback_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            fallback_ttl,
            resolver: Arc::new(OnceLock::new()),
        }
    }

    fn resolver(&self) -> Result<&TokioResolver> {
        if let Some(r) = self.resolver.get() {
            return Ok(r);
        }
        let r = TokioResolver::builder_tokio()
            .map_err(|e| Error::DnsResolution {
                host: String::new(),
                reason: format!("builder: {e}"),
            })?
            .build()
            .map_err(|e| Error::DnsResolution {
                host: String::new(),
                reason: format!("build: {e}"),
            })?;
        // get_or_init returns the existing one if a concurrent caller won.
        Ok(self.resolver.get_or_init(|| r))
    }

    pub async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        // If `host` is already a literal IP (v4 or v6), shortcut: skip DNS
        // and skip the cache so we don't fill it with synthetic entries.
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }

        let key = format!("{host}:{port}");
        if let Some(e) = self.inner.get(&key) {
            if e.inserted.elapsed() < e.ttl {
                return Ok(e.addrs.clone());
            }
        }

        let resolver = self.resolver()?;
        let lookup = resolver
            .lookup_ip(host)
            .await
            .map_err(|e| Error::DnsResolution {
                host: host.to_string(),
                reason: e.to_string(),
            })?;

        // Take the smallest TTL across the returned records — honouring a
        // 30 s record with a 300 s fallback would defeat the point of TTL.
        let ttl = lookup
            .as_lookup()
            .answers()
            .iter()
            .map(|r| Duration::from_secs(u64::from(r.ttl)))
            .min()
            .unwrap_or(self.fallback_ttl)
            .max(Duration::from_secs(1));

        // Preserve resolver order. The connector will try every returned
        // address before failing; reordering here would hide DNS policy
        // choices and make retries less explainable.
        let addrs: Vec<SocketAddr> = lookup.iter().map(|ip| SocketAddr::new(ip, port)).collect();
        if addrs.is_empty() {
            return Err(Error::DnsResolution {
                host: host.to_string(),
                reason: "no A/AAAA records".into(),
            });
        }
        self.inner.insert(
            key,
            Entry {
                addrs: addrs.clone(),
                inserted: Instant::now(),
                ttl,
            },
        );
        Ok(addrs)
    }
}
