//! Dedupe invariants and false-positive sanity tests.
//!
//! The crawler's frontier uses a growable bloom filter backed by a bounded
//! exact-recent set. These tests lock the contract:
//!   * first insert of a key returns true (newly seen);
//!   * second insert of the same key returns false (dupe);
//!   * fp rate stays close to configured when exercised at scale.

use crawlex::frontier::Dedupe;

#[test]
fn first_insert_is_new_second_is_dupe() {
    let d = Dedupe::new(1000, 0.001);
    assert!(d.insert_if_new("https://example.com/a"));
    assert!(!d.insert_if_new("https://example.com/a"));
}

#[test]
fn distinct_keys_are_all_new() {
    let d = Dedupe::new(1000, 0.001);
    for i in 0..500 {
        let key = format!("https://example.com/p/{i}");
        assert!(d.insert_if_new(&key), "key {i} unexpectedly marked dupe");
    }
}

#[test]
fn exact_recent_wipe_does_not_corrupt_bloom() {
    // exact_cap is 100_000. Fill past it so the wipe happens, then verify
    // keys inserted before the wipe are still deduped by the bloom.
    let d = Dedupe::new(200_000, 0.01);
    let known = "https://pre-wipe.example/zero";
    assert!(d.insert_if_new(known));

    for i in 0..100_500 {
        let _ = d.insert_if_new(&format!("https://bulk.example/{i}"));
    }
    // The known key survived in the bloom even though exact_recent was
    // cleared — second attempt must still be dupe.
    assert!(!d.insert_if_new(known), "bloom lost a pre-wipe key");
}

#[test]
fn false_positive_rate_stays_reasonable() {
    // With fp_rate 0.01 and expected 10_000 we insert 10_000 distinct keys
    // then query 10_000 different ones and assert <2% false positive.
    // Loose bound so the test stays stable; the real fp rate should sit
    // below 1% but we don't want flaky CI.
    let d = Dedupe::new(10_000, 0.01);
    for i in 0..10_000 {
        let _ = d.insert_if_new(&format!("A-{i}"));
    }
    let mut fps = 0usize;
    for i in 0..10_000 {
        // Insert B-i which is different from all A-i entries; a `false`
        // return means the bloom incorrectly thought we'd seen it.
        if !d.insert_if_new(&format!("B-{i}")) {
            fps += 1;
        }
    }
    let rate = fps as f64 / 10_000.0;
    assert!(
        rate < 0.02,
        "fp rate {:.4} exceeded 2% ceiling (raw {})",
        rate,
        fps
    );
}
