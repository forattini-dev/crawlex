//! Bimodal flight-time distribution — alternating-hand fast, same-hand slow.
//!
//! Keystroke dynamics research (Gunetti & Picardi 2005, Killourhy 2009)
//! repeatedly shows two distinct populations for inter-key flight time:
//!
//! * **Alternating hands** (e.g. `t→h`, `e→i`): fast flights, median ≈ 60–90 ms,
//!   because the two hands prepare the next key in parallel.
//! * **Same hand** (e.g. `a→s`, `q→w`): slow flights, median ≈ 110–180 ms,
//!   because fingers on one hand serialise their motion.
//!
//! A unimodal log-logistic model (what `keyboard::mod` uses by default)
//! averages these two populations — good, but missing the bimodal valley
//! in the histogram that modern keystroke ML trains on. This module
//! upgrades flight sampling to pick the right mode given the key
//! transition.

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

use crate::render::motion::MotionProfile;

/// Which hand types a given ASCII letter on QWERTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
    /// Digits, punctuation, or non-ASCII — use unimodal fallback.
    Other,
}

pub fn hand_of(ch: char) -> Hand {
    match ch.to_ascii_lowercase() {
        'q' | 'w' | 'e' | 'r' | 't' | 'a' | 's' | 'd' | 'f' | 'g' | 'z' | 'x' | 'c' | 'v' | 'b' => {
            Hand::Left
        }
        'y' | 'u' | 'i' | 'o' | 'p' | 'h' | 'j' | 'k' | 'l' | 'n' | 'm' => Hand::Right,
        _ => Hand::Other,
    }
}

/// Profile-aware bimodal knobs. Flight sampled as log-normal(μ, σ).
#[derive(Debug, Clone, Copy)]
pub struct BimodalParams {
    /// Alternating-hand median (ms). Log-normal μ = ln(this).
    pub alt_median_ms: f64,
    pub alt_sigma: f64,
    /// Same-hand median (ms).
    pub same_median_ms: f64,
    pub same_sigma: f64,
    /// When one of the transition keys is "Other", fall back to this.
    pub fallback_median_ms: f64,
    pub fallback_sigma: f64,
    /// Global enable — Fast profile short-circuits.
    pub enabled: bool,
}

impl BimodalParams {
    pub fn for_profile(p: MotionProfile) -> Self {
        match p {
            MotionProfile::Fast => BimodalParams {
                alt_median_ms: 20.0,
                alt_sigma: 0.1,
                same_median_ms: 25.0,
                same_sigma: 0.1,
                fallback_median_ms: 22.0,
                fallback_sigma: 0.1,
                enabled: false,
            },
            MotionProfile::Balanced => BimodalParams {
                alt_median_ms: 75.0,
                alt_sigma: 0.3,
                same_median_ms: 135.0,
                same_sigma: 0.35,
                fallback_median_ms: 95.0,
                fallback_sigma: 0.3,
                enabled: true,
            },
            MotionProfile::Human => BimodalParams {
                alt_median_ms: 90.0,
                alt_sigma: 0.32,
                same_median_ms: 160.0,
                same_sigma: 0.38,
                fallback_median_ms: 115.0,
                fallback_sigma: 0.33,
                enabled: true,
            },
            MotionProfile::Paranoid => BimodalParams {
                alt_median_ms: 140.0,
                alt_sigma: 0.35,
                same_median_ms: 230.0,
                same_sigma: 0.4,
                fallback_median_ms: 175.0,
                fallback_sigma: 0.35,
                enabled: true,
            },
        }
    }
}

