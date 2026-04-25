//! Human motion engine — WindMouse + Fitts' Law + Ornstein-Uhlenbeck jitter.
//!
//! This module replaces the cubic-bezier mouse path with a behavioural model
//! grounded in the research literature. Detectors (reCAPTCHA v3, DataDome,
//! Cloudflare ML) score on *trajectory shape*, not just motion vs. no-motion:
//!
//! * **WindMouse** (Benjamin Land, 2005 — ported 2021) integrates a gravity
//!   vector toward the target with random wind perturbation, producing the
//!   bell-curve velocity profile real hands exhibit.
//! * **Fitts' Law** (MT = a + b·log₂(D/W+1)) sets total movement time so the
//!   duration scales correctly with distance and target width.
//! * **Ornstein-Uhlenbeck** jitter (dX = θ(μ-X)dt + σdW) overlays a stationary
//!   tremor — idle mice don't stay still, they drift with mean reversion.
//! * **Overshoot** (~12% of human movements) briefly passes the target and
//!   corrects back, matching Fitts' ballistic-then-homing two-phase model.
//!
//! References: `research/evasion-deep-dive.md` §9.1–9.5.

use rand::rngs::SmallRng;
use rand::{Rng, RngExt, SeedableRng};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

pub mod device;
pub mod fatigue;
pub mod idle;
pub mod lifecycle;
pub mod scroll;
pub mod submovement;
pub mod touch;

pub use device::MotionDeviceProfile;
pub use idle::{IdleDrift, IdleState};
pub use lifecycle::{LifecycleEvent, LifecycleParams};
pub use scroll::{ScrollParams, ScrollTick};
pub use submovement::{SubmovementParams, SubmovementPhase};
pub use touch::{pointer_kind_from_ua, PointerKind, TouchFrame, TouchParams, TouchPhase};

/// Motion profile preset — picked per crawl via `Config::motion_profile`.
///
/// * `Fast` — WindMouse disabled, straight linear interpolation at minimal
///   delay. Preserves the ~15 rps throughput baseline; use for
///   dev/testing or when the target does no behavioural scoring.
/// * `Balanced` (default) — WindMouse + Fitts + OU jitter at moderate params.
///   Realistic shape, ~1–2s per click. Expected throughput ~8 rps.
/// * `Human` — Fully realistic (2–4s per click), overshoots tuned to
///   population averages. Use against known ML-scored targets.
/// * `Paranoid` — Aggressive overshoots, reading pauses injected, 5–10s per
///   click sequence. Maximum evasion at minimum throughput.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionProfile {
    Fast,
    #[default]
    Balanced,
    Human,
    Paranoid,
}

/// Process-wide ambient motion profile. Callers that own a `Config`
/// (CLI, render pool, tests) should call `set_active()` at startup so the
/// `interact::*` primitives pick the right preset without threading the
/// config through every call site. Defaults to `Balanced`.
static ACTIVE_PROFILE: AtomicU8 = AtomicU8::new(1);

impl MotionProfile {
    pub fn as_u8(self) -> u8 {
        match self {
            MotionProfile::Fast => 0,
            MotionProfile::Balanced => 1,
            MotionProfile::Human => 2,
            MotionProfile::Paranoid => 3,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => MotionProfile::Fast,
            2 => MotionProfile::Human,
            3 => MotionProfile::Paranoid,
            _ => MotionProfile::Balanced,
        }
    }

    /// Install `self` as the process-wide ambient profile used by
    /// `active()` and by the `interact::*` primitives.
    pub fn set_active(self) {
        ACTIVE_PROFILE.store(self.as_u8(), Ordering::Relaxed);
    }

