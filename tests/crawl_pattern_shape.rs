//! Wave 1 crawl-pattern shape tests.
//!
//! Assert the *distribution* properties detectors score on:
//!
//! * Inter-arrival times follow a log-normal curve (#31).
//! * Session depth cap is Pareto-shaped, not a fixed constant (#33).
//! * Click-graph frontier emits a hub-spoke out-degree distribution
//!   instead of a uniform BFS fan-out (#32).
//!
//! These use the in-process scheduler helpers — no live network.
//! The crawler integration is covered by existing HN throughput tests.

use crawlex::scheduler::{
    frontier_weight, InterArrivalJitter, JitterProfile, SessionDecision, SessionDepthTracker,
    WeightedFrontier, DEFAULT_FRONTIER_WEIGHTS,
};

/// #31 — inter-arrival distribution shape.
///
/// Draw 2000 samples from the `Human` profile. The log-normal with
/// μ=7.5, σ=1.0 has a theoretical median ≈ exp(7.5) ≈ 1808 ms. We
/// allow a wide band because the generator is a simple Box-Muller
/// seeded by system nanos (test stability > Monte-Carlo precision).
#[test]
fn inter_arrival_distribution_matches_log_normal() {
    let jitter = InterArrivalJitter::new(JitterProfile::Human);
    let mut samples: Vec<u64> = (0..2000)
        .map(|_| jitter.sample_raw().as_millis() as u64)
        .collect();
    samples.sort_unstable();

    let median = samples[samples.len() / 2];
    let p10 = samples[samples.len() / 10];
    let p90 = samples[(samples.len() * 9) / 10];

    // Median in the human-cadence band (0.4s – 6s).
    assert!(
        (400..=6000).contains(&median),
        "median {median} ms outside expected human cadence band"
    );
    // Heavy tail: p90 should comfortably exceed p10 * 3.
    assert!(
        p90 > p10.saturating_mul(3),
        "distribution too tight p10={p10} p90={p90} — not log-normal"
    );
    // Tail must sometimes reach several seconds (log-normal has
    // P(x > exp(μ+σ)) ≈ 0.16; exp(8.5) ≈ 4.9s).
    let above_5s = samples.iter().filter(|ms| **ms >= 5000).count();
    assert!(
        above_5s >= 50,
        "tail too thin: only {above_5s} samples ≥ 5s"
    );
}

/// #31b — `Off` profile yields zero delay (motion=fast bypass).
#[test]
fn inter_arrival_off_is_noop() {
    let jitter = InterArrivalJitter::new(JitterProfile::Off);
    for _ in 0..64 {
        assert!(jitter.sample_raw().is_zero());
        assert!(jitter.delay_for_next("s1").is_zero());
    }
}

/// #33 — session depth caps follow a Pareto distribution, not a
/// degenerate single-value cap.
#[test]
fn session_depth_caps_are_pareto_distributed() {
    let tracker = SessionDepthTracker::new(15);
    // Observe until each of many synthetic sessions ends. Record the
    // depth at which `EndSession` fired.
    let mut depths: Vec<usize> = Vec::with_capacity(200);
    for i in 0..200 {
        let key = format!("s{i}");
        let mut steps = 0;
        loop {
            steps += 1;
            if tracker.observe(&key) == SessionDecision::EndSession {
                depths.push(steps);
                break;
            }
            if steps > 500 {
                panic!("session {key} never ended — cap broken");
            }
        }
    }
    depths.sort_unstable();
    let median = depths[depths.len() / 2];
    let p90 = depths[(depths.len() * 9) / 10];
    let max = *depths.last().unwrap();
    // Pareto shape: median sits below the mean; p90 materially higher.
    assert!(
        (4..=30).contains(&median),
        "median session depth {median} outside Pareto expectation"
    );
    assert!(
        p90 > median,
        "no tail: p90={p90} median={median} — not Pareto"
    );
    // Truncation to 2*default_cap = 30.
    assert!(max <= 31, "cap truncation violated: max={max}");
}

/// #33b — cap=0 disables the tracker (legacy crawlers that don't want
/// a Pareto cap can opt out via config).
#[test]
fn session_depth_zero_cap_disables() {
    let tracker = SessionDepthTracker::new(0);
    for _ in 0..100 {
        assert_eq!(tracker.observe("s1"), SessionDecision::Continue);
    }
}

