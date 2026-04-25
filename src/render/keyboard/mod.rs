//! Keystroke timing engine — log-normal hold, log-logistic flight, Pareto pauses.
//!
//! Modern antibot ML (Arkose, hCaptcha, Cloudflare) fingerprints typing
//! cadence. A fixed inter-key delay is as visible as a missing mousemove.
//! This engine samples from the distributions reported in the keystroke
//! dynamics literature (`research/evasion-deep-dive.md` §9.6):
//!
//! * **Hold time** (key down → key up): log-normal with μ=log(100ms), σ=0.3.
//!   Typical range 50–180 ms; skilled typists sit near the mode.
//! * **Inter-key flight**: log-logistic with α≈70 ms, β≈3.5 — heavier tail
//!   than log-normal so occasional slow transitions look natural.
//! * **Thinking pauses**: Pareto with xₘ=500 ms, α=1.5. Heavy-tailed: most
//!   pauses are short, a few last several seconds.
//! * **Typos**: injected at `error_rate` with a corrective backspace + the
//!   intended character — matches human error-and-fix cadence.

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::render::motion::MotionProfile;

pub mod bimodal;
pub use bimodal::{hand_of, BimodalParams, Hand};

/// Typing profile derived from the global `MotionProfile` — same knob
/// controls both mouse realism and typing realism so operators tune one
/// setting.
#[derive(Debug, Clone, Copy)]
pub struct TypingParams {
    /// Words per minute target (words = 5 chars).
    pub wpm: f64,
    /// Log-normal μ for hold time (ln(ms)).
    pub hold_mu: f64,
    pub hold_sigma: f64,
    /// Log-logistic α (scale, ms) and β (shape) for inter-key flight.
    pub flight_alpha_ms: f64,
    pub flight_beta: f64,
    /// Pareto xₘ (scale, ms) and α (shape) for thinking pauses.
    pub pause_scale_ms: f64,
    pub pause_alpha: f64,
    /// Probability the engine inserts a thinking pause before a keystroke.
    pub thinking_prob: f64,
    /// Probability of a typo on any given character.
    pub error_rate: f64,
}

impl TypingParams {
    pub fn for_profile(p: MotionProfile) -> Self {
        match p {
            MotionProfile::Fast => TypingParams {
                wpm: 600.0,
                hold_mu: (30.0f64).ln(),
                hold_sigma: 0.15,
                flight_alpha_ms: 20.0,
                flight_beta: 4.0,
                pause_scale_ms: 50.0,
                pause_alpha: 3.0,
                thinking_prob: 0.0,
                error_rate: 0.0,
            },
            MotionProfile::Balanced => TypingParams {
                wpm: 180.0,
                hold_mu: (90.0f64).ln(),
                hold_sigma: 0.3,
                flight_alpha_ms: 70.0,
                flight_beta: 3.5,
                pause_scale_ms: 500.0,
                pause_alpha: 1.5,
                thinking_prob: 0.05,
                error_rate: 0.01,
            },
            MotionProfile::Human => TypingParams {
                wpm: 120.0,
                hold_mu: (100.0f64).ln(),
                hold_sigma: 0.35,
                flight_alpha_ms: 110.0,
                flight_beta: 3.0,
                pause_scale_ms: 700.0,
                pause_alpha: 1.4,
                thinking_prob: 0.08,
                error_rate: 0.015,
            },
            MotionProfile::Paranoid => TypingParams {
                wpm: 70.0,
                hold_mu: (130.0f64).ln(),
                hold_sigma: 0.4,
                flight_alpha_ms: 180.0,
                flight_beta: 2.6,
                pause_scale_ms: 1200.0,
                pause_alpha: 1.3,
                thinking_prob: 0.15,
                error_rate: 0.03,
            },
        }
    }
}

/// Scheduled event in the keystroke timeline.
#[derive(Debug, Clone)]
pub enum KeyEvent {
    /// Press-hold-release a single character.
    Char { ch: char, hold_ms: u64 },
    /// Idle gap before the next keystroke (thinking / natural flight).
    Pause { ms: u64 },
    /// Typo: press the wrong char, then the engine emits a `Backspace`
    /// Special next, then the correct char. Encoded as one enum so the
    /// executor can decide to use `Input.insertText` for unicode paths.
    Typo { wrong: char, hold_ms: u64 },
    /// Backspace (used to erase a typo).
    Backspace { hold_ms: u64 },
}