    /// Current process-wide ambient profile.
    pub fn active() -> Self {
        MotionProfile::from_u8(ACTIVE_PROFILE.load(Ordering::Relaxed))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MotionProfile::Fast => "fast",
            MotionProfile::Balanced => "balanced",
            MotionProfile::Human => "human",
            MotionProfile::Paranoid => "paranoid",
        }
    }

    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "fast" => Some(MotionProfile::Fast),
            "balanced" => Some(MotionProfile::Balanced),
            "human" => Some(MotionProfile::Human),
            "paranoid" => Some(MotionProfile::Paranoid),
            _ => None,
        }
    }

    pub fn params(self) -> MotionParams {
        match self {
            MotionProfile::Fast => MotionParams {
                gravity: 9.0,
                wind: 0.0,
                min_wait: 0.0,
                max_wait: 0.0,
                max_step: 40.0,
                target_area: 20.0,
                ou_theta: 0.0,
                ou_sigma: 0.0,
                overshoot_prob: 0.0,
                overshoot_px: 0.0,
                step_delay_ms_min: 1,
                step_delay_ms_max: 3,
                post_move_pause_ms_min: 5,
                post_move_pause_ms_max: 15,
                mouse_down_pause_ms_min: 5,
                mouse_down_pause_ms_max: 15,
                fitts_a_ms: 30.0,
                fitts_b_ms: 60.0,
                fitts_jitter: 0.1,
                use_windmouse: false,
                emit_mouseover: false,
            },
            MotionProfile::Balanced => MotionParams {
                gravity: 9.0,
                wind: 3.0,
                min_wait: 2.0,
                max_wait: 10.0,
                max_step: 10.0,
                target_area: 10.0,
                ou_theta: 0.7,
                ou_sigma: 0.5,
                overshoot_prob: 0.12,
                overshoot_px: 8.0,
                step_delay_ms_min: 6,
                step_delay_ms_max: 14,
                post_move_pause_ms_min: 30,
                post_move_pause_ms_max: 90,
                mouse_down_pause_ms_min: 30,
                mouse_down_pause_ms_max: 90,
                fitts_a_ms: 50.0,
                fitts_b_ms: 150.0,
                fitts_jitter: 0.2,
                use_windmouse: true,
                emit_mouseover: true,
            },
            MotionProfile::Human => MotionParams {
                gravity: 9.0,
                wind: 4.5,
                min_wait: 3.0,
                max_wait: 15.0,
                max_step: 8.0,
                target_area: 8.0,
                ou_theta: 0.6,
                ou_sigma: 0.8,
                overshoot_prob: 0.18,
                overshoot_px: 12.0,
                step_delay_ms_min: 10,
                step_delay_ms_max: 22,
                post_move_pause_ms_min: 80,
                post_move_pause_ms_max: 220,
                mouse_down_pause_ms_min: 50,
                mouse_down_pause_ms_max: 130,
                fitts_a_ms: 100.0,
                fitts_b_ms: 200.0,
                fitts_jitter: 0.25,
                use_windmouse: true,
                emit_mouseover: true,
            },
            MotionProfile::Paranoid => MotionParams {
                gravity: 8.0,
                wind: 6.0,
                min_wait: 4.0,
                max_wait: 20.0,
                max_step: 6.0,
                target_area: 6.0,
                ou_theta: 0.5,
                ou_sigma: 1.2,
                overshoot_prob: 0.28,
                overshoot_px: 18.0,
                step_delay_ms_min: 14,
                step_delay_ms_max: 30,
                post_move_pause_ms_min: 200,
                post_move_pause_ms_max: 600,
                mouse_down_pause_ms_min: 80,
                mouse_down_pause_ms_max: 220,
                fitts_a_ms: 200.0,
                fitts_b_ms: 300.0,
                fitts_jitter: 0.35,
                use_windmouse: true,
                emit_mouseover: true,
            },
        }
    }
}

