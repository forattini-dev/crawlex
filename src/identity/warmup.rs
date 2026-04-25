//! Session warming state machine — SCAFFOLD (issue #35).
//!
//! Background: login-gated targets (Instagram, LinkedIn, many banks) flag
//! a session as bot-like the moment it authenticates without prior
//! browsing. A "warm" session visited a handful of benign pages across a
//! few minutes before touching the login form, which matches real human
//! behaviour.
//!
//! This module provides a pure state machine the crawler / scheduler can
//! consult to *gate* login attempts until the warmup budget is spent. The
//! actual browsing-to-warm sequence (what URLs to visit, how to pace them)
//! is operator policy; we only track whether the budget has been met.
//!
//! **Scaffold status:** types + transitions + unit tests are real. The
//! crawler wire-up (querying `SessionWarmup::is_warm` before firing a
//! login action) is deferred — the code path currently has no login
//! concept, so this module is dormant until the identity pipeline grows
//! credential handling. Callers consuming this scaffold should treat
//! `SessionWarmup::default()` as "warmup disabled" and skip the gate.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// How many unique-URL visits a session must rack up before it's eligible
/// for login. Default is conservative enough that a single trivial crawl
/// can't accidentally warm a bogus identity.
pub const DEFAULT_MIN_VISITS: u32 = 5;
/// Minimum wall-clock budget spent warming. Pair with `MIN_VISITS` so a
/// rapid 5-page burst doesn't count.
pub const DEFAULT_MIN_ELAPSED: Duration = Duration::from_secs(10 * 60);
/// Minimum link-depth reached during warmup — a session that only hits
/// the homepage N times hasn't really browsed.
pub const DEFAULT_MIN_DEPTH: u32 = 2;

/// Operator-tunable knobs for the state machine. Carried in `Config` in a
/// later wave; for now the scaffold just exposes the defaults as constants.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WarmupPolicy {
    pub min_visits: u32,
    pub min_depth: u32,
    pub min_elapsed_secs: u64,
}

impl Default for WarmupPolicy {
    fn default() -> Self {
        Self {
            min_visits: DEFAULT_MIN_VISITS,
            min_depth: DEFAULT_MIN_DEPTH,
            min_elapsed_secs: DEFAULT_MIN_ELAPSED.as_secs(),
        }
    }
}

/// Discriminant exposed to operators / logs. Carries counters for the
/// `Warming` phase so the dashboard can show "3/5 visits, 4m30s elapsed"
/// without peeking at private state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum WarmupPhase {
    /// No browsing observed yet. Login attempts MUST be blocked.
    #[default]
    Cold,
    /// Accumulating visits. Login attempts are blocked until `is_warm`.
    Warming {
        urls_visited: u32,
        max_depth_reached: u32,
        elapsed_secs: u64,
    },
    /// Budget met — login attempts allowed.
    Warm,
}

/// Per-session warmup state. One instance is owned by each
/// `SessionIdentity` (see `crate::identity::bundle`).
///
/// Not thread-safe on its own — wrap in `Mutex` / `RwLock` at the session
/// registry level. The struct is intentionally small (`Instant` + two
/// u32 + a policy) so the lock contention footprint stays tiny.
#[derive(Debug, Clone)]
pub struct SessionWarmup {
    policy: WarmupPolicy,
    started_at: Option<Instant>,
    urls_visited: u32,
    max_depth_reached: u32,
    forced_warm: bool,
}

impl Default for SessionWarmup {
    fn default() -> Self {
        Self::new(WarmupPolicy::default())
    }
}

impl SessionWarmup {
    pub fn new(policy: WarmupPolicy) -> Self {
        Self {
            policy,
            started_at: None,
            urls_visited: 0,
            max_depth_reached: 0,
            forced_warm: false,
        }
    }

    pub fn policy(&self) -> WarmupPolicy {
        self.policy
    }

    /// Record a warmup visit. Depth is the link-distance from the seed.
    pub fn record_visit(&mut self, depth: u32) {
        if self.started_at.is_none() {
            self.started_at = Some(Instant::now());
        }
        self.urls_visited = self.urls_visited.saturating_add(1);
        if depth > self.max_depth_reached {
            self.max_depth_reached = depth;
        }
    }

    /// Escape hatch for operators who know a session is already trusted
    /// (e.g. resumed from a persisted cookie jar that's been used by a
    /// human). Idempotent.
    pub fn force_warm(&mut self) {
        self.forced_warm = true;
    }

    fn elapsed(&self) -> Duration {
        self.started_at
            .map(|t| t.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    /// True when all three budget dimensions (visits, depth, time) are met.
    pub fn is_warm(&self) -> bool {
        if self.forced_warm {
            return true;
        }
        self.urls_visited >= self.policy.min_visits
            && self.max_depth_reached >= self.policy.min_depth
            && self.elapsed().as_secs() >= self.policy.min_elapsed_secs
    }

    /// Public snapshot for metrics / CLI dashboards.
    pub fn phase(&self) -> WarmupPhase {
        if self.is_warm() {
            return WarmupPhase::Warm;
        }
        if self.started_at.is_none() {
            return WarmupPhase::Cold;
        }
        WarmupPhase::Warming {
            urls_visited: self.urls_visited,
            max_depth_reached: self.max_depth_reached,
            elapsed_secs: self.elapsed().as_secs(),
        }
    }

    /// Crawler gate — returns `Err` with a stable reason code when the
    /// caller should NOT fire a login attempt yet. Scaffold keeps the
    /// error type as a static string so the policy engine can match on it
    /// without importing this module.
    pub fn gate_login(&self) -> Result<(), &'static str> {
        if self.is_warm() {
            Ok(())
        } else if self.started_at.is_none() {
            Err("warmup:cold")
        } else {
            Err("warmup:insufficient")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_session_blocks_login() {
        let w = SessionWarmup::default();
        assert!(matches!(w.phase(), WarmupPhase::Cold));
        assert_eq!(w.gate_login(), Err("warmup:cold"));
    }

    #[test]
    fn warming_session_blocks_login() {
        let mut w = SessionWarmup::default();
        w.record_visit(1);
        w.record_visit(2);
        assert!(matches!(w.phase(), WarmupPhase::Warming { .. }));
        assert_eq!(w.gate_login(), Err("warmup:insufficient"));
    }

    #[test]
    fn force_warm_opens_gate() {
        let mut w = SessionWarmup::default();
        w.force_warm();
        assert!(w.is_warm());
        assert_eq!(w.gate_login(), Ok(()));
    }

    #[test]
    fn depth_requirement_matters() {
        // Even with enough visits + time, shallow browsing stays Warming.
        let policy = WarmupPolicy {
            min_visits: 3,
            min_depth: 2,
            min_elapsed_secs: 0,
        };
        let mut w = SessionWarmup::new(policy);
        w.record_visit(1);
        w.record_visit(1);
        w.record_visit(1);
        assert!(!w.is_warm());
        w.record_visit(2);
        assert!(w.is_warm());
    }
}
