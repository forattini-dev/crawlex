//! Idle OU drift — ambient cursor motion (B.2).

#![cfg(feature = "cdp-backend")]

use crawlex::render::motion::idle::{IdleDrift, IdleState};
use crawlex::render::motion::MotionProfile;

#[test]
fn fast_profile_disables_drift() {
    let mut d = IdleDrift::for_profile(MotionProfile::Fast, 1);
    for _ in 0..32 {
        assert_eq!(d.next_offset(), (0.0, 0.0));
    }
}

#[test]
fn balanced_drift_bounded_and_nonzero() {
    let mut d = IdleDrift::for_profile(MotionProfile::Balanced, 3);
    let mut max = 0.0f64;
    let mut any_nonzero = false;
    for _ in 0..300 {
        let (x, y) = d.next_offset();
        if x.abs() > 0.0 || y.abs() > 0.0 {
            any_nonzero = true;
        }
        max = max.max(x.abs()).max(y.abs());
    }
    assert!(any_nonzero);
    assert!(max < 15.0, "drift should be bounded, got {max}");
}

#[test]
fn idle_state_pause_resume() {
    let s = IdleState::new();
    assert!(!s.is_action_active());
    s.action_begin();
    assert!(s.is_action_active());
    s.action_end();
    assert!(!s.is_action_active());
}

#[test]
fn mean_reverts_to_zero() {
    let mut d = IdleDrift::for_profile(MotionProfile::Human, 11);
    let mut xs = Vec::with_capacity(3_000);
    for _ in 0..3_000 {
        let (x, _) = d.next_offset();
        xs.push(x);
    }
    let mean: f64 = xs.iter().copied().sum::<f64>() / xs.len() as f64;
    assert!(mean.abs() < 0.7, "mean={mean}");
}
