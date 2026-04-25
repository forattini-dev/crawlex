//! Submovement decomposition — primary → overshoot → correction.
//!
//! Fitts' Law paths aren't single ballistic sweeps: they decompose into
//! discrete submovements. The classical 3-phase model (Meyer 1988,
//! Elliott 2001) has a large ballistic "primary" carrying ~70% of the
//! distance, an "overshoot" that slightly passes the target, and a small
//! "correction" retracting back. reCAPTCHA v3 and perimeterx BOTH watch
//! for this signature — an unbroken single-segment WindMouse curve looks
//! younger than a population mean.
//!
//! `SubmovementPlan` decomposes a target offset into three `(ratio, width)`
//! sub-targets the caller can feed sequentially through `MotionEngine::
//! trajectory`. Each call lands on its own point; the concatenation
//! produces the population-average 3-phase signature.

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

use crate::render::motion::MotionProfile;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubmovementPhase {
    /// Fraction of the full straight-line distance this phase covers
    /// (signed — the correction phase is negative if it retracts).
    pub fraction: f64,
    /// Target width (px) for the Fitts MT calc at this phase. Phases with
    /// small `width` pay a higher MT, matching the long homing tail.
    pub target_width: f64,
    /// Post-phase sleep (ms) before starting the next phase. Non-zero on
    /// primary→overshoot so Chrome flushes the first batch of mousemove
    /// events before the correction starts.
    pub post_delay_ms: u64,
}

/// Profile-aware decomposition knobs.
#[derive(Debug, Clone, Copy)]
pub struct SubmovementParams {
    /// Probability the 3-phase plan fires (otherwise a single 100% phase is
    /// returned). Fast = 0.
    pub probability: f64,
    /// Fraction covered by the primary ballistic phase (0.5–0.85).
    pub primary_fraction: f64,
    /// Extra fraction past the target carried by overshoot (0–0.15).
    pub overshoot_fraction: f64,
    /// Target-width multiplier for the correction phase (< 1 makes MT rise).
    pub correction_width_scale: f64,
    pub inter_phase_delay_ms_min: u64,
    pub inter_phase_delay_ms_max: u64,
}

impl SubmovementParams {
    pub fn for_profile(p: MotionProfile) -> Self {
        match p {
            MotionProfile::Fast => SubmovementParams {
                probability: 0.0,
                primary_fraction: 1.0,
                overshoot_fraction: 0.0,
                correction_width_scale: 1.0,
                inter_phase_delay_ms_min: 0,
                inter_phase_delay_ms_max: 0,
            },
            MotionProfile::Balanced => SubmovementParams {
                probability: 0.35,
                primary_fraction: 0.72,
                overshoot_fraction: 0.06,
                correction_width_scale: 0.4,
                inter_phase_delay_ms_min: 15,
                inter_phase_delay_ms_max: 50,
            },
            MotionProfile::Human => SubmovementParams {
                probability: 0.55,
                primary_fraction: 0.7,
                overshoot_fraction: 0.09,
                correction_width_scale: 0.35,
                inter_phase_delay_ms_min: 25,
                inter_phase_delay_ms_max: 80,
            },
            MotionProfile::Paranoid => SubmovementParams {
                probability: 0.75,
                primary_fraction: 0.68,
                overshoot_fraction: 0.12,
                correction_width_scale: 0.3,
                inter_phase_delay_ms_min: 40,
                inter_phase_delay_ms_max: 120,
            },
        }
    }
}

/// Decompose a move into a sequence of phases.
pub fn plan(params: &SubmovementParams, target_width_px: f64, seed: u64) -> Vec<SubmovementPhase> {
    let mut rng = SmallRng::seed_from_u64(seed);
    plan_with_rng(params, target_width_px, &mut rng)
}

