//! Submovement decomposition — primary → overshoot → correction (#19).

#![cfg(feature = "cdp-backend")]

use crawlex::render::motion::submovement::{plan, SubmovementParams};
use crawlex::render::motion::MotionProfile;

#[test]
fn fast_profile_is_single_phase() {
    let p = SubmovementParams::for_profile(MotionProfile::Fast);
    let pp = plan(&p, 40.0, 1);
    assert_eq!(pp.len(), 1);
}

#[test]
fn human_profile_sometimes_decomposes() {
    let p = SubmovementParams::for_profile(MotionProfile::Human);
    let mut triples = 0usize;
    for seed in 0..200 {
        if plan(&p, 40.0, seed).len() == 3 {
            triples += 1;
        }
    }
    assert!(
        triples > 50,
        "expected ~55% 3-phase plans, got {triples}/200"
    );
}

#[test]
fn overshoot_goes_past_target() {
    let p = SubmovementParams::for_profile(MotionProfile::Paranoid);
    for seed in 0..100 {
        let pp = plan(&p, 40.0, seed);
        if pp.len() == 3 {
            assert!(pp[0].fraction < 1.0, "primary phase should undershoot");
            assert!(pp[1].fraction > 1.0, "overshoot phase should pass target");
            assert!(
                (pp[2].fraction - 1.0).abs() < 1e-9,
                "correction lands on target"
            );
            return;
        }
    }
    panic!("no 3-phase plan in 100 seeds");
}
