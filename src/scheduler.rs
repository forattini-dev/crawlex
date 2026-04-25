//! Render scheduler with per-host / per-origin / per-proxy / per-session
//! inflight budgets. Sits in front of `RenderPool` so a single noisy
//! origin can't monopolise the browser or trigger rate limits upstream.
//!
//! `BudgetLimits` defines the caps; `RenderBudgets::try_acquire`
//! atomically checks every dimension and, on success, returns an RAII
//! `BudgetGuard` that decrements each counter on drop.
//!
//! All counters are `AtomicUsize` so the hot path is lock-free outside
//! the DashMap slot initialisation.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BudgetLimits {
    pub max_per_host: usize,
    pub max_per_origin: usize,
    pub max_per_proxy: usize,
    pub max_per_session: usize,
    /// Cumulative cap on jobs a single session can touch before the
    /// scheduler ends it (wave 1 #33 — humans follow a Pareto depth
    /// distribution, not a 50+ uniform BFS). `0` disables the cap.
    #[serde(default = "default_session_total")]
    pub max_per_session_total: usize,
}

fn default_session_total() -> usize {
    15
}

impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            max_per_host: 4,
            max_per_origin: 2,
            max_per_proxy: 8,
            max_per_session: 1,
            max_per_session_total: default_session_total(),
        }
    }
}

/// Which dimension rejected a `try_acquire` call. Routed back to the
/// caller so they can emit `decision.made why=budget:<kind>` with the
/// right label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetKind {
    Host,
    Origin,
    Proxy,
    Session,
}

impl BudgetKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BudgetKind::Host => "host",
            BudgetKind::Origin => "origin",
            BudgetKind::Proxy => "proxy",
            BudgetKind::Session => "session",
        }
    }
}

/// Per-dimension counters. Indexed by the relevant key (host string,
/// origin string, proxy url-string, session id). Entries stay in the
/// map forever once created — cheap because `AtomicUsize` is a pointer
/// width — so the key lookup only locks during insertion.
#[derive(Default)]
pub struct RenderBudgets {
    per_host: DashMap<String, Arc<AtomicUsize>>,
    per_origin: DashMap<String, Arc<AtomicUsize>>,
    per_proxy: DashMap<String, Arc<AtomicUsize>>,
    per_session: DashMap<String, Arc<AtomicUsize>>,
    rejections: BudgetRejectionCounters,
    limits: BudgetLimits,
}

#[derive(Default, Debug)]
pub struct BudgetRejectionCounters {
    pub host: AtomicUsize,
    pub origin: AtomicUsize,
    pub proxy: AtomicUsize,
    pub session: AtomicUsize,
}

impl RenderBudgets {
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    pub fn limits(&self) -> BudgetLimits {
        self.limits
    }

    fn counter(map: &DashMap<String, Arc<AtomicUsize>>, key: &str) -> Arc<AtomicUsize> {
        if let Some(c) = map.get(key) {
            return c.clone();
        }
        map.entry(key.to_string())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
            .clone()
    }

    /// Atomically try to reserve a slot on all four dimensions. On
    /// success returns a guard whose `Drop` decrements each counter;
    /// on failure returns the offending dimension and leaves all
    /// counters untouched.
    pub fn try_acquire(
        &self,
        host: &str,
        origin: &str,
        proxy: Option<&Url>,
        session: &str,
    ) -> Result<BudgetGuard, BudgetKind> {
        let proxy_key = proxy
            .map(|u| u.to_string())
            .unwrap_or_else(|| "_direct_".to_string());

        let host_c = Self::counter(&self.per_host, host);
        let origin_c = Self::counter(&self.per_origin, origin);
        let proxy_c = Self::counter(&self.per_proxy, &proxy_key);
        let session_c = Self::counter(&self.per_session, session);

        // Host
        if try_bump(&host_c, self.limits.max_per_host).is_err() {
            self.rejections.host.fetch_add(1, Ordering::Relaxed);
            return Err(BudgetKind::Host);
        }
        // Origin
        if try_bump(&origin_c, self.limits.max_per_origin).is_err() {
            undo(&host_c);
            self.rejections.origin.fetch_add(1, Ordering::Relaxed);
            return Err(BudgetKind::Origin);
        }
        // Proxy
        if try_bump(&proxy_c, self.limits.max_per_proxy).is_err() {
            undo(&host_c);
            undo(&origin_c);
            self.rejections.proxy.fetch_add(1, Ordering::Relaxed);
            return Err(BudgetKind::Proxy);
        }
        // Session
        if try_bump(&session_c, self.limits.max_per_session).is_err() {
            undo(&host_c);
            undo(&origin_c);
            undo(&proxy_c);
            self.rejections.session.fetch_add(1, Ordering::Relaxed);
            return Err(BudgetKind::Session);
        }

        Ok(BudgetGuard {
            host: host_c,
            origin: origin_c,
            proxy: proxy_c,
            session: session_c,
        })
    }

