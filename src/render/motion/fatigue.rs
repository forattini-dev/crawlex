//! Session fatigue proxy — velocity decay over minutes_in_session.
//!
//! Real sessions slow down. Keystroke studies (Dhakal 2018, Vertanen 2015)
//! show typing rate decays linearly at ~0.5%/minute over an hour's
//! continuous use; mouse velocity shows a similar but shallower slope
//! (Rosenblum 2003). Antibot ML learns this signature — a session that
//! stays at peak throughput for hours is an obvious bot.
//!
//! This module exposes a monotone `velocity_factor(minutes)` and a
//! `flight_time_factor(minutes)` pair the motion + keyboard engines can
//! multiply into their per-step parameters. Both stay well-bounded so
//! long-running crawls don't asymptote to 0 throughput.
//!
//! Process-wide session start (`SESSION_START_MS`) initialises lazily on
//! first query; call `reset_session_start()` in tests that need
//! determinism.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::render::motion::MotionProfile;

static SESSION_START_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ensure_started() -> u64 {
    let start = SESSION_START_MS.load(Ordering::Relaxed);
    if start != 0 {
        return start;
    }
    let n = now_ms();
    // CAS — first writer wins, every other thread adopts the same epoch.
    let _ = SESSION_START_MS.compare_exchange(0, n, Ordering::AcqRel, Ordering::Acquire);
    SESSION_START_MS.load(Ordering::Relaxed)
}

/// Override the session start (mainly for tests / replays).
pub fn set_session_start_ms(t: u64) {
    SESSION_START_MS.store(t, Ordering::Relaxed);
}

/// Minutes elapsed since the current session started.
pub fn minutes_in_session() -> f64 {
    let start = ensure_started();
    let now = now_ms();
    if now <= start {
        return 0.0;
    }
    (now - start) as f64 / 60_000.0
}

/// Velocity multiplier: 1.0 at session start, tapering toward ~0.7 over a
/// long session. Decay slope (`k`) depends on motion profile — Fast ignores
/// fatigue entirely, Paranoid decays fastest.
pub fn velocity_factor_for(profile: MotionProfile, minutes: f64) -> f64 {
    let k = match profile {
        MotionProfile::Fast => 0.0,
        MotionProfile::Balanced => 0.0005,
        MotionProfile::Human => 0.0008,
        MotionProfile::Paranoid => 0.0012,
    };
    // Monotone decay, floored so we never stall completely.
    (1.0 - k * minutes.max(0.0)).clamp(0.7, 1.0)
}

/// Inter-key flight multiplier: inverse of velocity — tired fingers flight
/// *longer*. Ceiling of 1.3 keeps typing from looking implausibly slow.
pub fn flight_factor_for(profile: MotionProfile, minutes: f64) -> f64 {
    let v = velocity_factor_for(profile, minutes);
    if v <= 0.0 {
        return 1.3;
    }
    (1.0 / v).clamp(1.0, 1.3)
}

/// Convenience: current session's velocity factor for the active profile.
pub fn current_velocity_factor() -> f64 {
    velocity_factor_for(MotionProfile::active(), minutes_in_session())
}

pub fn current_flight_factor() -> f64 {
    flight_factor_for(MotionProfile::active(), minutes_in_session())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_profile_has_no_decay() {
        assert_eq!(velocity_factor_for(MotionProfile::Fast, 60.0), 1.0);
        assert_eq!(flight_factor_for(MotionProfile::Fast, 60.0), 1.0);
    }

    #[test]
    fn velocity_decays_monotonically() {
        let a = velocity_factor_for(MotionProfile::Human, 0.0);
        let b = velocity_factor_for(MotionProfile::Human, 30.0);
        let c = velocity_factor_for(MotionProfile::Human, 120.0);
        assert!(a >= b && b >= c);
        assert_eq!(a, 1.0);
    }

    #[test]
    fn velocity_never_drops_below_floor() {
        for m in [0.0, 100.0, 1_000.0, 10_000.0] {
            let v = velocity_factor_for(MotionProfile::Paranoid, m);
            assert!(v >= 0.7, "velocity floor breached at m={m}: v={v}");
        }
    }

    #[test]
    fn flight_grows_with_fatigue() {
        let a = flight_factor_for(MotionProfile::Human, 0.0);
        let b = flight_factor_for(MotionProfile::Human, 120.0);
        assert!(b >= a);
        assert!(b <= 1.3);
    }

    #[test]
    fn session_start_override_works() {
        set_session_start_ms(1);
        let mins = minutes_in_session();
        assert!(mins > 0.0);
    }
}