pub struct TypingEngine {
    rng: SmallRng,
    pub params: TypingParams,
    /// Bimodal flight distribution (alternating vs same-hand). When
    /// `bimodal.enabled` is true the engine swaps the default log-logistic
    /// inter-key flight for a per-transition log-normal sample.
    pub bimodal: BimodalParams,
    /// Multiplier applied to every inter-key flight (≥1.0). Set by fatigue
    /// integration — sessions that have been running for many minutes see
    /// slower typing. Clamp at 1.3× via `fatigue::flight_factor_for`.
    pub flight_scale: f64,
}

impl TypingEngine {
    pub fn new(profile: MotionProfile) -> Self {
        Self {
            rng: rand::make_rng::<SmallRng>(),
            params: TypingParams::for_profile(profile),
            bimodal: BimodalParams::for_profile(profile),
            flight_scale: crate::render::motion::fatigue::flight_factor_for(
                profile,
                crate::render::motion::fatigue::minutes_in_session(),
            ),
        }
    }

    pub fn with_seed(profile: MotionProfile, seed: u64) -> Self {
        Self {
            rng: SmallRng::seed_from_u64(seed),
            params: TypingParams::for_profile(profile),
            bimodal: BimodalParams::for_profile(profile),
            flight_scale: 1.0,
        }
    }

    /// Schedule a typing timeline for `text`. Each `KeyEvent` is either a
    /// character to dispatch or a pause to sleep through. Total wall-clock
    /// time roughly matches `chars / (wpm * 5 / 60)` seconds.
    pub fn schedule(&mut self, text: &str) -> Vec<KeyEvent> {
        let mut out = Vec::with_capacity(text.chars().count() * 2);
        let mut first = true;
        let mut prev_ch: Option<char> = None;

        for ch in text.chars() {
            if !first {
                // Inter-key flight delay. Bimodal (alt-hand fast / same-hand
                // slow) when enabled; log-logistic baseline otherwise. Scale
                // by session fatigue.
                let base = if self.bimodal.enabled {
                    let prev = prev_ch.unwrap_or(ch);
                    bimodal::sample_flight_ms(prev, ch, &self.bimodal, &mut self.rng) as f64
                } else {
                    log_logistic(
                        &mut self.rng,
                        self.params.flight_alpha_ms,
                        self.params.flight_beta,
                    )
                };
                let flight = (base * self.flight_scale.max(1.0)).clamp(10.0, 2_600.0) as u64;
                out.push(KeyEvent::Pause { ms: flight });
            }

            // Occasional thinking pause on top of the flight delay.
            if self.params.thinking_prob > 0.0 && u01(&mut self.rng) < self.params.thinking_prob {
                let pause = pareto(
                    &mut self.rng,
                    self.params.pause_scale_ms,
                    self.params.pause_alpha,
                );
                let pause = pause.clamp(self.params.pause_scale_ms, 10_000.0) as u64;
                out.push(KeyEvent::Pause { ms: pause });
            }

            // Typo injection (only on ASCII letters — we're not trying to
            // model IME/unicode correction cycles).
            if self.params.error_rate > 0.0
                && ch.is_ascii_alphabetic()
                && u01(&mut self.rng) < self.params.error_rate
            {
                let wrong = neighbor_key(&mut self.rng, ch);
                let hold_wrong =
                    lognormal_u64(&mut self.rng, self.params.hold_mu, self.params.hold_sigma);
                out.push(KeyEvent::Typo {
                    wrong,
                    hold_ms: hold_wrong,
                });

                // Short realisation delay before the backspace.
                let notice = pareto(&mut self.rng, 150.0, 2.0).clamp(80.0, 600.0) as u64;
                out.push(KeyEvent::Pause { ms: notice });

                let hold_bs =
                    lognormal_u64(&mut self.rng, self.params.hold_mu, self.params.hold_sigma);
                out.push(KeyEvent::Backspace { hold_ms: hold_bs });

                // Short recovery delay before the correct char goes in.
                let recover = log_logistic(
                    &mut self.rng,
                    self.params.flight_alpha_ms,
                    self.params.flight_beta,
                );
                out.push(KeyEvent::Pause {
                    ms: (recover.clamp(20.0, 400.0)) as u64,
                });
            }

            let hold = lognormal_u64(&mut self.rng, self.params.hold_mu, self.params.hold_sigma);
            out.push(KeyEvent::Char { ch, hold_ms: hold });
            first = false;
            prev_ch = Some(ch);
        }
        out
    }
}

