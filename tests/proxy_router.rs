//! Integration tests for the score-driven `ProxyRouter`. Covers EWMA
//! convergence, consecutive-failure quarantine, affinity round-trip
//! through SQLite, and operator-triggered eviction.

#![cfg(feature = "sqlite")]

use std::time::Duration;
use url::Url;

use crawlex::proxy::{
    router::{hydrate_from_storage, pack_score_rows},
    ProxyOutcome, ProxyRouter, RotationStrategy, RouterThresholds,
};
use crawlex::storage::sqlite::SqliteStorage;

fn urls(n: usize) -> Vec<Url> {
    (0..n)
        .map(|i| Url::parse(&format!("http://p{i}.test:8080")).unwrap())
        .collect()
}

#[test]
fn ewma_converges_over_sequence() {
    let r = ProxyRouter::new(
        urls(1),
        RotationStrategy::RoundRobin,
        RouterThresholds::default(),
    );
    let p = r.pick("conv.test", 0).unwrap();
    // Alternating fast/slow samples should settle near the mean.
    for _ in 0..20 {
        r.record_outcome(&p, ProxyOutcome::Success { latency_ms: 50.0 });
        r.record_outcome(&p, ProxyOutcome::Success { latency_ms: 150.0 });
    }
    let snap = r.scores_snapshot().pop().unwrap().1;
    let p50 = snap.latency_p50_ms.unwrap();
    assert!(
        p50 > 70.0 && p50 < 130.0,
        "EWMA p50 {p50} did not converge to the 50/150ms band"
    );
    assert_eq!(snap.success, 40);
}

#[test]
fn consecutive_failures_trigger_quarantine() {
    let r = ProxyRouter::new(
        urls(2),
        RotationStrategy::RoundRobin,
        RouterThresholds {
            max_consecutive_failures: 3,
            quarantine_secs: 60,
            ..RouterThresholds::default()
        },
    );
    let p = r.pick("q.test", 0).unwrap();
    for _ in 0..3 {
        r.record_outcome(&p, ProxyOutcome::Timeout);
    }
    let snap = r
        .scores_snapshot()
        .into_iter()
        .find(|(u, _)| u == &p)
        .unwrap()
        .1;
    assert!(snap.quarantine_until.is_some());
    assert!(snap.is_quarantined(std::time::Instant::now()));
}

#[test]
fn quarantine_recovery_picks_proxy_again() {
    let r = ProxyRouter::new(
        urls(1),
        RotationStrategy::RoundRobin,
        RouterThresholds {
            max_consecutive_failures: 2,
            quarantine_secs: 0,
            ..RouterThresholds::default()
        },
    );
    let p = r.pick("recover.test", 0).unwrap();
    for _ in 0..2 {
        r.record_outcome(&p, ProxyOutcome::Timeout);
    }
    std::thread::sleep(Duration::from_millis(10));
    let again = r.pick("recover.test", 0).unwrap();
    assert_eq!(again, p);
}

#[test]
fn eviction_removes_from_rotation() {
    let r = ProxyRouter::new(
        urls(2),
        RotationStrategy::RoundRobin,
        RouterThresholds::default(),
    );
    let proxies = urls(2);
    r.evict(&proxies[0]);
    // After eviction, pick should never return the evicted proxy.
    for i in 0..10 {
        let picked = r.pick("evict.test", i).unwrap();
        assert_ne!(picked, proxies[0]);
    }
}

#[tokio::test]
async fn affinity_round_trip_via_sqlite() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let storage = SqliteStorage::open(&path).unwrap();
    let r = ProxyRouter::new(
        urls(3),
        RotationStrategy::RoundRobin,
        RouterThresholds::default(),
    );
    // Force affinity to pin, then record enough success to shape the score.
    let p = r.pick("round.trip", 7).unwrap();
    for _ in 0..5 {
        r.record_outcome(&p, ProxyOutcome::Success { latency_ms: 100.0 });
    }
    let (scores, affinity) = r.drain_pending();
    assert!(!scores.is_empty());
    assert!(!affinity.is_empty());
    storage
        .save_proxy_scores(pack_score_rows(scores))
        .await
        .unwrap();
    let aff_rows: Vec<(String, i64, String)> = affinity
        .into_iter()
        .map(|(h, b, u)| (h, b as i64, u.to_string()))
        .collect();
    storage.save_host_affinity(aff_rows).await.unwrap();

    // Give the writer thread a moment to commit before we crack a read
    // connection — the handshake uses WAL so a racing read is legal, but
    // we want the row visible.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Fresh router + fresh storage handle pointing at the same file. The
    // hydration path should re-pin (round.trip, 7) → same proxy.
    let r2 = ProxyRouter::new(
        urls(3),
        RotationStrategy::RoundRobin,
        RouterThresholds::default(),
    );
    let storage2 = SqliteStorage::open(&path).unwrap();
    hydrate_from_storage(&r2, &storage2).await.unwrap();
    let again = r2.pick("round.trip", 7).unwrap();
    assert_eq!(again, p, "affinity did not survive the SQLite round-trip");
    // Score history survived too.
    let snap2 = r2
        .scores_snapshot()
        .into_iter()
        .find(|(u, _)| u == &p)
        .unwrap()
        .1;
    assert_eq!(snap2.success, 5);
}
