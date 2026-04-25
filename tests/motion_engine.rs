//! Shape + determinism tests for the human motion engine.
//!
//! These are pure-math checks; no Chrome required. Live event-sequence
//! integrity is covered by `tests/motion_live.rs` (#[ignore]).

#![cfg(feature = "cdp-backend")]

use crawlex::render::motion::{fitts_mt_ms, MotionEngine, MotionProfile, Point};

#[test]
fn fitts_mt_scales_with_difficulty() {
    let params = MotionProfile::Balanced.params();
    let easy = fitts_mt_ms(50.0, 100.0, &params);
    let hard = fitts_mt_ms(2000.0, 5.0, &params);
    assert!(
        hard > easy * 3.0,
        "Fitts MT should scale strongly with difficulty (easy={easy}, hard={hard})"
    );
}

#[test]
fn balanced_trajectory_lands_on_target() {
    let mut eng = MotionEngine::with_seed(MotionProfile::Balanced, 17);
    let from = Point { x: 10.0, y: 10.0 };
    let to = Point { x: 800.0, y: 600.0 };
    let pts = eng.trajectory(from, to, 50.0);
    assert!(!pts.is_empty());
    let last = pts.last().copied().unwrap();
    let err = (last.x - to.x).hypot(last.y - to.y);
    assert!(err < 3.0, "trajectory overshot target by {err}px");
}

#[test]
fn windmouse_velocity_is_not_constant() {
    // A flat linear interpolation has constant per-step distance. WindMouse
    // should produce varying step magnitudes (bell-curve velocity profile).
    let mut eng = MotionEngine::with_seed(MotionProfile::Human, 41);
    let pts = eng.trajectory(Point { x: 0.0, y: 0.0 }, Point { x: 800.0, y: 400.0 }, 40.0);
    assert!(pts.len() > 15);
    let speeds: Vec<f64> = pts
        .windows(2)
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .collect();
    let mean = speeds.iter().copied().sum::<f64>() / speeds.len() as f64;
    let var = speeds.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / speeds.len() as f64;
    let std = var.sqrt();
    assert!(
        std > 0.2,
        "WindMouse step sizes should vary; got std={std}, mean={mean}"
    );
}

#[test]
fn fast_profile_short_path_for_throughput() {
    // Fast profile must not add behavioural latency — the Balanced
    // baseline (14.9 rps) demands a tight path. Cap at ≤ 12 samples for
    // any reasonable distance.
    let mut eng = MotionEngine::with_seed(MotionProfile::Fast, 0);
    let pts = eng.trajectory(
        Point { x: 0.0, y: 0.0 },
        Point {
            x: 1200.0,
            y: 900.0,
        },
        40.0,
    );
    assert!(
        pts.len() <= 12,
        "fast profile should keep paths short, got {}",
        pts.len()
    );
    let total_delay: u64 = pts.iter().map(|p| p.delay_ms).sum();
    assert!(
        total_delay < 100,
        "fast profile shouldn't add >100ms of delay, got {total_delay}ms"
    );
}

#[test]
fn deterministic_seed_yields_identical_paths() {
    let go = || {
        let mut e = MotionEngine::with_seed(MotionProfile::Balanced, 1234);
        e.trajectory(Point { x: 5.0, y: 5.0 }, Point { x: 300.0, y: 200.0 }, 30.0)
    };
    let a = go();
    let b = go();
    assert_eq!(a.len(), b.len());
    for (pa, pb) in a.iter().zip(b.iter()) {
        assert!((pa.x - pb.x).abs() < 1e-9);
        assert!((pa.y - pb.y).abs() < 1e-9);
        assert_eq!(pa.delay_ms, pb.delay_ms);
    }
}

#[test]
fn active_profile_round_trip() {
    // Atomic install/read.
    MotionProfile::Human.set_active();
    assert_eq!(MotionProfile::active(), MotionProfile::Human);
    MotionProfile::Balanced.set_active();
    assert_eq!(MotionProfile::active(), MotionProfile::Balanced);
}
