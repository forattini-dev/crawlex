//! Scroll scheduling — bursts with bell-curve velocity + Pareto reading dwells.
//!
//! Real users don't scroll a page top-to-bottom at a constant rate. They
//! fire a short burst of wheel ticks (accelerate → peak → decelerate), pause
//! to read for a heavy-tailed dwell time, fire another burst, and so on.
//! reCAPTCHA v3 and Cloudflare ML both score on scroll velocity signatures —
//! a uniform `scrollBy(0, 1000)` is as obvious as a straight-line mouse path.
//!
//! This module is pure math: it returns a schedule of `(delta_y, delay_ms)`
//! pairs that `interact::scroll_by` walks and dispatches as CDP wheel events.
//! References: `research/evasion-deep-dive.md` §9.3 (scroll dynamics).

use rand::rngs::SmallRng;
use rand::{Rng, RngExt, SeedableRng};

use crate::render::motion::MotionProfile;

/// One wheel tick in a scheduled scroll: emit `delta_y` px of wheel delta,
/// then sleep for `delay_ms` before the next tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollTick {
    pub delta_y: f64,
    pub delay_ms: u64,
}

/// Tunables derived from the ambient `MotionProfile`. Fast profile collapses
/// to a single tick with zero dwell to preserve throughput.
#[derive(Debug, Clone, Copy)]
pub struct ScrollParams {
    /// Target ticks per burst (actual count jittered ±20%).
    pub ticks_per_burst: usize,
    /// Peak px-per-tick at the middle of the burst.
    pub peak_tick_px: f64,
    /// Per-tick delay window inside a burst (ms).
    pub tick_delay_min_ms: u64,
    pub tick_delay_max_ms: u64,
    /// Pareto xₘ (scale ms) and α (shape) for inter-burst reading dwell.
    pub dwell_scale_ms: f64,
    pub dwell_alpha: f64,
    /// Hard ceiling on a single dwell (ms) — Pareto's tail is unbounded in
    /// theory, but a 30s scroll pause means the crawler is stuck.
    pub dwell_cap_ms: u64,
    /// When true, produce a flat single-tick schedule.
    pub flat: bool,
}

impl ScrollParams {
    pub fn for_profile(p: MotionProfile) -> Self {
        match p {
            MotionProfile::Fast => ScrollParams {
                ticks_per_burst: 1,
                peak_tick_px: 240.0,
                tick_delay_min_ms: 10,
                tick_delay_max_ms: 30,
                dwell_scale_ms: 0.0,
                dwell_alpha: 2.0,
                dwell_cap_ms: 0,
                flat: true,
            },
            MotionProfile::Balanced => ScrollParams {
                ticks_per_burst: 5,
                peak_tick_px: 120.0,
                tick_delay_min_ms: 40,
                tick_delay_max_ms: 120,
                dwell_scale_ms: 500.0,
                dwell_alpha: 1.5,
                dwell_cap_ms: 6_000,
                flat: false,
            },
            MotionProfile::Human => ScrollParams {
                ticks_per_burst: 6,
                peak_tick_px: 110.0,
                tick_delay_min_ms: 60,
                tick_delay_max_ms: 160,
                dwell_scale_ms: 700.0,
                dwell_alpha: 1.4,
                dwell_cap_ms: 9_000,
                flat: false,
            },
            MotionProfile::Paranoid => ScrollParams {
                ticks_per_burst: 7,
                peak_tick_px: 90.0,
                tick_delay_min_ms: 100,
                tick_delay_max_ms: 240,
                dwell_scale_ms: 1_000.0,
                dwell_alpha: 1.3,
                dwell_cap_ms: 15_000,
                flat: false,
            },
        }
    }
}

/// Schedule scroll ticks covering a signed `dy` total delta. The returned
/// sequence alternates bursts (multiple ticks with short intra delays) with
/// reading dwells (single long `ScrollTick { delta_y: 0, delay_ms: pareto }`
/// entries). The sum of `delta_y` equals `dy` within one tick of rounding.
pub fn schedule(dy: f64, params: &ScrollParams, seed: u64) -> Vec<ScrollTick> {
    let mut rng = SmallRng::seed_from_u64(seed);
    schedule_with_rng(dy, params, &mut rng)
}

/// Process-default scroll schedule using the ambient motion profile.
pub fn schedule_for_active_profile(dy: f64) -> Vec<ScrollTick> {
    let params = ScrollParams::for_profile(MotionProfile::active());
    let mut rng = rand::make_rng::<SmallRng>();
    schedule_with_rng(dy, &params, &mut rng)
}

