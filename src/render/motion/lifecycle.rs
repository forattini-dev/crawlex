//! Document lifecycle simulation — focus/blur/visibilitychange.
//!
//! Page lifecycle is a fingerprint. A tab that never blurs, never loses
//! focus, and never goes `hidden` screams automation. reCAPTCHA v3 and
//! Akamai Bot Manager track `document.hidden` + `document.hasFocus()` in
//! their scoring models. This module models tab-switch / window-blur events
//! as a Pareto-distributed stream of `(hidden_for_ms)` values, and exposes a
//! CDP-driven emitter that injects a real `visibilitychange` + matching
//! focus/blur pair at the scheduled times.
//!
//! The emitter uses `Runtime.evaluate` to fire synthetic events via the
//! same `Document` surface the browser itself uses — we do not touch the
//! Chrome 149 handler patches, only the protocol.

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

use crate::render::motion::MotionProfile;

/// One lifecycle event: page went hidden for `hidden_ms`, then came back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleEvent {
    /// Wall-clock offset (ms from session start) when the page goes hidden.
    pub hide_at_ms: u64,
    /// Duration the page stays hidden before reappearing.
    pub hidden_ms: u64,
}

/// Lifecycle tunables per profile.
#[derive(Debug, Clone, Copy)]
pub struct LifecycleParams {
    /// Mean inter-event gap in ms (Poisson-ish — actual sampled from
    /// exponential around this mean).
    pub mean_gap_ms: u64,
    /// Pareto xₘ / α for the hidden dwell.
    pub hidden_scale_ms: f64,
    pub hidden_alpha: f64,
    /// Maximum dwell (ms) — unbounded Pareto would freeze renders.
    pub hidden_cap_ms: u64,
    /// Set to 0 to suppress emission entirely (Fast profile).
    pub enabled: bool,
}

impl LifecycleParams {
    pub fn for_profile(p: MotionProfile) -> Self {
        match p {
            MotionProfile::Fast => LifecycleParams {
                mean_gap_ms: 300_000,
                hidden_scale_ms: 0.0,
                hidden_alpha: 2.0,
                hidden_cap_ms: 0,
                enabled: false,
            },
            MotionProfile::Balanced => LifecycleParams {
                mean_gap_ms: 45_000,
                hidden_scale_ms: 1_500.0,
                hidden_alpha: 1.4,
                hidden_cap_ms: 12_000,
                enabled: true,
            },
            MotionProfile::Human => LifecycleParams {
                mean_gap_ms: 30_000,
                hidden_scale_ms: 2_000.0,
                hidden_alpha: 1.3,
                hidden_cap_ms: 20_000,
                enabled: true,
            },
            MotionProfile::Paranoid => LifecycleParams {
                mean_gap_ms: 20_000,
                hidden_scale_ms: 3_000.0,
                hidden_alpha: 1.2,
                hidden_cap_ms: 30_000,
                enabled: true,
            },
        }
    }
}

/// Schedule up to `n_events` lifecycle transitions over the session.
pub fn schedule(params: &LifecycleParams, n_events: usize, seed: u64) -> Vec<LifecycleEvent> {
    if !params.enabled || n_events == 0 {
        return Vec::new();
    }
    let mut rng = SmallRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(n_events);
    let mut t: u64 = 0;
    for _ in 0..n_events {
        let gap = exponential_ms(&mut rng, params.mean_gap_ms as f64) as u64;
        t = t.saturating_add(gap);
        let hidden = pareto(&mut rng, params.hidden_scale_ms, params.hidden_alpha)
            .clamp(params.hidden_scale_ms * 0.3, params.hidden_cap_ms as f64)
            as u64;
        out.push(LifecycleEvent {
            hide_at_ms: t,
            hidden_ms: hidden,
        });
        t = t.saturating_add(hidden);
    }
    out
}

/// JS snippet dispatched via `Runtime.evaluate` to simulate a hide
/// transition. Fires both `visibilitychange` (with `document.hidden=true`
/// via a property override) and a window `blur`, matching what a real
/// tab-switch emits. The property override auto-restores on show.
pub const HIDE_SNIPPET: &str = r#"
(() => {
  try {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('blur'));
  } catch (_) {}
})();
"#;

pub const SHOW_SNIPPET: &str = r#"
(() => {
  try {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('focus'));
  } catch (_) {}
})();
"#;

fn u01(rng: &mut SmallRng) -> f64 {
    (rng.random::<u32>() as f64) / (u32::MAX as f64 + 1.0)
}

fn exponential_ms(rng: &mut SmallRng, mean: f64) -> f64 {
    let u = u01(rng).clamp(1e-6, 1.0 - 1e-6);
    -mean * (1.0 - u).ln()
}

fn pareto(rng: &mut SmallRng, scale: f64, alpha: f64) -> f64 {
    let u = u01(rng).clamp(1e-6, 1.0 - 1e-6);
    scale * (1.0 - u).powf(-1.0 / alpha.max(0.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_profile_disables_events() {
        let p = LifecycleParams::for_profile(MotionProfile::Fast);
        assert!(!p.enabled);
        assert!(schedule(&p, 5, 1).is_empty());
    }

    #[test]
    fn events_are_monotonic() {
        let p = LifecycleParams::for_profile(MotionProfile::Balanced);
        let ev = schedule(&p, 10, 42);
        assert_eq!(ev.len(), 10);
        for w in ev.windows(2) {
            assert!(
                w[1].hide_at_ms >= w[0].hide_at_ms,
                "events must be in time order"
            );
        }
    }

    #[test]
    fn hidden_durations_within_cap() {
        let p = LifecycleParams::for_profile(MotionProfile::Balanced);
        let ev = schedule(&p, 200, 7);
        for e in &ev {
            assert!(e.hidden_ms <= p.hidden_cap_ms, "hidden dwell exceeded cap");
            assert!(e.hidden_ms > 0);
        }
    }

    #[test]
    fn schedule_is_deterministic() {
        let p = LifecycleParams::for_profile(MotionProfile::Balanced);
        let a = schedule(&p, 8, 123);
        let b = schedule(&p, 8, 123);
        assert_eq!(a, b);
    }

    #[test]
    fn snippets_are_non_empty() {
        assert!(HIDE_SNIPPET.contains("visibilitychange"));
        assert!(SHOW_SNIPPET.contains("visibilitychange"));
    }
}
