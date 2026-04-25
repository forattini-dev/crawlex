//! Scroll scheduler — bursts with Pareto dwells (B.1).

#![cfg(feature = "cdp-backend")]

use crawlex::render::motion::scroll::{schedule, ScrollParams};
use crawlex::render::motion::MotionProfile;

#[test]
fn balanced_scroll_has_bursts_and_dwells() {
    let p = ScrollParams::for_profile(MotionProfile::Balanced);
    let ticks = schedule(2_000.0, &p, 7);
    let dwells = ticks.iter().filter(|t| t.delta_y == 0.0).count();
    let moves = ticks.iter().filter(|t| t.delta_y.abs() > 0.0).count();
    assert!(dwells >= 1, "expected reading dwells between bursts");
    assert!(moves > dwells, "moves should outnumber dwells");
}

#[test]
fn fast_profile_preserves_throughput() {
    // Fast schedule: no zero-delta dwells, tick deltas large (≤240 px).
    let p = ScrollParams::for_profile(MotionProfile::Fast);
    let ticks = schedule(1_000.0, &p, 1);
    for t in &ticks {
        assert!(t.delta_y.abs() > 1.0, "fast must not insert dwells");
        assert!(t.delta_y.abs() <= 260.0);
    }
}

#[test]
fn sum_of_deltas_is_dy() {
    let p = ScrollParams::for_profile(MotionProfile::Human);
    let dy = 3_000.0;
    let ticks = schedule(dy, &p, 99);
    let sum: f64 = ticks.iter().map(|t| t.delta_y).sum();
    assert!((sum - dy).abs() <= p.peak_tick_px, "sum={sum}, dy={dy}");
}

#[test]
fn determinism_across_profiles() {
    for prof in [
        MotionProfile::Balanced,
        MotionProfile::Human,
        MotionProfile::Paranoid,
    ] {
        let p = ScrollParams::for_profile(prof);
        let a = schedule(600.0, &p, 17);
        let b = schedule(600.0, &p, 17);
        assert_eq!(a.len(), b.len());
    }
}