fn plan_with_rng(
    params: &SubmovementParams,
    target_width_px: f64,
    rng: &mut SmallRng,
) -> Vec<SubmovementPhase> {
    let single = vec![SubmovementPhase {
        fraction: 1.0,
        target_width: target_width_px.max(4.0),
        post_delay_ms: 0,
    }];
    if params.probability <= 0.0 {
        return single;
    }
    if u01(rng) >= params.probability {
        return single;
    }

    // Jitter the fractions a touch so we don't produce an identical
    // 0.72 / 1.06 / -0.06 triple every time.
    let mut jitter = |base: f64, span: f64| base + (u01(rng) * 2.0 - 1.0) * span;
    let primary_frac = jitter(params.primary_fraction, 0.05).clamp(0.55, 0.9);
    let overshoot_frac = jitter(params.overshoot_fraction, 0.02).clamp(0.02, 0.18);
    // The three fractions here are *cumulative destination* fractions —
    // i.e. position reached after each phase relative to total distance.
    // Phase 1: primary. Phase 2: overshoot peak (slightly past target).
    // Phase 3: correction (back onto target exactly).
    let p1 = primary_frac;
    let p2 = 1.0 + overshoot_frac;
    let p3 = 1.0;

    let delay_lo = params.inter_phase_delay_ms_min;
    let delay_hi = params.inter_phase_delay_ms_max.max(delay_lo + 1);
    let d1 = u64_range(rng, delay_lo, delay_hi);
    let d2 = u64_range(rng, delay_lo, delay_hi);

    vec![
        SubmovementPhase {
            fraction: p1,
            target_width: target_width_px.max(8.0),
            post_delay_ms: d1,
        },
        SubmovementPhase {
            fraction: p2,
            target_width: target_width_px.max(8.0),
            post_delay_ms: d2,
        },
        SubmovementPhase {
            fraction: p3,
            target_width: (target_width_px * params.correction_width_scale).max(4.0),
            post_delay_ms: 0,
        },
    ]
}

fn u01(rng: &mut SmallRng) -> f64 {
    (rng.random::<u32>() as f64) / (u32::MAX as f64 + 1.0)
}

fn u64_range(rng: &mut SmallRng, lo: u64, hi: u64) -> u64 {
    if hi <= lo {
        return lo;
    }
    rng.random_range(lo..hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_profile_is_single_phase() {
        let p = SubmovementParams::for_profile(MotionProfile::Fast);
        let plan = plan(&p, 40.0, 1);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].fraction, 1.0);
    }

    #[test]
    fn three_phase_fractions_land_on_target() {
        let p = SubmovementParams::for_profile(MotionProfile::Human);
        // Force the probability branch by retrying seeds until we get a 3-phase plan.
        let mut got_three = None;
        for seed in 0..50 {
            let pp = plan(&p, 40.0, seed);
            if pp.len() == 3 {
                got_three = Some(pp);
                break;
            }
        }
        let pp = got_three.expect("should eventually produce a 3-phase plan");
        assert!(pp[0].fraction > 0.5 && pp[0].fraction < 0.95);
        assert!(pp[1].fraction > 1.0);
        assert!((pp[2].fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn probability_tail_returns_single() {
        let p = SubmovementParams::for_profile(MotionProfile::Balanced);
        let mut singles = 0usize;
        let n = 500;
        for seed in 0..n {
            if plan(&p, 40.0, seed).len() == 1 {
                singles += 1;
            }
        }
        let frac_single = singles as f64 / n as f64;
        let expected_single = 1.0 - p.probability;
        assert!(
            (frac_single - expected_single).abs() < 0.1,
            "expected ~{expected_single} single-phase plans, got {frac_single}"
        );
    }

    #[test]
    fn correction_phase_uses_narrower_target() {
        let p = SubmovementParams::for_profile(MotionProfile::Human);
        for seed in 0..50 {
            let pp = plan(&p, 50.0, seed);
            if pp.len() == 3 {
                assert!(pp[2].target_width < pp[0].target_width);
                return;
            }
        }
    }
}
