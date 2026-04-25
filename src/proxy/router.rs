//! `ProxyRouter` — score-driven proxy rotation with EWMA latency, quarantine
//! windows, and persisted host↔bundle affinity.
//!
//! Replaces the legacy `Vec<banned: bool>` rotator. Public API:
//!
//! * `pick(host, bundle_id)` — returns a live proxy, preferring any prior
//!   affinity pin for the (host, bundle_id) tuple, otherwise picking the
//!   highest-scoring live proxy according to the configured `RotationStrategy`.
//! * `record_outcome(proxy, outcome)` — updates counters, EWMA latency, and
//!   quarantine state. Sets `last_success_at` on success so recovery works.
//! * `evict(proxy)` — force-remove a proxy from rotation (operator / health
//!   checker). Preserves score snapshot for later diagnostics.
//! * `scores_snapshot()` — cheap read of `(Url, ProxyScore)` pairs for
//!   throttled SQLite persistence or metrics exposure.
//!
//! Feature-gate-free: intentionally does not depend on cdp-backend, sqlite,
//! or any rendering concept — all persistence wiring lives in the caller
//! (Crawler) so the mini build compiles identically.

use dashmap::DashMap;
use parking_lot::Mutex;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

/// Rotation strategy for `ProxyRouter::pick`. Kept as a separate enum so the
/// `Config` layer and CLI can parse a string ("round-robin", "sticky-per-host",
/// etc.) without depending on the router's internals.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum RotationStrategy {
    #[default]
    RoundRobin,
    Sequential,
    Random,
    StickyPerHost,
}

/// Tunable thresholds for quarantine + score floors. Intentionally small —
/// policy-level knobs (retries, backoff) live in `PolicyThresholds`; this
/// struct only covers the router's own heuristics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RouterThresholds {
    /// N consecutive non-success outcomes → quarantine.
    pub max_consecutive_failures: u32,
    /// Minimum success rate over lifetime counters — below this, the proxy
    /// is considered "low score" and de-prioritised (but still picked when
    /// no healthier peer exists).
    pub min_success_rate: f64,
    /// Base quarantine window used on failure streaks.
    pub quarantine_secs: u64,
    /// Longer quarantine used when a challenge page was detected (antibot).
    pub challenge_quarantine_secs: u64,
}

impl Default for RouterThresholds {
    fn default() -> Self {
        Self {
            max_consecutive_failures: 3,
            min_success_rate: 0.5,
            quarantine_secs: 30,
            challenge_quarantine_secs: 300,
        }
    }
}

/// Outcome of a single request through a given proxy. `record_outcome` folds
/// this into the running score + quarantine state.
#[derive(Debug, Clone, Copy)]
pub enum ProxyOutcome {
    Success {
        latency_ms: f64,
    },
    Timeout,
    Reset,
    Status(u16),
    /// Antibot challenge detected in the response. Currently wire-only — task
    /// 4.2 owns the detection plumbing; the router already handles it.
    ChallengeHit,
    ConnectFailed,
}

/// Cumulative per-proxy score — all fields are monotonic counters except
/// the EWMA latency values and the `last_*` timestamps.
#[derive(Debug, Clone, Default)]
pub struct ProxyScore {
    pub success: u32,
    pub timeouts: u32,
    pub resets: u32,
    pub status_4xx: u32,
    pub status_5xx: u32,
    pub challenge_hits: u32,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    pub last_success_at: Option<Instant>,
    pub quarantine_until: Option<Instant>,
    pub consecutive_failures: u32,
}

impl ProxyScore {
    /// Success rate over total observed outcomes. Returns 1.0 when the
    /// proxy has never been used (optimistic default so freshly added
    /// proxies get traffic).
    pub fn success_rate(&self) -> f64 {
        let total = self.success
            + self.timeouts
            + self.resets
            + self.status_4xx
            + self.status_5xx
            + self.challenge_hits;
        if total == 0 {
            return 1.0;
        }
        self.success as f64 / total as f64
    }