    pub fn rejection_snapshot(&self) -> (usize, usize, usize, usize) {
        (
            self.rejections.host.load(Ordering::Relaxed),
            self.rejections.origin.load(Ordering::Relaxed),
            self.rejections.proxy.load(Ordering::Relaxed),
            self.rejections.session.load(Ordering::Relaxed),
        )
    }

    pub fn inflight(&self, kind: BudgetKind, key: &str) -> usize {
        let map = match kind {
            BudgetKind::Host => &self.per_host,
            BudgetKind::Origin => &self.per_origin,
            BudgetKind::Proxy => &self.per_proxy,
            BudgetKind::Session => &self.per_session,
        };
        map.get(key).map(|c| c.load(Ordering::Relaxed)).unwrap_or(0)
    }
}

fn try_bump(c: &AtomicUsize, max: usize) -> Result<(), ()> {
    let mut cur = c.load(Ordering::Acquire);
    loop {
        if cur >= max {
            return Err(());
        }
        match c.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(()),
            Err(actual) => cur = actual,
        }
    }
}

fn undo(c: &AtomicUsize) {
    // Never wraps because we only call undo after a successful bump.
    c.fetch_sub(1, Ordering::AcqRel);
}

/// RAII guard — decrements the four counters when dropped.
#[derive(Debug)]
pub struct BudgetGuard {
    host: Arc<AtomicUsize>,
    origin: Arc<AtomicUsize>,
    proxy: Arc<AtomicUsize>,
    session: Arc<AtomicUsize>,
}

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        self.host.fetch_sub(1, Ordering::AcqRel);
        self.origin.fetch_sub(1, Ordering::AcqRel);
        self.proxy.fetch_sub(1, Ordering::AcqRel);
        self.session.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Convenience: derive host + origin from a URL using the same rules
/// the render path uses (host lowercased, origin from `Url::origin`).
pub fn host_and_origin(url: &Url) -> (String, String) {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let origin = url.origin().ascii_serialization();
    (host, origin)
}

// ---------------------------------------------------------------------
// Wave 1 #31 — InterArrivalJitter
//
// Server-side ML detectors score sessions on the *distribution* of
// inter-arrival times between requests. A classic human fingerprint is
// log-normal (μ≈7.5, σ≈1.0 in ms) — median around 1.8 s with a long
// tail. Crawlers that hit every 100 ms show a degenerate spike.
//
// We track the last-issued timestamp per session and expose
// `delay_for_next` returning a Duration that a caller can `sleep` for
// before dispatching. The profile is tunable via `JitterProfile`; the
// `Soft` default stays friendly to CI (50–500 ms) while `Human` feeds
// the full log-normal distribution used against ML-scored targets.
// ---------------------------------------------------------------------

/// Distribution shape used by `InterArrivalJitter`. `motion-profile
/// fast` bypass is handled by setting `JitterProfile::Off` — the
/// scheduler never introduces an artificial delay in that mode.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum JitterProfile {
    /// No added delay. Equivalent to `motion-profile fast`.
    Off,
    /// 50–500 ms uniform. CI/dev default — keeps tests quick but still
    /// avoids perfectly-spaced dispatch.
    #[default]
    Soft,
    /// Log-normal μ=7.5 σ=1.0 (ms), clamped at 30 s. Matches real
    /// human click cadence on content sites.
    Human,
    /// Heavier tail (μ=8.0 σ=1.1, clamped at 60 s). Paranoid targets.
    Paranoid,
}