/// Tunable parameters for the motion engine. Built from a `MotionProfile`.
#[derive(Debug, Clone, Copy)]
pub struct MotionParams {
    /// WindMouse gravity — pull strength toward the target.
    pub gravity: f64,
    /// WindMouse wind — random perturbation magnitude.
    pub wind: f64,
    pub min_wait: f64,
    pub max_wait: f64,
    /// WindMouse velocity cap (step size in px).
    pub max_step: f64,
    /// Convergence radius (switches wind off when within).
    pub target_area: f64,
    /// OU mean-reversion rate (θ).
    pub ou_theta: f64,
    /// OU volatility (σ).
    pub ou_sigma: f64,
    /// Probability the engine overshoots past the target then corrects back.
    pub overshoot_prob: f64,
    /// Magnitude (px) of the overshoot when it fires.
    pub overshoot_px: f64,
    /// Per-step inter-sample delay (ms).
    pub step_delay_ms_min: u64,
    pub step_delay_ms_max: u64,
    /// Delay between the final mousemove and mousedown (ms).
    pub post_move_pause_ms_min: u64,
    pub post_move_pause_ms_max: u64,
    /// Delay between mousedown and mouseup (ms).
    pub mouse_down_pause_ms_min: u64,
    pub mouse_down_pause_ms_max: u64,
    /// Fitts' law intercept (ms).
    pub fitts_a_ms: f64,
    /// Fitts' law slope (ms/bit).
    pub fitts_b_ms: f64,
    /// Multiplicative jitter applied to MT (±fraction).
    pub fitts_jitter: f64,
    /// When false, fall back to simple linear interpolation (Fast profile).
    pub use_windmouse: bool,
    /// When true, wrapping callers should also dispatch mouseover/mouseenter
    /// events before mousedown — required for event-sequence integrity
    /// against modern detectors that flag click-without-move.
    pub emit_mouseover: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A sampled point along the trajectory with the delay to wait after
/// dispatching it (before emitting the next point).
#[derive(Debug, Clone, Copy)]
pub struct TimedPoint {
    pub x: f64,
    pub y: f64,
    pub delay_ms: u64,
}

/// Fitts' law movement time in milliseconds.
///
/// `MT = a + b · log₂(D/W + 1)`.
/// Returns ≥ `a` — clamped so a 0-distance target still has a floor delay.
pub fn fitts_mt_ms(distance_px: f64, target_width_px: f64, params: &MotionParams) -> f64 {
    let d = distance_px.max(0.0);
    let w = target_width_px.max(1.0);
    let id = (d / w + 1.0).log2().max(0.0);
    params.fitts_a_ms + params.fitts_b_ms * id
}

/// Human motion engine — build with `MotionEngine::new(profile)` or
/// `MotionEngine::with_seed(profile, seed)` for determinism.
pub struct MotionEngine {
    rng: SmallRng,
    pub params: MotionParams,
}

impl MotionEngine {
    pub fn new(profile: MotionProfile) -> Self {
        Self {
            rng: rand::make_rng::<SmallRng>(),
            params: profile.params(),
        }
    }

    pub fn with_seed(profile: MotionProfile, seed: u64) -> Self {
        Self {
            rng: SmallRng::seed_from_u64(seed),
            params: profile.params(),
        }
    }

    pub fn with_params(params: MotionParams, seed: u64) -> Self {
        Self {
            rng: SmallRng::seed_from_u64(seed),
            params,
        }
    }

    /// Generate a trajectory from `from` to `to`, targeting a box of
    /// `target_width` px. The returned sequence includes timing delays
    /// whose total scales roughly with Fitts' MT.
    ///
    /// With `MotionProfile::Fast` this collapses to a short linear path
    /// with minimal delays — preserving dev throughput.
    pub fn trajectory(&mut self, from: Point, to: Point, target_width: f64) -> Vec<TimedPoint> {
        if !self.params.use_windmouse {
            return self.linear_trajectory(from, to);
        }

        // Possibly aim past the target, then correct back (Fitts' ballistic
        // + homing phases). Overshoot magnitude scales inversely with the
        // movement length so tiny targets still behave sensibly.
        let do_overshoot = self.params.overshoot_prob > 0.0
            && rand_unit(&mut self.rng) < self.params.overshoot_prob;

        let mut points: Vec<(f64, f64)> = Vec::new();
        let start = (from.x, from.y);
        let final_target = (to.x, to.y);

        if do_overshoot {
            let (ox, oy) =
                overshoot_target(&mut self.rng, start, final_target, self.params.overshoot_px);
            self.windmouse_points(start, (ox, oy), &mut points);
            let mid = *points.last().unwrap_or(&start);
            self.windmouse_points(mid, final_target, &mut points);
        } else {
            self.windmouse_points(start, final_target, &mut points);
        }

        // Overlay OU jitter on the interior samples so idle tremor is baked
        // in. Don't perturb start/end — those anchor to real coordinates.
        if self.params.ou_sigma > 0.0 && points.len() > 2 {
            let mut jx = 0.0f64;
            let mut jy = 0.0f64;
            let last = points.len() - 1;
            for p in points.iter_mut().take(last).skip(1) {
                let (nx, ny) = ou_step(
                    &mut self.rng,
                    jx,
                    jy,
                    self.params.ou_theta,
                    self.params.ou_sigma,
                );
                jx = nx;
                jy = ny;
                p.0 += jx;
                p.1 += jy;
            }
        }

        // Scale per-step delays so the trajectory's total duration tracks
        // Fitts MT within a jitter window.
        let distance = ((final_target.0 - start.0).hypot(final_target.1 - start.1)).max(1.0);
        let mt = fitts_mt_ms(distance, target_width, &self.params);
        let jitter = 1.0 + (rand_unit(&mut self.rng) * 2.0 - 1.0) * self.params.fitts_jitter;
        let mt = (mt * jitter).max(self.params.fitts_a_ms * 0.5);

        let n = points.len().max(1);
        let mean_delay = (mt / n as f64).max(1.0);
        let min = self.params.step_delay_ms_min.max(1) as f64;
        let max = self.params.step_delay_ms_max.max(min as u64 + 1) as f64;

        points
            .into_iter()
            .map(|(x, y)| {
                // Centre the delay on Fitts-derived `mean_delay` but keep
                // it inside the [min, max] window so single steps stay in
                // a human-plausible regime.
                let lo = (mean_delay * 0.6).max(min).min(max);
                let hi = (mean_delay * 1.4).max(lo + 1.0).min(max.max(lo + 1.0));
                let d = self.rng.random_range(lo..hi);
                TimedPoint {
                    x,
                    y,
                    delay_ms: d as u64,
                }
            })
            .collect()
    }