    /// Composite score in `[0.0, 1.0]`. Used by `pick` to rank live proxies
    /// and by `PolicyEngine` via `Crawler::proxy_score_for`.
    pub fn composite(&self) -> f64 {
        // Weighted: success-rate (0.7) + latency bonus (0.3 if under 1s).
        let rate = self.success_rate();
        let latency_bonus = match self.latency_p50_ms {
            Some(p50) if p50 > 0.0 => (1_000.0 / (p50 + 1_000.0)).clamp(0.0, 1.0),
            _ => 0.5,
        };
        0.7 * rate + 0.3 * latency_bonus
    }

    pub fn is_quarantined(&self, now: Instant) -> bool {
        self.quarantine_until.is_some_and(|t| t > now)
    }
}

/// Serializable snapshot form — used when persisting to / loading from SQLite.
/// Timestamps are represented as epoch seconds so they survive restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyScoreSnapshot {
    pub url: String,
    pub success: u32,
    pub timeouts: u32,
    pub resets: u32,
    pub status_4xx: u32,
    pub status_5xx: u32,
    pub challenge_hits: u32,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    pub last_success_at_unix: Option<i64>,
    pub quarantine_until_unix: Option<i64>,
}

/// EWMA α for latency smoothing. 0.2 weighs the most recent sample at 20%,
/// giving a roughly 5-sample memory — responsive to shifts without thrashing
/// on one outlier.
const LATENCY_ALPHA: f64 = 0.2;

struct Entry {
    url: Url,
    score: Mutex<ProxyScore>,
    evicted: Mutex<bool>,
}

/// Routing engine: picks proxies, records outcomes, persists affinity.
pub struct ProxyRouter {
    entries: Vec<Entry>,
    url_index: HashMap<String, usize>,
    strategy: RotationStrategy,
    thresholds: RouterThresholds,
    cursor: AtomicUsize,
    /// `(host, bundle_id) → proxy index`. Persisted out-of-band by the
    /// caller (via `affinity_snapshot`).
    affinity: DashMap<(String, u64), usize>,
    /// Pending score updates awaiting flush. Caller drains via
    /// `drain_pending`. Bounded implicitly by the number of proxies.
    pending_dirty: DashMap<usize, ()>,
    /// Pending affinity updates. `(host, bundle_id, proxy_url)`.
    pending_affinity: Mutex<Vec<(String, u64, Url)>>,
}