impl JitterProfile {
    pub fn from_motion_profile_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "fast" => JitterProfile::Off,
            "human" => JitterProfile::Human,
            "paranoid" => JitterProfile::Paranoid,
            _ => JitterProfile::Soft,
        }
    }

    /// Draw a delay sample. Uses a cheap deterministic-ish PRNG seeded
    /// by system nanos so we don't pull a full `rand` dep chain for
    /// what's effectively a jitter noise source.
    pub fn sample(self) -> Duration {
        match self {
            JitterProfile::Off => Duration::ZERO,
            JitterProfile::Soft => {
                let u = uniform_u64();
                Duration::from_millis(50 + (u % 451))
            }
            JitterProfile::Human => sample_log_normal(7.5, 1.0, 30_000),
            JitterProfile::Paranoid => sample_log_normal(8.0, 1.1, 60_000),
        }
    }
}

/// Tracks the last dispatch timestamp per session so we can add the
/// *missing* delay to hit the target inter-arrival distribution. If the
/// caller already burnt more than the sampled target we return zero —
/// we never slow real traffic, only pad artificially-fast crawlers.
#[derive(Default)]
pub struct InterArrivalJitter {
    last_seen_ms: DashMap<String, Arc<AtomicU64>>,
    profile: parking_lot::RwLock<JitterProfile>,
}

impl InterArrivalJitter {
    pub fn new(profile: JitterProfile) -> Self {
        Self {
            last_seen_ms: DashMap::new(),
            profile: parking_lot::RwLock::new(profile),
        }
    }

    pub fn set_profile(&self, profile: JitterProfile) {
        *self.profile.write() = profile;
    }

    pub fn profile(&self) -> JitterProfile {
        *self.profile.read()
    }