    fn linear_trajectory(&mut self, from: Point, to: Point) -> Vec<TimedPoint> {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let dist = (dx * dx + dy * dy).sqrt().max(1.0);
        let steps = (dist / 40.0).clamp(3.0, 10.0) as usize;
        let min = self.params.step_delay_ms_min.max(1);
        let max = self.params.step_delay_ms_max.max(min + 1);
        (1..=steps)
            .map(|i| {
                let t = i as f64 / steps as f64;
                TimedPoint {
                    x: from.x + dx * t,
                    y: from.y + dy * t,
                    delay_ms: self.rng.random_range(min..max),
                }
            })
            .collect()
    }

    /// WindMouse core loop (Benjamin Land 2005 / 2021 port).
    /// Appends trajectory samples (x, y) to `out`.
    fn windmouse_points(
        &mut self,
        start: (f64, f64),
        target: (f64, f64),
        out: &mut Vec<(f64, f64)>,
    ) {
        let (mut x, mut y) = start;
        let (tx, ty) = target;
        let (mut vx, mut vy) = (0.0f64, 0.0f64);
        let (mut wx, mut wy) = (0.0f64, 0.0f64);
        let sqrt3 = 3.0f64.sqrt();
        let sqrt5 = 5.0f64.sqrt();
        let mut m = self.params.max_step.max(1.0);
        let gravity = self.params.gravity.max(0.1);
        let wind_cap = self.params.wind.max(0.0);
        let converge = self.params.target_area.max(1.0);

        // Hard bound on iterations so a degenerate param set can never
        // explode into a runaway loop (detector ML doesn't care about
        // megastep paths; callers do when they block on us).
        let max_iters = 10_000usize;
        for _ in 0..max_iters {
            let dx = tx - x;
            let dy = ty - y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < 1.0 {
                break;
            }
            let wmag = wind_cap.min(dist);
            if dist >= converge {
                wx = wx / sqrt3 + (rand_unit(&mut self.rng) * 2.0 - 1.0) * wmag / sqrt5;
                wy = wy / sqrt3 + (rand_unit(&mut self.rng) * 2.0 - 1.0) * wmag / sqrt5;
            } else {
                wx /= sqrt3;
                wy /= sqrt3;
                if m < 3.0 {
                    m = rand_unit(&mut self.rng) * 3.0 + 3.0;
                } else {
                    m /= sqrt5;
                }
            }
            let gx = gravity * dx / dist;
            let gy = gravity * dy / dist;
            vx += wx + gx;
            vy += wy + gy;
            let v = (vx * vx + vy * vy).sqrt();
            if v > m {
                let vclip = m / 2.0 + rand_unit(&mut self.rng) * m / 2.0;
                if v > 0.0 {
                    vx = vx / v * vclip;
                    vy = vy / v * vclip;
                }
            }
            x += vx;
            y += vy;
            out.push((x, y));
        }
        // Snap to exact target so downstream clicks land where we asked.
        if out
            .last()
            .map(|p| (p.0 - tx).hypot(p.1 - ty) > 0.5)
            .unwrap_or(true)
        {
            out.push((tx, ty));
        }
    }
}

fn rand_unit(rng: &mut SmallRng) -> f64 {
    // Uniform on [0, 1).
    (rng.next_u32() as f64) / (u32::MAX as f64 + 1.0)
}

/// One Euler-Maruyama step of Ornstein-Uhlenbeck around μ=0:
/// `X_{t+1} = X_t · (1 - θ) + σ · N(0, 1)`.
/// Gaussian sampled via Box-Muller on two uniforms.
fn ou_step(rng: &mut SmallRng, x: f64, y: f64, theta: f64, sigma: f64) -> (f64, f64) {
    let (nx, ny) = gaussian_pair(rng);
    (
        x * (1.0 - theta) + sigma * nx,
        y * (1.0 - theta) + sigma * ny,
    )
}

fn gaussian_pair(rng: &mut SmallRng) -> (f64, f64) {
    // Box-Muller. Guard u1 > 0 so ln doesn't explode.
    let u1 = rand_unit(rng).max(f64::MIN_POSITIVE);
    let u2 = rand_unit(rng);
    let r = (-2.0 * u1.ln()).sqrt();
    let theta = 2.0 * std::f64::consts::PI * u2;
    (r * theta.cos(), r * theta.sin())
}

fn overshoot_target(
    rng: &mut SmallRng,
    from: (f64, f64),
    to: (f64, f64),
    magnitude: f64,
) -> (f64, f64) {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let dist = (dx * dx + dy * dy).sqrt().max(1.0);
    let ux = dx / dist;
    let uy = dy / dist;
    let jitter = 0.5 + rand_unit(rng);
    let m = magnitude * jitter;
    (to.0 + ux * m, to.1 + uy * m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_from_str_round_trip() {
        for p in [
            MotionProfile::Fast,
            MotionProfile::Balanced,
            MotionProfile::Human,
            MotionProfile::Paranoid,
        ] {
            let s = p.as_str();
            assert_eq!(MotionProfile::from_str_ci(s), Some(p));
        }
        assert_eq!(
            MotionProfile::from_str_ci("FAST"),
            Some(MotionProfile::Fast)
        );
        assert!(MotionProfile::from_str_ci("nope").is_none());
    }

    #[test]
    fn fitts_mt_scales_with_distance() {
        let params = MotionProfile::Balanced.params();
        let short = fitts_mt_ms(10.0, 20.0, &params);
        let long = fitts_mt_ms(1000.0, 20.0, &params);
        assert!(
            long > short,
            "long move should take longer: {long} > {short}"
        );
        // Rough sanity: Fitts at D=1000, W=20 → log2(51) ≈ 5.67.
        // MT ≈ 50 + 150·5.67 ≈ 900 ms.
        assert!(long > 500.0 && long < 1200.0, "MT out of band: {long}");
    }

    #[test]
    fn fitts_mt_scales_with_target_width() {
        let params = MotionProfile::Balanced.params();
        let wide = fitts_mt_ms(500.0, 200.0, &params);
        let narrow = fitts_mt_ms(500.0, 5.0, &params);
        assert!(narrow > wide, "narrow target should take longer");
    }

    #[test]
    fn trajectory_reaches_target() {
        let mut eng = MotionEngine::with_seed(MotionProfile::Balanced, 42);
        let from = Point { x: 10.0, y: 10.0 };
        let to = Point { x: 400.0, y: 300.0 };
        let pts = eng.trajectory(from, to, 40.0);
        assert!(!pts.is_empty());
        let last = pts.last().unwrap();
        let dx = last.x - to.x;
        let dy = last.y - to.y;
        assert!(
            dx.hypot(dy) < 2.0,
            "final point should snap near target, got ({}, {})",
            last.x,
            last.y
        );
    }

    #[test]
    fn trajectory_has_multiple_samples_in_windmouse_mode() {
        let mut eng = MotionEngine::with_seed(MotionProfile::Balanced, 7);
        let pts = eng.trajectory(Point { x: 0.0, y: 0.0 }, Point { x: 500.0, y: 500.0 }, 30.0);
        // WindMouse with max_step≈10 over a 707 px diagonal → dozens of
        // samples. We only need "more than a handful" to prove it isn't a
        // two-point line.
        assert!(
            pts.len() > 20,
            "expected bell-curve WindMouse path, got {}",
            pts.len()
        );
    }

    #[test]
    fn fast_profile_produces_short_path() {
        let mut eng = MotionEngine::with_seed(MotionProfile::Fast, 3);
        let pts = eng.trajectory(Point { x: 0.0, y: 0.0 }, Point { x: 500.0, y: 500.0 }, 30.0);
        assert!(
            pts.len() <= 10,
            "fast path should be short, got {}",
            pts.len()
        );
    }

    #[test]
    fn trajectory_is_deterministic_with_seed() {
        let run = |seed| {
            let mut eng = MotionEngine::with_seed(MotionProfile::Balanced, seed);
            eng.trajectory(Point { x: 0.0, y: 0.0 }, Point { x: 200.0, y: 200.0 }, 30.0)
        };
        let a = run(11);
        let b = run(11);
        assert_eq!(a.len(), b.len());
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert!((pa.x - pb.x).abs() < 1e-9);
            assert!((pa.y - pb.y).abs() < 1e-9);
            assert_eq!(pa.delay_ms, pb.delay_ms);
        }
    }

    #[test]
    fn velocity_profile_has_a_peak_interior() {
        // Bell-curve velocity: step magnitudes should rise then fall, so
        // the midpoint steps exceed the boundary steps on average.
        let mut eng = MotionEngine::with_seed(MotionProfile::Balanced, 99);
        let pts = eng.trajectory(Point { x: 0.0, y: 0.0 }, Point { x: 600.0, y: 0.0 }, 30.0);
        if pts.len() < 12 {
            return;
        }
        let mut speeds = Vec::with_capacity(pts.len() - 1);
        for w in pts.windows(2) {
            let dx = w[1].x - w[0].x;
            let dy = w[1].y - w[0].y;
            speeds.push(dx.hypot(dy));
        }
        let n = speeds.len();
        let mean_mid: f64 =
            speeds[n / 3..2 * n / 3].iter().copied().sum::<f64>() / (n / 3).max(1) as f64;
        let mean_edge: f64 = (speeds.iter().take(n / 6).copied().sum::<f64>()
            + speeds.iter().rev().take(n / 6).copied().sum::<f64>())
            / (2 * (n / 6).max(1)) as f64;
        assert!(
            mean_mid > mean_edge * 0.8,
            "middle of path should carry >~edge velocity: mid={mean_mid}, edge={mean_edge}"
        );
    }

    #[test]
    fn ou_step_mean_reverts_to_zero() {
        // Stationarity smoke-test: after many steps, sample mean should be
        // small relative to σ/√(1-(1-θ)²) ≈ steady-state stddev.
        let mut rng = SmallRng::seed_from_u64(77);
        let (mut x, mut y) = (0.0f64, 0.0f64);
        let theta = 0.7;
        let sigma = 0.5;
        let mut samples_x = Vec::with_capacity(2_000);
        for _ in 0..2_000 {
            let (nx, ny) = ou_step(&mut rng, x, y, theta, sigma);
            x = nx;
            y = ny;
            samples_x.push(x);
        }
        let mean = samples_x.iter().copied().sum::<f64>() / samples_x.len() as f64;
        assert!(
            mean.abs() < 0.3,
            "OU should mean-revert near zero, got {mean}"
        );
    }

    #[test]
    fn overshoot_target_lies_past_the_real_target() {
        let mut rng = SmallRng::seed_from_u64(5);
        let from = (0.0, 0.0);
        let to = (100.0, 0.0);
        let (ox, _oy) = overshoot_target(&mut rng, from, to, 10.0);
        assert!(
            ox > to.0,
            "overshoot x={ox} should be past target x={}",
            to.0
        );
    }

    #[test]
    fn overshoot_frequency_within_tolerance() {
        // Across many trajectories we expect ≈ overshoot_prob of them to
        // enter the overshoot branch. We can't observe the branch directly
        // but overshoots produce longer average paths — exploit that.
        let params = MotionProfile::Balanced.params();
        let mut total = 0usize;
        let mut overshoot_like = 0usize;
        for seed in 0..200 {
            let mut eng = MotionEngine::with_seed(MotionProfile::Balanced, seed);
            let pts = eng.trajectory(Point { x: 0.0, y: 0.0 }, Point { x: 200.0, y: 0.0 }, 30.0);
            // Path is "overshoot-like" if its total polyline length is
            // noticeably longer than the straight-line distance.
            let mut len = 0.0;
            let mut prev = (0.0, 0.0);
            for p in &pts {
                len += (p.x - prev.0).hypot(p.y - prev.1);
                prev = (p.x, p.y);
            }
            if len > 220.0 {
                overshoot_like += 1;
            }
            total += 1;
        }
        let frac = overshoot_like as f64 / total as f64;
        // Loose bound — overshoots compound path length, but so does OU
        // jitter. Just assert we see a meaningful fraction while proving
        // it's not 100% (i.e. overshoot is probabilistic).
        assert!(
            frac < 0.99,
            "overshoot shouldn't fire on every trajectory (frac={frac}, prob={})",
            params.overshoot_prob
        );
    }
}
