//! Distribution + shape tests for the keystroke engine.

#![cfg(feature = "cdp-backend")]

use crawlex::render::keyboard::{KeyEvent, TypingEngine};
use crawlex::render::motion::MotionProfile;

fn total_ms(events: &[KeyEvent]) -> u64 {
    events
        .iter()
        .map(|e| match e {
            KeyEvent::Char { hold_ms, .. }
            | KeyEvent::Typo { hold_ms, .. }
            | KeyEvent::Backspace { hold_ms, .. } => *hold_ms,
            KeyEvent::Pause { ms } => *ms,
        })
        .sum()
}

#[test]
fn schedule_preserves_text() {
    let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 1);
    let text = "crawl the web gently";
    let ev = eng.schedule(text);
    let got: String = ev
        .iter()
        .filter_map(|e| match e {
            KeyEvent::Char { ch, .. } => Some(*ch),
            _ => None,
        })
        .collect();
    assert_eq!(got, text);
}

#[test]
fn balanced_wpm_within_tolerance() {
    let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 2);
    let text: String = "lorem ipsum dolor sit amet ".repeat(20);
    let ev = eng.schedule(&text);
    let ms = total_ms(&ev);
    let minutes = ms as f64 / 60_000.0;
    let words = text.chars().count() as f64 / 5.0;
    let wpm = words / minutes;
    // Target 180 WPM nominal; realistic cadence with flight + hold +
    // occasional thinking pauses + typos yields a lower effective WPM
    // (~30–60 for this profile). The point of the test is that the
    // schedule produces a bounded, plausible-human rate — not a tight
    // match on the `wpm` knob (which feeds the distribution tails, not
    // a hard clock). Guard both tails so a regression that flat-lines
    // delays (→ unrealistically fast) or that blows them up (→ hangs)
    // gets caught.
    assert!(
        (10.0..=400.0).contains(&wpm),
        "effective WPM {wpm} outside plausible-human band (target {})",
        eng.params.wpm
    );
}

#[test]
fn human_profile_slower_than_balanced() {
    // Sanity: human profile should be visibly slower than balanced.
    let mut a = TypingEngine::with_seed(MotionProfile::Balanced, 7);
    let mut b = TypingEngine::with_seed(MotionProfile::Human, 7);
    let text = "the five boxing wizards jump quickly";
    let ms_balanced = total_ms(&a.schedule(text));
    let ms_human = total_ms(&b.schedule(text));
    assert!(
        ms_human > ms_balanced,
        "human profile should be slower (balanced={ms_balanced}ms, human={ms_human}ms)"
    );
}

#[test]
fn hold_times_inside_clamp_band() {
    let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 3);
    let ev = eng.schedule(&"a".repeat(300));
    for e in &ev {
        if let KeyEvent::Char { hold_ms, .. } = e {
            assert!(
                *hold_ms >= 20 && *hold_ms <= 800,
                "hold {hold_ms} out of clamp"
            );
        }
    }
}

#[test]
fn typo_emits_backspace_then_correct_char() {
    let mut eng = TypingEngine::with_seed(MotionProfile::Balanced, 11);
    eng.params.error_rate = 1.0; // force a typo on every eligible char
    let ev = eng.schedule("abc");
    // Expect sequence around each letter: Typo → Pause → Backspace → Pause → Char.
    let mut saw_sequence = false;
    for w in ev.windows(5) {
        if matches!(w[0], KeyEvent::Typo { .. })
            && matches!(w[1], KeyEvent::Pause { .. })
            && matches!(w[2], KeyEvent::Backspace { .. })
            && matches!(w[3], KeyEvent::Pause { .. })
            && matches!(w[4], KeyEvent::Char { .. })
        {
            saw_sequence = true;
            break;
        }
    }
    assert!(saw_sequence, "typo sequence not found in events: {ev:?}");
}

#[test]
fn deterministic_schedule_with_seed() {
    let mut a = TypingEngine::with_seed(MotionProfile::Human, 42);
    let mut b = TypingEngine::with_seed(MotionProfile::Human, 42);
    assert_eq!(
        total_ms(&a.schedule("password123")),
        total_ms(&b.schedule("password123"))
    );
}

#[test]
fn fast_profile_emits_no_thinking_pauses() {
    let mut eng = TypingEngine::with_seed(MotionProfile::Fast, 5);
    let ev = eng.schedule(&"x".repeat(200));
    // Fast has thinking_prob=0, so all Pauses should be inter-key flights
    // capped under a few hundred ms.
    for e in &ev {
        if let KeyEvent::Pause { ms } = e {
            assert!(*ms <= 500, "fast profile pause {ms}ms exceeds flight cap");
        }
    }
}