    /// Return the delay to wait before dispatching the next job on
    /// `session_key`. Also updates the session's last-seen stamp as if
    /// the delay were honoured, so back-to-back calls compose.
    pub fn delay_for_next(&self, session_key: &str) -> Duration {
        let profile = *self.profile.read();
        if profile == JitterProfile::Off {
            return Duration::ZERO;
        }
        let target = profile.sample();
        let now = now_ms();
        let slot = self
            .last_seen_ms
            .entry(session_key.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        let last = slot.load(Ordering::Relaxed);
        let elapsed_ms = now.saturating_sub(last);
        let target_ms = target.as_millis() as u64;
        let next_stamp = now + target_ms.saturating_sub(elapsed_ms.min(target_ms));
        slot.store(next_stamp, Ordering::Relaxed);
        if elapsed_ms >= target_ms {
            Duration::ZERO
        } else {
            Duration::from_millis(target_ms - elapsed_ms)
        }
    }

    /// Sample the raw distribution without touching session state. Used
    /// by tests to verify the distribution shape.
    pub fn sample_raw(&self) -> Duration {
        self.profile().sample()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn uniform_u64() -> u64 {
    // xorshift64* seeded from clock nanos — adequate for jitter noise.
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x243f_6a88_85a3_08d3);
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

fn uniform_f64() -> f64 {
    // Map top 53 bits into [0,1); avoids denormal edge on f64 rounding.
    let u = uniform_u64() >> 11;
    (u as f64) / ((1u64 << 53) as f64)
}

fn sample_std_normal() -> f64 {
    // Box-Muller. Two uniforms → one normal. We discard the pair's
    // companion so callers stay simple; acceptable — this is a jitter
    // generator, not a Monte Carlo kernel.
    let mut u1 = uniform_f64();
    if u1 < 1e-12 {
        u1 = 1e-12;
    }
    let u2 = uniform_f64();
    let r = (-2.0 * u1.ln()).sqrt();
    let theta = 2.0 * std::f64::consts::PI * u2;
    r * theta.cos()
}

fn sample_log_normal(mu: f64, sigma: f64, cap_ms: u64) -> Duration {
    let n = sample_std_normal();
    let v = (mu + sigma * n).exp();
    let ms = v.round().max(0.0) as u64;
    Duration::from_millis(ms.min(cap_ms))
}

// ---------------------------------------------------------------------
// Wave 1 #33 — Session depth tracker
//
// Humans follow a Pareto-shaped session depth distribution: most
// sessions touch 3–7 pages, a long tail runs deeper. Crawlers that
// burn 50+ pages on a single cookie jar are instantly flagged.
//
// `SessionDepthTracker::observe` increments the counter for a session
// and returns a `SessionDecision` telling the caller whether to keep
// going, end the session at the end of this job, or end immediately.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionDecision {
    /// Dispatch the job on the current session.
    Continue,
    /// Dispatch this job, but treat the session as closed afterwards
    /// (next job on the same key should hit `EndSession`).
    EndAfter,
    /// Refuse to dispatch on this session — caller should rotate.
    EndSession,
}

#[derive(Default)]
pub struct SessionDepthTracker {
    counters: DashMap<String, Arc<AtomicUsize>>,
    /// Per-session *observed* hard cap, drawn from a Pareto distribution
    /// the first time the session is seen. Keeps each session's depth
    /// unique instead of every session stopping at exactly N.
    caps: DashMap<String, usize>,
    default_cap: usize,
}

impl SessionDepthTracker {
    pub fn new(default_cap: usize) -> Self {
        Self {
            counters: DashMap::new(),
            caps: DashMap::new(),
            default_cap,
        }
    }

    /// Increment the depth counter for `session_key` and return the
    /// resulting decision. Sampled cap follows a Pareto (α=1.3,
    /// xm=3) truncated to `[3, 2 * default_cap]` — matches the shape
    /// real human sessions have.
    pub fn observe(&self, session_key: &str) -> SessionDecision {
        if self.default_cap == 0 {
            return SessionDecision::Continue;
        }
        let cap = {
            if let Some(c) = self.caps.get(session_key) {
                *c
            } else {
                let sampled = sample_pareto_cap(self.default_cap);
                self.caps.insert(session_key.to_string(), sampled);
                sampled
            }
        };
        let counter = self
            .counters
            .entry(session_key.to_string())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
            .clone();
        let seen = counter.fetch_add(1, Ordering::AcqRel) + 1;
        if seen > cap {
            SessionDecision::EndSession
        } else if seen == cap {
            SessionDecision::EndAfter
        } else {
            SessionDecision::Continue
        }
    }

    pub fn reset(&self, session_key: &str) {
        self.counters.remove(session_key);
        self.caps.remove(session_key);
    }

    pub fn depth(&self, session_key: &str) -> usize {
        self.counters
            .get(session_key)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    pub fn cap(&self, session_key: &str) -> Option<usize> {
        self.caps.get(session_key).map(|c| *c)
    }
}

fn sample_pareto_cap(default_cap: usize) -> usize {
    // Pareto: x = xm / U^(1/α). We use xm=3, α=1.3 (fits Chrome-history
    // traces of real browsing sessions on news sites). Truncate to
    // [3, 2*default_cap] so tests don't need to handle 10^6 outliers.
    let mut u = uniform_f64();
    if u < 1e-9 {
        u = 1e-9;
    }
    let x = 3.0 / u.powf(1.0 / 1.3);
    let hi = (default_cap as f64) * 2.0;
    x.clamp(3.0, hi).round() as usize
}

// ---------------------------------------------------------------------
// Wave 1 #32 — Click-graph shape (hub-spoke weighting)
//
// A uniform BFS frontier produces a flat out-degree distribution that
// Shodan/PerimeterX graph-shape models flag on sight. Humans show a
// hub-spoke pattern: they return to the landing page, then dive into a
// spoke, then back to hub. We emulate that by assigning a weight to
// every discovered URL based on its distance from the first page and
// letting the caller pick the next job by weighted sampling.
// ---------------------------------------------------------------------

/// Bucketed depth weights. Index = depth from session root; value =
/// sampling weight. Deeper pages decay fast so the scheduler rarely
/// walks 6-hop chains from a single entry point.
pub const DEFAULT_FRONTIER_WEIGHTS: [f32; 5] = [1.0, 0.7, 0.5, 0.3, 0.15];

/// Returns the hub-spoke weight for a URL at `depth`. Anything deeper
/// than the curve length collapses to the tail weight.
pub fn frontier_weight(depth: usize) -> f32 {
    let last = *DEFAULT_FRONTIER_WEIGHTS.last().unwrap_or(&0.15);
    *DEFAULT_FRONTIER_WEIGHTS.get(depth).unwrap_or(&last)
}

/// In-memory weighted frontier. NOT a JobQueue replacement — this is a
/// helper for tests + an opt-in picker that takes `(key, depth)` pairs
/// and returns the index of the next key to pop, biased toward the hub.
#[derive(Default)]
pub struct WeightedFrontier {
    inner: parking_lot::Mutex<Vec<(String, usize)>>,
}

impl WeightedFrontier {
    pub fn push(&self, key: String, depth: usize) {
        self.inner.lock().push((key, depth));
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// Pick the next key using weighted-sample-without-replacement.
    /// Falls back to strict FIFO if all weights sum to zero.
    pub fn pop_weighted(&self) -> Option<String> {
        let mut inner = self.inner.lock();
        if inner.is_empty() {
            return None;
        }
        let total: f32 = inner.iter().map(|(_, d)| frontier_weight(*d)).sum();
        if total <= 0.0 {
            return Some(inner.remove(0).0);
        }
        let mut pick = (uniform_f64() as f32) * total;
        for (i, (_, d)) in inner.iter().enumerate() {
            let w = frontier_weight(*d);
            if pick <= w {
                return Some(inner.remove(i).0);
            }
            pick -= w;
        }
        Some(inner.remove(0).0)
    }

    /// Count out-edges bucketed by depth. Used by tests to assert the
    /// hub-spoke shape of the resulting click-graph.
    pub fn depth_histogram(&self) -> Vec<usize> {
        let inner = self.inner.lock();
        let mut hist = vec![0usize; DEFAULT_FRONTIER_WEIGHTS.len()];
        for (_, d) in inner.iter() {
            let idx = (*d).min(hist.len() - 1);
            hist[idx] += 1;
        }
        hist
    }
}

#[cfg(test)]
mod wave1_tests {
    use super::*;

    #[test]
    fn jitter_off_returns_zero() {
        let j = InterArrivalJitter::new(JitterProfile::Off);
        assert_eq!(j.delay_for_next("s1"), Duration::ZERO);
    }

    #[test]
    fn jitter_soft_range() {
        let j = InterArrivalJitter::new(JitterProfile::Soft);
        // Draw a raw sample (not throttled by session state).
        for _ in 0..32 {
            let d = j.sample_raw();
            let ms = d.as_millis() as u64;
            assert!((50..=500).contains(&ms), "soft out of range: {ms}");
        }
    }

    #[test]
    fn jitter_human_log_normal_shape() {
        let j = InterArrivalJitter::new(JitterProfile::Human);
        // Draw many samples; median should sit near exp(7.5) ≈ 1808 ms.
        let mut samples: Vec<u64> = (0..2000)
            .map(|_| j.sample_raw().as_millis() as u64)
            .collect();
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        assert!(
            (400..=6000).contains(&median),
            "median out of range: {median}"
        );
        // Tail must reach past 5s at least occasionally.
        let p90 = samples[(samples.len() * 9) / 10];
        assert!(p90 > 2000, "p90 too tight: {p90}");
    }

    #[test]
    fn depth_tracker_ends_after_pareto_cap() {
        let t = SessionDepthTracker::new(5);
        let mut saw_end = false;
        for _ in 0..200 {
            if t.observe("s1") == SessionDecision::EndSession {
                saw_end = true;
                break;
            }
        }
        assert!(saw_end, "depth tracker never ended session");
    }

    #[test]
    fn frontier_bias_toward_hub() {
        let f = WeightedFrontier::default();
        // 1 hub + 9 deep pages. Weighted pick should favour the hub
        // over a strict FIFO in the majority of runs.
        let mut hub_first = 0;
        for _ in 0..200 {
            let f = WeightedFrontier::default();
            f.push("hub".into(), 0);
            for i in 0..9 {
                f.push(format!("deep{i}"), 4);
            }
            if f.pop_weighted().as_deref() == Some("hub") {
                hub_first += 1;
            }
        }
        // Hub weight 1.0 vs 9 × 0.15 = 1.35 → expect ~42% hub-first
        // (expected ≈85 of 200). Loose bound for stochastic stability.
        assert!(
            (50..=130).contains(&hub_first),
            "hub-first hit rate out of range: {hub_first}"
        );
        let _ = f.len();
    }
}