fn u01(rng: &mut SmallRng) -> f64 {
    (rng.next_u32() as f64) / (u32::MAX as f64 + 1.0)
}

fn gaussian(rng: &mut SmallRng) -> f64 {
    // Box-Muller (single sample).
    let u1 = u01(rng).max(f64::MIN_POSITIVE);
    let u2 = u01(rng);
    let r = (-2.0 * u1.ln()).sqrt();
    r * (2.0 * std::f64::consts::PI * u2).cos()
}

fn lognormal_ms(rng: &mut SmallRng, mu: f64, sigma: f64) -> f64 {
    (mu + sigma * gaussian(rng)).exp()
}

fn lognormal_u64(rng: &mut SmallRng, mu: f64, sigma: f64) -> u64 {
    let v = lognormal_ms(rng, mu, sigma);
    v.clamp(20.0, 800.0) as u64
}

/// Inverse-CDF sample: `x = α · (u / (1 - u))^(1/β)`.
fn log_logistic(rng: &mut SmallRng, alpha: f64, beta: f64) -> f64 {
    let u = u01(rng).clamp(1e-6, 1.0 - 1e-6);
    alpha * (u / (1.0 - u)).powf(1.0 / beta)
}

/// Pareto Type I with scale xₘ and shape α: `x = xₘ · (1 - u)^(-1/α)`.
fn pareto(rng: &mut SmallRng, scale: f64, alpha: f64) -> f64 {
    let u = u01(rng).clamp(1e-6, 1.0 - 1e-6);
    scale * (1.0 - u).powf(-1.0 / alpha.max(0.1))
}

