//! Idle cursor drift — Ornstein-Uhlenbeck stationary process per page.
//!
//! When no action is running, a real user's cursor doesn't sit frozen. Hand
//! tremor, display micro-motion, and subconscious movements all add small,
//! mean-reverting perturbations. Modern antibot ML (Cloudflare, DataDome)
//! flags the "cursor hasn't moved in N seconds while page is focused" signal
//! as a bot tell. This module models that ambient drift as an Ornstein-
//! Uhlenbeck process running in its own tokio task, pausing whenever an
//! action takes the wheel.
//!
//! The engine here is pure math (`IdleDrift`): tests can sample it with no
//! tokio/CDP dependencies. `interact::spawn_idle_drift` owns the runtime
//! wiring — see that function for the pause/resume handshake.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

use crate::render::motion::MotionProfile;

/// Running state shared between the idle driver task and the action layer.
/// `action_active.store(true)` pauses the driver; flip back to `false` to
/// resume. `resume_at_ms` is the wall-clock monotonic timestamp (from
/// `Instant::now()` epoch millis) after which drift may restart — used to
/// stagger resume after a scheduled pause expires rather than busy-looping.
#[derive(Debug, Default)]
pub struct IdleState {
    pub action_active: AtomicBool,
    pub resume_at_ms: AtomicU64,
}

impl IdleState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Mark an action as starting. Drift halts until `action_end()`.
    pub fn action_begin(&self) {
        self.action_active.store(true, Ordering::Release);
    }

    pub fn action_end(&self) {
        self.action_active.store(false, Ordering::Release);
    }

    pub fn is_action_active(&self) -> bool {
        self.action_active.load(Ordering::Acquire)
    }
}

/// Pure-math OU drift generator. Produces (dx, dy) offsets around the
/// cursor's resting position. Callers clamp the absolute position inside
/// the viewport before dispatching.
pub struct IdleDrift {
    x: f64,
    y: f64,
    theta: f64,
    sigma: f64,
    pub step_ms_min: u64,
    pub step_ms_max: u64,
    rng: SmallRng,
}

impl IdleDrift {
    pub fn for_profile(p: MotionProfile, seed: u64) -> Self {
        let (theta, sigma, lo, hi) = match p {
            MotionProfile::Fast => (1.0, 0.0, 10_000, 20_000),
            MotionProfile::Balanced => (0.35, 0.9, 300, 900),
            MotionProfile::Human => (0.3, 1.2, 400, 1_200),
            MotionProfile::Paranoid => (0.25, 1.6, 500, 1_800),
        };
        Self {
            x: 0.0,
            y: 0.0,
            theta,
            sigma,
            step_ms_min: lo,
            step_ms_max: hi,
            rng: SmallRng::seed_from_u64(seed),
        }
    }

    /// Advance one OU step. Returns the absolute offset (dx, dy) that should
    /// be *added* to the resting cursor position.
    pub fn next_offset(&mut self) -> (f64, f64) {
        if self.sigma <= 0.0 {
            return (0.0, 0.0);
        }
        let (nx, ny) = gaussian_pair(&mut self.rng);
        self.x = self.x * (1.0 - self.theta) + self.sigma * nx;
        self.y = self.y * (1.0 - self.theta) + self.sigma * ny;
        (self.x, self.y)
    }

    /// Sample the next inter-sample delay (ms). Kept wide (hundreds of ms to
    /// seconds) so drift events don't drown real CDP traffic.
    pub fn next_delay_ms(&mut self) -> u64 {
        if self.step_ms_max <= self.step_ms_min {
            return self.step_ms_min;
        }
        self.rng.random_range(self.step_ms_min..self.step_ms_max)
    }
}

fn u01(rng: &mut SmallRng) -> f64 {
    (rng.random::<u32>() as f64) / (u32::MAX as f64 + 1.0)
}

fn gaussian_pair(rng: &mut SmallRng) -> (f64, f64) {
    let u1 = u01(rng).max(f64::MIN_POSITIVE);
    let u2 = u01(rng);
    let r = (-2.0 * u1.ln()).sqrt();
    let t = 2.0 * std::f64::consts::PI * u2;
    (r * t.cos(), r * t.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_profile_produces_no_drift() {
        let mut d = IdleDrift::for_profile(MotionProfile::Fast, 1);
        for _ in 0..10 {
            let (x, y) = d.next_offset();
            assert_eq!((x, y), (0.0, 0.0));
        }
    }

    #[test]
    fn balanced_drift_is_bounded_and_nonzero() {
        let mut d = IdleDrift::for_profile(MotionProfile::Balanced, 7);
        let mut max = 0.0f64;
        let mut any_nonzero = false;
        for _ in 0..500 {
            let (x, y) = d.next_offset();
            if x.abs() > 0.0 || y.abs() > 0.0 {
                any_nonzero = true;
            }
            max = max.max(x.abs()).max(y.abs());
        }
        assert!(any_nonzero, "balanced drift should produce movement");
        // OU stationary std ≈ σ/√(1-(1-θ)²). For θ=0.35, σ=0.9 → ≈ 1.1.
        // Max over 500 samples should stay well under ~10.
        assert!(max < 15.0, "drift should be bounded, got max={max}");
    }

    #[test]
    fn drift_mean_reverts_to_zero() {
        let mut d = IdleDrift::for_profile(MotionProfile::Balanced, 3);
        let mut xs = Vec::with_capacity(2_000);
        for _ in 0..2_000 {
            let (x, _y) = d.next_offset();
            xs.push(x);
        }
        let mean = xs.iter().copied().sum::<f64>() / xs.len() as f64;
        assert!(mean.abs() < 0.5, "drift should mean-revert, got {mean}");
    }

    #[test]
    fn idle_state_transitions() {
        let s = IdleState::new();
        assert!(!s.is_action_active());
        s.action_begin();
        assert!(s.is_action_active());
        s.action_end();
        assert!(!s.is_action_active());
    }

    #[test]
    fn next_delay_ms_in_window() {
        let mut d = IdleDrift::for_profile(MotionProfile::Balanced, 9);
        for _ in 0..200 {
            let ms = d.next_delay_ms();
            assert!(ms >= d.step_ms_min && ms < d.step_ms_max);
        }
    }
}