impl ProxyRouter {
    pub fn new(
        proxies: Vec<Url>,
        strategy: RotationStrategy,
        thresholds: RouterThresholds,
    ) -> Self {
        let mut url_index = HashMap::with_capacity(proxies.len());
        let entries: Vec<Entry> = proxies
            .into_iter()
            .enumerate()
            .map(|(i, url)| {
                url_index.insert(url.to_string(), i);
                Entry {
                    url,
                    score: Mutex::new(ProxyScore::default()),
                    evicted: Mutex::new(false),
                }
            })
            .collect();
        Self {
            entries,
            url_index,
            strategy,
            thresholds,
            cursor: AtomicUsize::new(0),
            affinity: DashMap::new(),
            pending_dirty: DashMap::new(),
            pending_affinity: Mutex::new(Vec::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Hydrate scores from a persisted snapshot at startup. Unknown URLs are
    /// silently dropped (operator removed them from config) — the pool is the
    /// source of truth for membership, SQLite just carries history.
    pub fn hydrate_scores(
        &self,
        snapshots: Vec<(ProxyScoreSnapshot, Option<Instant>, Option<Instant>)>,
    ) {
        for (snap, last_success, quarantine_until) in snapshots {
            let Some(&idx) = self.url_index.get(&snap.url) else {
                continue;
            };
            let mut s = self.entries[idx].score.lock();
            s.success = snap.success;
            s.timeouts = snap.timeouts;
            s.resets = snap.resets;
            s.status_4xx = snap.status_4xx;
            s.status_5xx = snap.status_5xx;
            s.challenge_hits = snap.challenge_hits;
            s.latency_p50_ms = snap.latency_p50_ms;
            s.latency_p95_ms = snap.latency_p95_ms;
            s.last_success_at = last_success;
            s.quarantine_until = quarantine_until;
        }
    }

    /// Hydrate host affinity table at startup.
    pub fn hydrate_affinity(&self, entries: Vec<(String, u64, String)>) {
        for (host, bundle, url) in entries {
            if let Some(&idx) = self.url_index.get(&url) {
                self.affinity.insert((host, bundle), idx);
            }
        }
    }

    /// Score-aware pick. Returns `None` when the pool is empty or every proxy
    /// is quarantined/evicted.
    pub fn pick(&self, host: &str, bundle_id: u64) -> Option<Url> {
        if self.entries.is_empty() {
            return None;
        }
        let now = Instant::now();

        // Fast path: honour an existing affinity pin as long as the pinned
        // proxy is still alive.
        if let Some(idx_ref) = self.affinity.get(&(host.to_string(), bundle_id)) {
            let idx = *idx_ref;
            if self.is_alive(idx, now) {
                return Some(self.entries[idx].url.clone());
            }
        }

        let live: Vec<usize> = (0..self.entries.len())
            .filter(|&i| self.is_alive(i, now))
            .collect();
        if live.is_empty() {
            return None;
        }

        let idx = match self.strategy {
            RotationStrategy::RoundRobin => {
                let c = self.cursor.fetch_add(1, Ordering::Relaxed);
                live[c % live.len()]
            }
            RotationStrategy::Sequential => {
                let c = self.cursor.load(Ordering::Relaxed);
                live[c % live.len()]
            }
            RotationStrategy::Random => {
                let mut rng = rand::rng();
                *live.choose(&mut rng).unwrap()
            }
            RotationStrategy::StickyPerHost => {
                // Score-ranked: pick the best-scoring live proxy, tie-break
                // by hashed host so the same host keeps landing on the same
                // proxy as long as scores stay comparable.
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                host.hash(&mut h);
                let hv = h.finish() as usize;
                let mut scored: Vec<(usize, f64)> = live
                    .iter()
                    .map(|&i| (i, self.entries[i].score.lock().composite()))
                    .collect();
                // Highest composite first; within a tolerance of 0.05, fall
                // back to hash-based choice so pinning is stable.
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                let top_score = scored[0].1;
                let top_band: Vec<usize> = scored
                    .iter()
                    .filter(|(_, s)| (top_score - s).abs() <= 0.05)
                    .map(|(i, _)| *i)
                    .collect();
                top_band[hv % top_band.len()]
            }
        };

        // Pin this (host, bundle) to the chosen proxy for stickiness + logs.
        let key = (host.to_string(), bundle_id);
        let changed = self
            .affinity
            .get(&key)
            .map(|prev| *prev != idx)
            .unwrap_or(true);
        if changed {
            self.affinity.insert(key.clone(), idx);
            self.pending_affinity
                .lock()
                .push((key.0, key.1, self.entries[idx].url.clone()));
        }
        Some(self.entries[idx].url.clone())
    }

    fn is_alive(&self, idx: usize, now: Instant) -> bool {
        if *self.entries[idx].evicted.lock() {
            return false;
        }
        let s = self.entries[idx].score.lock();
        !s.is_quarantined(now)
    }

    /// Fold a single outcome into the proxy's running score. Silently ignores
    /// unknown URLs so the caller never needs to branch on "was this proxy
    /// ours?" — caller just funnels every result through here.
    pub fn record_outcome(&self, proxy: &Url, outcome: ProxyOutcome) {
        let Some(&idx) = self.url_index.get(proxy.as_str()) else {
            return;
        };
        let now = Instant::now();
        {
            let mut s = self.entries[idx].score.lock();
            match outcome {
                ProxyOutcome::Success { latency_ms } => {
                    s.success = s.success.saturating_add(1);
                    s.last_success_at = Some(now);
                    s.consecutive_failures = 0;
                    s.latency_p50_ms = Some(match s.latency_p50_ms {
                        Some(prev) => prev + LATENCY_ALPHA * (latency_ms - prev),
                        None => latency_ms,
                    });
                    // p95 tracked as a slow-moving upper-envelope EWMA: only
                    // samples above the current p50 drag it up.
                    let p95_sample = latency_ms.max(s.latency_p50_ms.unwrap_or(latency_ms));
                    s.latency_p95_ms = Some(match s.latency_p95_ms {
                        Some(prev) => prev + (LATENCY_ALPHA * 0.5) * (p95_sample - prev),
                        None => p95_sample,
                    });
                }
                ProxyOutcome::Timeout => {
                    s.timeouts = s.timeouts.saturating_add(1);
                    s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                }
                ProxyOutcome::Reset | ProxyOutcome::ConnectFailed => {
                    s.resets = s.resets.saturating_add(1);
                    s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                }
                ProxyOutcome::Status(code) => {
                    if (400..500).contains(&code) {
                        s.status_4xx = s.status_4xx.saturating_add(1);
                    } else if (500..600).contains(&code) {
                        s.status_5xx = s.status_5xx.saturating_add(1);
                        s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                    } else if (200..400).contains(&code) {
                        // 2xx/3xx bucketed as success for counter purposes;
                        // caller owns latency via `Success { latency_ms }`.
                        s.success = s.success.saturating_add(1);
                        s.last_success_at = Some(now);
                        s.consecutive_failures = 0;
                    }
                }
                ProxyOutcome::ChallengeHit => {
                    s.challenge_hits = s.challenge_hits.saturating_add(1);
                    s.consecutive_failures = s.consecutive_failures.saturating_add(1);
                    s.quarantine_until =
                        Some(now + Duration::from_secs(self.thresholds.challenge_quarantine_secs));
                }
            }

            // Consecutive-failure trip: quarantine (unless challenge path
            // already set it to a longer window).
            if s.consecutive_failures >= self.thresholds.max_consecutive_failures {
                let target = now + Duration::from_secs(self.thresholds.quarantine_secs);
                let keep_longer = s.quarantine_until.is_some_and(|t| t > target);
                if !keep_longer {
                    s.quarantine_until = Some(target);
                }
            }
        }
        self.pending_dirty.insert(idx, ());
    }

    /// Manually evict a proxy from rotation. Score is preserved so operators
    /// can still read it via `scores_snapshot`.
    pub fn evict(&self, proxy: &Url) {
        if let Some(&idx) = self.url_index.get(proxy.as_str()) {
            *self.entries[idx].evicted.lock() = true;
            self.pending_dirty.insert(idx, ());
        }
    }

    /// Restore an evicted proxy (inverse of `evict`).
    pub fn reinstate(&self, proxy: &Url) {
        if let Some(&idx) = self.url_index.get(proxy.as_str()) {
            *self.entries[idx].evicted.lock() = false;
            self.pending_dirty.insert(idx, ());
        }
    }

    /// Composite score for a specific proxy. Used by `PolicyEngine` via
    /// `PolicyContext.proxy_score`.
    pub fn score_for(&self, proxy: &Url) -> Option<f32> {
        let idx = *self.url_index.get(proxy.as_str())?;
        Some(self.entries[idx].score.lock().composite() as f32)
    }

    /// Read-only snapshot of all scores. Cheap enough to call per flush tick.
    pub fn scores_snapshot(&self) -> Vec<(Url, ProxyScore)> {
        self.entries
            .iter()
            .map(|e| (e.url.clone(), e.score.lock().clone()))
            .collect()
    }

    /// Drain dirty indices + affinity updates; caller persists them to SQLite.
    /// Combined drain keeps the flush write a single transaction.
    #[allow(clippy::type_complexity)]
    pub fn drain_pending(&self) -> (Vec<(Url, ProxyScore)>, Vec<(String, u64, Url)>) {
        let mut dirty_scores = Vec::new();
        let dirty_keys: Vec<usize> = self.pending_dirty.iter().map(|kv| *kv.key()).collect();
        for idx in dirty_keys {
            self.pending_dirty.remove(&idx);
            let score = self.entries[idx].score.lock().clone();
            dirty_scores.push((self.entries[idx].url.clone(), score));
        }
        let affinity_updates = std::mem::take(&mut *self.pending_affinity.lock());
        (dirty_scores, affinity_updates)
    }

    /// Count of pending dirty score entries awaiting flush. Used by the
    /// throttled flush loop to short-circuit the `save_*` round-trip when
    /// nothing has changed.
    pub fn pending_dirty_len(&self) -> usize {
        self.pending_dirty.len()
    }

    /// Best-scoring live proxy (used by `Decision::SwitchProxy`).
    pub fn best_alternative(&self, avoid: &Url, host: &str, bundle_id: u64) -> Option<Url> {
        let now = Instant::now();
        let mut best: Option<(usize, f64)> = None;
        for (i, e) in self.entries.iter().enumerate() {
            if e.url == *avoid {
                continue;
            }
            if !self.is_alive(i, now) {
                continue;
            }
            let s = e.score.lock().composite();
            if best.map(|(_, bs)| s > bs).unwrap_or(true) {
                best = Some((i, s));
            }
        }
        let (idx, _) = best?;
        // Re-pin affinity to the new proxy so follow-up picks stay sticky.
        let key = (host.to_string(), bundle_id);
        self.affinity.insert(key.clone(), idx);
        self.pending_affinity
            .lock()
            .push((key.0, key.1, self.entries[idx].url.clone()));
        Some(self.entries[idx].url.clone())
    }
}

/// Convert a `(Url, ProxyScore)` drain row into the serializable
/// snapshot form used by the SQLite writer. Instants round-trip as
/// unix-epoch seconds so persisted state survives a restart.
fn now_plus_offset_unix(now_instant: Instant, now_unix: i64, t: Instant) -> i64 {
    // Instant is monotonic and not directly convertible to wall time.
    // We anchor "now" to a single (Instant, unix_secs) pair and project
    // the target Instant to wall time relative to that anchor.
    let delta = t
        .checked_duration_since(now_instant)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(|| -(now_instant.duration_since(t).as_secs() as i64));
    now_unix + delta
}

/// Convert a score row back from unix-epoch seconds into a live `Instant`,
/// clamping past timestamps to "right now" so a stale quarantine from a
/// previous run doesn't pin us.
fn unix_to_instant(now_instant: Instant, now_unix: i64, target_unix: i64) -> Instant {
    let delta = target_unix - now_unix;
    if delta >= 0 {
        now_instant + Duration::from_secs(delta as u64)
    } else {
        now_instant
            .checked_sub(Duration::from_secs((-delta) as u64))
            .unwrap_or(now_instant)
    }
}

/// Package drained scores + affinity into SQLite row types. Feature-gated
/// because `ProxyScoreRow` only exists with the `sqlite` feature.
#[cfg(feature = "sqlite")]
pub fn pack_score_rows(
    drained: Vec<(Url, ProxyScore)>,
) -> Vec<crate::storage::sqlite::ProxyScoreRow> {
    use crate::storage::sqlite::ProxyScoreRow;
    let now_instant = Instant::now();
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    drained
        .into_iter()
        .map(|(url, s)| ProxyScoreRow {
            url: url.to_string(),
            success: s.success as i64,
            timeouts: s.timeouts as i64,
            resets: s.resets as i64,
            status_4xx: s.status_4xx as i64,
            status_5xx: s.status_5xx as i64,
            challenge_hits: s.challenge_hits as i64,
            latency_p50_ms: s.latency_p50_ms,
            latency_p95_ms: s.latency_p95_ms,
            last_success_at: s
                .last_success_at
                .map(|t| now_plus_offset_unix(now_instant, now_unix, t)),
            quarantine_until: s
                .quarantine_until
                .map(|t| now_plus_offset_unix(now_instant, now_unix, t)),
        })
        .collect()
}

/// Hydrate the router from a previously-persisted SQLite snapshot.
#[cfg(feature = "sqlite")]
pub async fn hydrate_from_storage(
    router: &ProxyRouter,
    storage: &crate::storage::sqlite::SqliteStorage,
) -> crate::Result<()> {
    let rows = storage.load_proxy_scores().await?;
    let now_instant = Instant::now();
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let snaps: Vec<(ProxyScoreSnapshot, Option<Instant>, Option<Instant>)> = rows
        .into_iter()
        .map(|r| {
            let last_success = r
                .last_success_at
                .map(|u| unix_to_instant(now_instant, now_unix, u));
            let quarantine = r
                .quarantine_until
                .map(|u| unix_to_instant(now_instant, now_unix, u));
            (
                ProxyScoreSnapshot {
                    url: r.url,
                    success: r.success as u32,
                    timeouts: r.timeouts as u32,
                    resets: r.resets as u32,
                    status_4xx: r.status_4xx as u32,
                    status_5xx: r.status_5xx as u32,
                    challenge_hits: r.challenge_hits as u32,
                    latency_p50_ms: r.latency_p50_ms,
                    latency_p95_ms: r.latency_p95_ms,
                    last_success_at_unix: r.last_success_at,
                    quarantine_until_unix: r.quarantine_until,
                },
                last_success,
                quarantine,
            )
        })
        .collect();
    router.hydrate_scores(snaps);
    let aff = storage.load_host_affinity().await?;
    let entries: Vec<(String, u64, String)> =
        aff.into_iter().map(|(h, b, u)| (h, b as u64, u)).collect();
    router.hydrate_affinity(entries);
    Ok(())
}

/// Spawn the throttled flush loop. Drains the router's pending dirty
/// queue every `interval` (or immediately once `batch_threshold` changes
/// pile up) and writes them to SQLite via the writer thread. Returns the
/// spawned `JoinHandle` so callers can abort on shutdown; dropping it
/// leaks the task until the router itself is dropped.
#[cfg(feature = "sqlite")]
pub fn start_flush_loop(
    router: Arc<ProxyRouter>,
    storage: Arc<crate::storage::sqlite::SqliteStorage>,
    interval: Duration,
    batch_threshold: usize,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                // Lightweight fast path: if many pending, don't wait for
                // the tick. Cheap busy check every 50ms is enough — no
                // hot loop because pending_dirty_len is O(1) on DashMap.
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if router.pending_dirty_len() < batch_threshold {
                        continue;
                    }
                }
            }
            let (scores, affinity) = router.drain_pending();
            if !scores.is_empty() {
                let rows = pack_score_rows(scores);
                if let Err(e) = storage.save_proxy_scores(rows).await {
                    tracing::debug!(?e, "proxy router flush: save_proxy_scores failed");
                }
            }
            if !affinity.is_empty() {
                let rows: Vec<(String, i64, String)> = affinity
                    .into_iter()
                    .map(|(h, b, u)| (h, b as i64, u.to_string()))
                    .collect();
                if let Err(e) = storage.save_host_affinity(rows).await {
                    tracing::debug!(?e, "proxy router flush: save_host_affinity failed");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router(n: usize, strat: RotationStrategy) -> ProxyRouter {
        let urls: Vec<Url> = (0..n)
            .map(|i| Url::parse(&format!("http://p{i}.test:8080")).unwrap())
            .collect();
        ProxyRouter::new(urls, strat, RouterThresholds::default())
    }

    #[test]
    fn pick_returns_none_on_empty() {
        let r = ProxyRouter::new(
            vec![],
            RotationStrategy::RoundRobin,
            RouterThresholds::default(),
        );
        assert!(r.pick("example.com", 0).is_none());
    }

    #[test]
    fn happy_path_score_converges() {
        let r = router(2, RotationStrategy::RoundRobin);
        let p = r.pick("a.com", 0).unwrap();
        for _ in 0..10 {
            r.record_outcome(&p, ProxyOutcome::Success { latency_ms: 100.0 });
        }
        let snap = r.scores_snapshot();
        let s = snap.iter().find(|(u, _)| u == &p).unwrap().1.clone();
        assert_eq!(s.success, 10);
        assert!(s.quarantine_until.is_none());
        assert!(s.latency_p50_ms.is_some());
        let p50 = s.latency_p50_ms.unwrap();
        assert!(
            (p50 - 100.0).abs() < 1.0,
            "p50 should converge to 100ms: {p50}"
        );
    }

    #[test]
    fn degradation_triggers_quarantine() {
        let r = router(2, RotationStrategy::RoundRobin);
        let p = r.pick("b.com", 0).unwrap();
        for _ in 0..5 {
            r.record_outcome(&p, ProxyOutcome::Success { latency_ms: 120.0 });
        }
        for _ in 0..3 {
            r.record_outcome(&p, ProxyOutcome::Timeout);
        }
        let s = r
            .scores_snapshot()
            .into_iter()
            .find(|(u, _)| u == &p)
            .unwrap()
            .1;
        assert!(s.quarantine_until.is_some());
        assert!(s.is_quarantined(Instant::now()));
    }

    #[test]
    fn recovery_after_quarantine_expires() {
        let thresholds = RouterThresholds {
            quarantine_secs: 0,
            ..RouterThresholds::default()
        };
        let urls: Vec<Url> = (0..1)
            .map(|i| Url::parse(&format!("http://p{i}.test:8080")).unwrap())
            .collect();
        let r = ProxyRouter::new(urls, RotationStrategy::RoundRobin, thresholds);
        let p = r.pick("c.com", 0).unwrap();
        for _ in 0..3 {
            r.record_outcome(&p, ProxyOutcome::Timeout);
        }
        // Quarantine_secs = 0 → already expired. Next pick should succeed.
        std::thread::sleep(Duration::from_millis(10));
        let again = r.pick("c.com", 0);
        assert_eq!(again, Some(p));
    }

    #[test]
    fn affinity_is_sticky() {
        let r = router(3, RotationStrategy::RoundRobin);
        let first = r.pick("sticky.com", 42).unwrap();
        for _ in 0..5 {
            let p = r.pick("sticky.com", 42).unwrap();
            assert_eq!(p, first);
        }
    }

    #[test]
    fn affinity_skips_quarantined_pin() {
        let r = router(2, RotationStrategy::RoundRobin);
        let first = r.pick("skip.com", 1).unwrap();
        for _ in 0..3 {
            r.record_outcome(&first, ProxyOutcome::Timeout);
        }
        let next = r.pick("skip.com", 1).unwrap();
        assert_ne!(next, first);
    }

    #[test]
    fn challenge_hit_long_quarantine() {
        let r = router(1, RotationStrategy::RoundRobin);
        let p = r.pick("ch.com", 0).unwrap();
        r.record_outcome(&p, ProxyOutcome::ChallengeHit);
        let s = r.scores_snapshot().pop().unwrap().1;
        let q = s.quarantine_until.unwrap();
        assert!(q > Instant::now() + Duration::from_secs(60));
    }

    #[test]
    fn best_alternative_avoids_current() {
        let r = router(3, RotationStrategy::RoundRobin);
        let urls: Vec<Url> = r.entries.iter().map(|e| e.url.clone()).collect();
        // Make urls[1] clearly best.
        for _ in 0..5 {
            r.record_outcome(&urls[1], ProxyOutcome::Success { latency_ms: 50.0 });
        }
        for _ in 0..2 {
            r.record_outcome(&urls[0], ProxyOutcome::Success { latency_ms: 500.0 });
        }
        let alt = r.best_alternative(&urls[0], "alt.com", 0).unwrap();
        assert_eq!(alt, urls[1]);
    }

    #[test]
    fn drain_pending_returns_changes() {
        let r = router(2, RotationStrategy::RoundRobin);
        let p = r.pick("d.com", 0).unwrap();
        r.record_outcome(&p, ProxyOutcome::Success { latency_ms: 80.0 });
        let (scores, aff) = r.drain_pending();
        assert!(!scores.is_empty());
        assert!(!aff.is_empty());
        // Second drain sees nothing.
        let (s2, a2) = r.drain_pending();
        assert!(s2.is_empty());
        assert!(a2.is_empty());
    }
}
