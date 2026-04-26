//! Page-load wait strategy.
//!
//! Lives in the crate root (not under `render/`) so it's visible in
//! mini builds too — `Config::wait_strategy` is public and part of the
//! CLI surface via `--wait-strategy`, regardless of whether the browser
//! backend is compiled in.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WaitStrategy {
    Load,
    DomContentLoaded,
    NetworkIdle {
        idle_ms: u64,
    },
    Selector {
        css: String,
        timeout_ms: u64,
    },
    Fixed {
        ms: u64,
    },
    /// Sleep proportional to word count after the page settles. reCAPTCHA v3
    /// and DataDome score a near-instant post-load extraction as bot-like —
    /// a real reader lingers (words / wpm). The render pool owns the actual
    /// timing; this variant only carries the knobs.
    ReadingDwell {
        wpm: u32,
        jitter_ms: u64,
    },
}

impl Default for WaitStrategy {
    fn default() -> Self {
        WaitStrategy::NetworkIdle { idle_ms: 500 }
    }
}

/// Pure helper: compute how long a "reader" would dwell on `words` text.
///
/// Formula: `(words / wpm) * 60_000` ms plus a Gaussian jitter with
/// mean 0 and stddev `jitter_ms`, clamped to `[min, max]`. Deterministic
/// given the same RNG seed — callers pass their own `SmallRng` so tests
/// can pin the outcome without touching wall-clock.
///
/// Kept in the crate root (not under `render/`) so mini builds can unit-test
/// the math without pulling the browser backend.
pub fn compute_dwell_ms(
    words: u64,
    wpm: u32,
    jitter_ms: u64,
    min: u64,
    max: u64,
    rng: &mut rand::rngs::SmallRng,
) -> u64 {
    // wpm=0 would divide by zero; treat as "no base dwell, jitter only".
    let base_ms: f64 = if wpm == 0 {
        0.0
    } else {
        (words as f64) / (wpm as f64) * 60_000.0
    };
    let jitter = gaussian(rng) * (jitter_ms as f64);
    let total = (base_ms + jitter).max(0.0) as u64;
    total.clamp(min, max)
}

/// Box-Muller Gaussian sample (mean 0, stddev 1). Local copy of the idiom
/// used in `render/motion/idle.rs` — duplicating 5 lines is cheaper than
/// exposing a crate-wide `rand` util just for the mini build surface.
fn gaussian(rng: &mut rand::rngs::SmallRng) -> f64 {
    use rand::RngExt;
    // u01 in (0,1] — avoid ln(0).
    let u1 = ((rng.random::<u32>() as f64) / (u32::MAX as f64 + 1.0)).max(f64::MIN_POSITIVE);
    let u2 = (rng.random::<u32>() as f64) / (u32::MAX as f64 + 1.0);
    (-2.0_f64 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;

    #[test]
    fn reading_dwell_variant_serde_roundtrip() {
        // Guards the CLI/config surface: the new variant must survive a
        // JSON round-trip the same way the old ones do.
        let w = WaitStrategy::ReadingDwell {
            wpm: 250,
            jitter_ms: 40,
        };
        let j = serde_json::to_string(&w).expect("serialize");
        let back: WaitStrategy = serde_json::from_str(&j).expect("deserialize");
        match back {
            WaitStrategy::ReadingDwell { wpm, jitter_ms } => {
                assert_eq!(wpm, 250);
                assert_eq!(jitter_ms, 40);
            }
            other => panic!("expected ReadingDwell, got {other:?}"),
        }
    }

    #[test]
    fn reading_dwell_sleep_bounded() {
        // 500 words / 250 wpm = 2 min = 120_000 ms — clamped to max.
        let mut rng = SmallRng::seed_from_u64(42);
        let v = compute_dwell_ms(500, 250, 40, 500, 10_000, &mut rng);
        assert_eq!(v, 10_000, "long reads must clamp to max");

        // 0 words → base 0 ms; jitter can't push us below `min`.
        let mut rng = SmallRng::seed_from_u64(7);
        let v = compute_dwell_ms(0, 250, 40, 500, 10_000, &mut rng);
        assert!(
            (500..=10_000).contains(&v),
            "zero-word base must still respect clamp, got {v}"
        );

        // Fixed seed deterministic: same inputs → same output twice.
        let mut a = SmallRng::seed_from_u64(123);
        let mut b = SmallRng::seed_from_u64(123);
        let va = compute_dwell_ms(80, 250, 40, 500, 10_000, &mut a);
        let vb = compute_dwell_ms(80, 250, 40, 500, 10_000, &mut b);
        assert_eq!(va, vb, "deterministic for a fixed seed");

        // 80 words / 250 wpm ≈ 19_200 ms … but clamped to 10_000. Use a
        // smaller value to check the non-clamped arithmetic.
        // 100 words / 250 wpm = 24_000 ms → clamped. So try 20 words.
        // 20 words / 250 wpm = 4_800 ms; with σ=40 jitter it sits inside
        // [4_700, 4_900] for any reasonable seed.
        let mut rng = SmallRng::seed_from_u64(999);
        let v = compute_dwell_ms(20, 250, 40, 500, 10_000, &mut rng);
        // Generous band: 3σ ≈ 120ms either side.
        assert!(
            (4_680..=4_920).contains(&v),
            "jitter should stay within ~3σ of 4800 ms, got {v}"
        );
    }
}