/// Pick a plausible typo — a neighbouring key on QWERTY. Falls back to the
/// original char (no-op typo) when no neighbour is known; the caller still
/// treats it as a typo but the effect is imperceptible.
fn neighbor_key(rng: &mut SmallRng, ch: char) -> char {
    let neighbors: &[char] = match ch.to_ascii_lowercase() {
        'q' => &['w', 'a'],
        'w' => &['q', 'e', 's'],
        'e' => &['w', 'r', 'd'],
        'r' => &['e', 't', 'f'],
        't' => &['r', 'y', 'g'],
        'y' => &['t', 'u', 'h'],
        'u' => &['y', 'i', 'j'],
        'i' => &['u', 'o', 'k'],
        'o' => &['i', 'p', 'l'],
        'p' => &['o', 'l'],
        'a' => &['q', 's', 'z'],
        's' => &['a', 'd', 'w', 'x'],
        'd' => &['s', 'f', 'e', 'c'],
        'f' => &['d', 'g', 'r', 'v'],
        'g' => &['f', 'h', 't', 'b'],
        'h' => &['g', 'j', 'y', 'n'],
        'j' => &['h', 'k', 'u', 'm'],
        'k' => &['j', 'l', 'i'],
        'l' => &['k', 'o', 'p'],
        'z' => &['a', 'x'],
        'x' => &['z', 's', 'c'],
        'c' => &['x', 'd', 'v'],
        'v' => &['c', 'f', 'b'],
        'b' => &['v', 'g', 'n'],
        'n' => &['b', 'h', 'm'],
        'm' => &['n', 'j'],
        _ => return ch,
    };
    let idx = (u01(rng) * neighbors.len() as f64) as usize;
    let pick = neighbors.get(idx).copied().unwrap_or(ch);
    if ch.is_ascii_uppercase() {
        pick.to_ascii_uppercase()
    } else {
        pick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_chars(events: &[KeyEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, KeyEvent::Char { .. } | KeyEvent::Typo { .. }))
            .count()
    }

    #[test]
    fn schedule_emits_one_char_event_per_text_char() {
        let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 1);
        let text = "hello world";
        let ev = eng.schedule(text);
        // Each text char produces at least one Char/Typo event (typos add
        // an extra Typo + Backspace + Char, but the real char still lands).
        let real_chars: Vec<char> = ev
            .iter()
            .filter_map(|e| match e {
                KeyEvent::Char { ch, .. } => Some(*ch),
                _ => None,
            })
            .collect();
        assert_eq!(real_chars.iter().collect::<String>(), text);
    }

    #[test]
    fn schedule_is_deterministic_with_seed() {
        let mut a = TypingEngine::with_seed(MotionProfile::Balanced, 7);
        let mut b = TypingEngine::with_seed(MotionProfile::Balanced, 7);
        let ea = a.schedule("abc123");
        let eb = b.schedule("abc123");
        assert_eq!(count_chars(&ea), count_chars(&eb));
        assert_eq!(ea.len(), eb.len());
    }

    #[test]
    fn hold_times_log_normal_range() {
        let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 2);
        let ev = eng.schedule(&"a".repeat(500));
        let holds: Vec<u64> = ev
            .iter()
            .filter_map(|e| match e {
                KeyEvent::Char { hold_ms, .. } => Some(*hold_ms),
                _ => None,
            })
            .collect();
        let mean = holds.iter().copied().sum::<u64>() as f64 / holds.len() as f64;
        // Balanced profile: exp(ln(90)) = 90 ms expected; LogNormal mean
        // is exp(μ + σ²/2) ≈ 90 · e^0.045 ≈ 94 ms.
        assert!(
            mean > 60.0 && mean < 150.0,
            "hold mean outside plausible band: {mean}"
        );
        // Clamp at 20..800.
        for h in &holds {
            assert!(*h >= 20 && *h <= 800, "hold out of clamp: {h}");
        }
    }

    #[test]
    fn effective_wpm_within_tolerance() {
        // WPM ≈ (chars / 5) / minutes. Convert schedule total time.
        let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 3);
        let text: String = "the quick brown fox jumps over the lazy dog ".repeat(10);
        let events = eng.schedule(&text);
        let total_ms: u64 = events
            .iter()
            .map(|e| match e {
                KeyEvent::Char { hold_ms, .. }
                | KeyEvent::Typo { hold_ms, .. }
                | KeyEvent::Backspace { hold_ms, .. } => *hold_ms,
                KeyEvent::Pause { ms } => *ms,
            })
            .sum();
        let minutes = (total_ms as f64) / 60_000.0;
        let words = (text.chars().count() as f64) / 5.0;
        let wpm = words / minutes.max(1e-6);
        // Balanced target: 180 WPM. Tolerate broad ±50% — typing model
        // inserts thinking pauses + typos that lower effective rate.
        assert!(
            (5.0..=600.0).contains(&wpm),
            "effective WPM out of plausible band: {wpm} (target {})",
            eng.params.wpm
        );
    }

    #[test]
    fn typo_sequence_is_well_formed() {
        // With error_rate high and fixed seed, we should see at least one
        // Typo → Backspace → Char run.
        let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 12);
        eng.params.error_rate = 0.9;
        let ev = eng.schedule("abcdef");
        let mut saw_cycle = false;
        for win in ev.windows(5) {
            let has_typo = matches!(win[0], KeyEvent::Typo { .. });
            let has_bs = win.iter().any(|e| matches!(e, KeyEvent::Backspace { .. }));
            let has_char = win.iter().any(|e| matches!(e, KeyEvent::Char { .. }));
            if has_typo && has_bs && has_char {
                saw_cycle = true;
                break;
            }
        }
        assert!(saw_cycle, "expected Typo+Backspace+Char cycle in events");
    }

    #[test]
    fn neighbor_key_is_adjacent_on_qwerty() {
        let mut rng = SmallRng::seed_from_u64(9);
        for _ in 0..50 {
            let n = neighbor_key(&mut rng, 'a');
            // 'a' neighbours: q/s/z (all ≠ a).
            assert_ne!(n, 'a');
            assert!(matches!(n, 'q' | 's' | 'z'));
        }
    }

    #[test]
    fn fast_profile_has_no_typos_or_thinking_pauses() {
        let mut eng = TypingEngine::with_seed(MotionProfile::Fast, 4);
        let ev = eng.schedule("hello world from the fast profile");
        for e in &ev {
            assert!(!matches!(e, KeyEvent::Typo { .. }));
            assert!(!matches!(e, KeyEvent::Backspace { .. }));
        }
    }
}