/// #32 — click-graph weights decay with depth.
#[test]
fn frontier_weights_decay_with_depth() {
    let mut prev = f32::INFINITY;
    for (d, w) in DEFAULT_FRONTIER_WEIGHTS.iter().enumerate() {
        assert!(
            *w <= prev,
            "weight curve not monotone at depth {d}: prev={prev} cur={w}"
        );
        prev = *w;
    }
    // Out-of-range depth collapses to tail, not zero.
    let tail = *DEFAULT_FRONTIER_WEIGHTS.last().unwrap();
    assert_eq!(frontier_weight(99), tail);
}

/// #32 — hub-spoke sampling: 1 hub + N deep pages. Over 400 trials
/// the hub fires first materially more often than chance would allow
/// for a uniform pick (uniform would hit hub ~10% of the time; we
/// expect ~40%).
#[test]
fn weighted_frontier_biases_toward_hub() {
    let trials = 400;
    let mut hub_first = 0;
    for _ in 0..trials {
        let f = WeightedFrontier::default();
        f.push("hub".into(), 0);
        for i in 0..9 {
            f.push(format!("deep{i}"), 4);
        }
        if f.pop_weighted().as_deref() == Some("hub") {
            hub_first += 1;
        }
    }
    let pct = (hub_first as f64 / trials as f64) * 100.0;
    assert!(
        pct > 20.0,
        "hub-first rate {pct:.1}% too low — weighting not applied"
    );
    // Not so aggressive it's deterministic; depth weight is non-zero.
    assert!(
        pct < 80.0,
        "hub-first rate {pct:.1}% too high — deep pages never fire"
    );
}

/// #32b — out-degree histogram of a full session matches hub-spoke
/// shape: more entries at shallow depth than at deep depth.
#[test]
fn frontier_histogram_shape_is_hub_spoke() {
    let f = WeightedFrontier::default();
    // Simulate a 100-job crawl seeded by one hub.
    f.push("hub".into(), 0);
    // Depth 1: 8 neighbours of the hub (human fans out a little).
    for i in 0..8 {
        f.push(format!("d1_{i}"), 1);
    }
    // Depth 2: 20 neighbours of depth-1 pages.
    for i in 0..20 {
        f.push(format!("d2_{i}"), 2);
    }
    // Depth 3+: the long tail detectors mostly *don't* see on humans.
    for i in 0..5 {
        f.push(format!("d3_{i}"), 3);
    }
    for i in 0..2 {
        f.push(format!("d4_{i}"), 4);
    }
    let hist = f.depth_histogram();
    // Shape: depth 2 is the widest bucket, deeper buckets taper off.
    assert!(hist[2] > hist[0], "depth-2 should exceed depth-0 (hub)");
    assert!(
        hist[2] >= hist[3],
        "out-degree must not grow beyond depth 2"
    );
    assert!(hist[3] >= hist[4], "tail must taper");
}

/// #23 — CHIPS partitioned cookie isolation.
#[test]
fn partitioned_cookie_isolation_across_top_level_sites() {
    use crawlex::http::cookies::PartitionedCookieStore;
    use http::HeaderMap;
    use url::Url;

    let mut hdr_a = HeaderMap::new();
    hdr_a.append(
        "set-cookie",
        "k=A; Path=/; Secure; SameSite=None; Partitioned"
            .parse()
            .unwrap(),
    );
    let mut hdr_b = HeaderMap::new();
    hdr_b.append(
        "set-cookie",
        "k=B; Path=/; Secure; SameSite=None; Partitioned"
            .parse()
            .unwrap(),
    );

    let store = PartitionedCookieStore::new();
    let cdn = Url::parse("https://embed.cdn/").unwrap();
    store.ingest("https://siteA.test/", &cdn, &hdr_a);
    store.ingest("https://siteB.test/", &cdn, &hdr_b);

    let a = store
        .cookie_header("https://siteA.test/", &cdn)
        .expect("partition A has the cookie");
    let b = store
        .cookie_header("https://siteB.test/", &cdn)
        .expect("partition B has the cookie");
    assert!(a.contains("k=A"), "partition A leaked: {a}");
    assert!(!a.contains("k=B"), "partition B leaked into A: {a}");
    assert!(b.contains("k=B"), "partition B missing: {b}");
    assert!(!b.contains("k=A"), "partition A leaked into B: {b}");

    // A partitioned cookie that lacks Secure is rejected outright.
    let mut bad = HeaderMap::new();
    bad.append(
        "set-cookie",
        "bad=1; Path=/; SameSite=None; Partitioned".parse().unwrap(),
    );
    store.ingest("https://siteA.test/", &cdn, &bad);
    assert!(store.invalid_partitioned_count() >= 1);
}