fn schedule_with_rng(dy: f64, params: &ScrollParams, rng: &mut SmallRng) -> Vec<ScrollTick> {
    if dy.abs() < 1.0 {
        return Vec::new();
    }

    if params.flat {
        // Single tick = legacy behaviour. Fast profile stays a no-op on
        // throughput: the caller still caps at 120px per wheel.
        let sign = dy.signum();
        let mut remaining = dy.abs();
        let mut out = Vec::new();
        while remaining > 1.0 {
            let step = remaining.min(params.peak_tick_px);
            out.push(ScrollTick {
                delta_y: sign * step,
                delay_ms: u64_range(
                    rng,
                    params.tick_delay_min_ms,
                    params.tick_delay_max_ms.max(params.tick_delay_min_ms + 1),
                ),
            });
            remaining -= step;
        }
        return out;
    }

    let sign = dy.signum();
    let mut remaining = dy.abs();
    let mut out: Vec<ScrollTick> = Vec::new();
    let mut first_burst = true;

    while remaining > 1.0 {
        // Pareto reading dwell between bursts. Skip before the very first
        // burst — a user who just clicked into a page doesn't pause first.
        if !first_burst {
            let pause = pareto(rng, params.dwell_scale_ms, params.dwell_alpha)
                .clamp(params.dwell_scale_ms * 0.2, params.dwell_cap_ms as f64)
                as u64;
            out.push(ScrollTick {
                delta_y: 0.0,
                delay_ms: pause,
            });
        }
        first_burst = false;

        // Jitter the burst length ±20% so it doesn't lock to a fixed
        // tick-count signature.
        let base = params.ticks_per_burst.max(1) as f64;
        let jitter = 0.8 + rng.random_range(0.0..0.4);
        let n = ((base * jitter).round() as usize).max(1);

        // Plan a bell-curve of per-tick deltas. Centre of burst is tallest.
        // Use a triangular kernel — quick to compute and visually similar
        // to real wheel bursts we profiled.
        let peak = params.peak_tick_px;
        let weights: Vec<f64> = (0..n)
            .map(|i| {
                let t = (i as f64 + 0.5) / n as f64;
                let tri = 1.0 - (2.0 * t - 1.0).abs();
                (tri * peak).max(peak * 0.25)
            })
            .collect();
        let planned: f64 = weights.iter().sum();
        let burst_budget = remaining.min(planned).max(1.0);
        let scale = burst_budget / planned;

        for w in &weights {
            let mag = (w * scale).min(remaining);
            if mag < 1.0 {
                break;
            }
            let delay = u64_range(
                rng,
                params.tick_delay_min_ms,
                params.tick_delay_max_ms.max(params.tick_delay_min_ms + 1),
            );
            out.push(ScrollTick {
                delta_y: sign * mag,
                delay_ms: delay,
            });
            remaining -= mag;
        }
    }
    out
}

fn u64_range(rng: &mut SmallRng, lo: u64, hi: u64) -> u64 {
    if hi <= lo {
        return lo;
    }
    rng.random_range(lo..hi)
}

fn pareto(rng: &mut SmallRng, scale: f64, alpha: f64) -> f64 {
    let u = u01(rng).clamp(1e-6, 1.0 - 1e-6);
    scale * (1.0 - u).powf(-1.0 / alpha.max(0.1))
}

fn u01(rng: &mut SmallRng) -> f64 {
    (rng.next_u32() as f64) / (u32::MAX as f64 + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_profile_is_flat_no_dwells() {
        let p = ScrollParams::for_profile(MotionProfile::Fast);
        let ticks = schedule(500.0, &p, 1);
        assert!(!ticks.is_empty());
        for t in &ticks {
            assert!(
                t.delta_y.abs() > 0.5,
                "fast profile should have no zero-delta dwells"
            );
        }
    }

    #[test]
    fn balanced_schedule_sum_matches_dy() {
        let p = ScrollParams::for_profile(MotionProfile::Balanced);
        let dy = 1200.0;
        let ticks = schedule(dy, &p, 42);
        let sum: f64 = ticks.iter().map(|t| t.delta_y).sum();
        assert!((sum - dy).abs() <= p.peak_tick_px, "sum={sum} dy={dy}");
    }

    #[test]
    fn balanced_schedule_has_dwells_between_bursts() {
        let p = ScrollParams::for_profile(MotionProfile::Balanced);
        let ticks = schedule(2_000.0, &p, 7);
        let dwells = ticks.iter().filter(|t| t.delta_y == 0.0).count();
        assert!(dwells >= 1, "expected at least one inter-burst dwell");
    }

    #[test]
    fn determinism_with_seed() {
        let p = ScrollParams::for_profile(MotionProfile::Balanced);
        let a = schedule(800.0, &p, 99);
        let b = schedule(800.0, &p, 99);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.delay_ms, y.delay_ms);
            assert!((x.delta_y - y.delta_y).abs() < 1e-9);
        }
    }

    #[test]
    fn burst_has_bell_curve_velocity() {
        let p = ScrollParams::for_profile(MotionProfile::Balanced);
        let ticks = schedule(500.0, &p, 11);
        let burst: Vec<f64> = ticks
            .iter()
            .take_while(|t| t.delta_y != 0.0)
            .map(|t| t.delta_y.abs())
            .collect();
        if burst.len() >= 3 {
            let mid = burst[burst.len() / 2];
            let edge = burst[0].min(*burst.last().unwrap());
            assert!(
                mid >= edge,
                "burst middle should peak: mid={mid}, edge={edge}"
            );
        }
    }

    #[test]
    fn negative_dy_goes_up() {
        let p = ScrollParams::for_profile(MotionProfile::Balanced);
        let ticks = schedule(-500.0, &p, 3);
        for t in &ticks {
            assert!(
                t.delta_y <= 0.0,
                "upward scroll should emit non-positive deltas"
            );
        }
    }

    #[test]
    fn tiny_dy_yields_no_schedule() {
        let p = ScrollParams::for_profile(MotionProfile::Balanced);
        let ticks = schedule(0.4, &p, 1);
        assert!(ticks.is_empty());
    }
}