/// Sample a single flight-time (ms) for a transition `prev → next`.
pub fn sample_flight_ms(prev: char, next: char, params: &BimodalParams, rng: &mut SmallRng) -> u64 {
    let (median, sigma) = match (hand_of(prev), hand_of(next)) {
        (Hand::Left, Hand::Right) | (Hand::Right, Hand::Left) => {
            (params.alt_median_ms, params.alt_sigma)
        }
        (Hand::Left, Hand::Left) | (Hand::Right, Hand::Right) => {
            (params.same_median_ms, params.same_sigma)
        }
        _ => (params.fallback_median_ms, params.fallback_sigma),
    };
    let mu = median.max(1.0).ln();
    let v = lognormal(rng, mu, sigma);
    v.clamp(10.0, 2_000.0) as u64
}

/// Convenience: seed-based flight sampling (test friendly).
pub fn sample_with_seed(prev: char, next: char, params: &BimodalParams, seed: u64) -> u64 {
    let mut rng = SmallRng::seed_from_u64(seed);
    sample_flight_ms(prev, next, params, &mut rng)
}

fn u01(rng: &mut SmallRng) -> f64 {
    (rng.random::<u32>() as f64) / (u32::MAX as f64 + 1.0)
}

fn gaussian(rng: &mut SmallRng) -> f64 {
    let u1 = u01(rng).max(f64::MIN_POSITIVE);
    let u2 = u01(rng);
    let r = (-2.0 * u1.ln()).sqrt();
    r * (2.0 * std::f64::consts::PI * u2).cos()
}

fn lognormal(rng: &mut SmallRng, mu: f64, sigma: f64) -> f64 {
    (mu + sigma * gaussian(rng)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_assignment_matches_qwerty() {
        assert_eq!(hand_of('a'), Hand::Left);
        assert_eq!(hand_of('f'), Hand::Left);
        assert_eq!(hand_of('j'), Hand::Right);
        assert_eq!(hand_of('p'), Hand::Right);
        assert_eq!(hand_of('1'), Hand::Other);
    }

    #[test]
    fn alt_hand_flights_median_lower_than_same_hand() {
        let p = BimodalParams::for_profile(MotionProfile::Balanced);
        let mut rng_a = SmallRng::seed_from_u64(1);
        let mut rng_s = SmallRng::seed_from_u64(1);
        let mut alt = Vec::new();
        let mut same = Vec::new();
        for _ in 0..500 {
            alt.push(sample_flight_ms('t', 'h', &p, &mut rng_a) as f64);
            same.push(sample_flight_ms('a', 's', &p, &mut rng_s) as f64);
        }
        alt.sort_by(|a, b| a.partial_cmp(b).unwrap());
        same.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let alt_med = alt[alt.len() / 2];
        let same_med = same[same.len() / 2];
        assert!(
            same_med > alt_med * 1.2,
            "same-hand median ({same_med}) should be distinctly > alt-hand ({alt_med})"
        );
    }

    #[test]
    fn fast_profile_disables_bimodal() {
        let p = BimodalParams::for_profile(MotionProfile::Fast);
        assert!(!p.enabled);
    }

    #[test]
    fn determinism_with_seed() {
        let p = BimodalParams::for_profile(MotionProfile::Balanced);
        let a = sample_with_seed('a', 's', &p, 42);
        let b = sample_with_seed('a', 's', &p, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn flights_are_clamped() {
        let p = BimodalParams::for_profile(MotionProfile::Balanced);
        for seed in 0..200 {
            let ms = sample_with_seed('a', 's', &p, seed);
            assert!((10..=2_000).contains(&ms), "flight out of clamp: {ms}");
        }
    }

    #[test]
    fn other_chars_use_fallback() {
        let p = BimodalParams::for_profile(MotionProfile::Balanced);
        // ' ' → 'a' uses fallback (space is Other). Median should sit
        // between alt and same.
        let mut rng = SmallRng::seed_from_u64(17);
        let mut vals = Vec::new();
        for _ in 0..300 {
            vals.push(sample_flight_ms(' ', 'a', &p, &mut rng) as f64);
        }
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = vals[vals.len() / 2];
        assert!(med > p.alt_median_ms * 0.5);
        assert!(med < p.same_median_ms * 2.0);
    }
}
