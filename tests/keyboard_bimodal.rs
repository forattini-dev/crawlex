//! Bimodal flight-time distribution (#18).

#![cfg(feature = "cdp-backend")]

use crawlex::render::keyboard::bimodal::{hand_of, sample_flight_ms, BimodalParams, Hand};
use crawlex::render::motion::MotionProfile;
use rand::rngs::SmallRng;
use rand::SeedableRng;

#[test]
fn qwerty_hand_map_sane() {
    assert_eq!(hand_of('a'), Hand::Left);
    assert_eq!(hand_of('h'), Hand::Right);
    assert_eq!(hand_of('5'), Hand::Other);
}

#[test]
fn alt_hand_flight_median_below_same_hand() {
    let p = BimodalParams::for_profile(MotionProfile::Human);
    let mut rng_alt = SmallRng::seed_from_u64(7);
    let mut rng_same = SmallRng::seed_from_u64(7);
    let mut alt: Vec<f64> = (0..400)
        .map(|_| sample_flight_ms('t', 'h', &p, &mut rng_alt) as f64)
        .collect();
    let mut same: Vec<f64> = (0..400)
        .map(|_| sample_flight_ms('a', 's', &p, &mut rng_same) as f64)
        .collect();
    alt.sort_by(|a, b| a.partial_cmp(b).unwrap());
    same.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let alt_med = alt[alt.len() / 2];
    let same_med = same[same.len() / 2];
    assert!(same_med > alt_med * 1.25, "alt={alt_med} same={same_med}");
}

#[test]
fn fast_profile_disabled() {
    let p = BimodalParams::for_profile(MotionProfile::Fast);
    assert!(!p.enabled);
}
