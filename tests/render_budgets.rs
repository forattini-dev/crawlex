//! Unit coverage for `RenderBudgets`: rejection paths, release decrement,
//! multi-dimension independence, and concurrent acquire/release stress.

use std::sync::Arc;
use std::thread;
use url::Url;

use crawlex::scheduler::{BudgetKind, BudgetLimits, RenderBudgets};

#[test]
fn single_host_limit_rejects() {
    let budgets = RenderBudgets::new(BudgetLimits {
        max_per_host: 2,
        max_per_origin: 10,
        max_per_proxy: 10,
        max_per_session: 10,
        ..Default::default()
    });
    let a = budgets.try_acquire("h", "o", None, "s1").unwrap();
    let b = budgets.try_acquire("h", "o", None, "s2").unwrap();
    // Third should be rejected on host dimension.
    match budgets.try_acquire("h", "o", None, "s3") {
        Err(BudgetKind::Host) => {}
        other => panic!("expected Host rejection, got {other:?}"),
    }
    drop(a);
    // After release, acquisition works again.
    let c = budgets.try_acquire("h", "o", None, "s4").unwrap();
    drop(b);
    drop(c);
}

#[test]
fn origin_limit_rejects_and_unwinds_host() {
    let budgets = RenderBudgets::new(BudgetLimits {
        max_per_host: 10,
        max_per_origin: 1,
        max_per_proxy: 10,
        max_per_session: 10,
        ..Default::default()
    });
    let g1 = budgets.try_acquire("h", "o", None, "s1").unwrap();
    // Second hits origin; host counter must NOT be left bumped.
    match budgets.try_acquire("h", "o", None, "s2") {
        Err(BudgetKind::Origin) => {}
        other => panic!("expected Origin rejection, got {other:?}"),
    }
    // Host has 1 inflight (from g1), not 2 (the failed attempt was unwound).
    assert_eq!(budgets.inflight(BudgetKind::Host, "h"), 1);
    drop(g1);
    assert_eq!(budgets.inflight(BudgetKind::Host, "h"), 0);
}

#[test]
fn per_session_default_one_serializes() {
    let budgets = RenderBudgets::new(BudgetLimits::default());
    let a = budgets.try_acquire("h", "o", None, "session-X").unwrap();
    // Same session = rejected.
    match budgets.try_acquire("h2", "o2", None, "session-X") {
        Err(BudgetKind::Session) => {}
        other => panic!("expected Session rejection, got {other:?}"),
    }
    drop(a);
    let _ = budgets.try_acquire("h", "o", None, "session-X").unwrap();
}

#[test]
fn proxy_key_isolated() {
    let budgets = RenderBudgets::new(BudgetLimits {
        max_per_host: 10,
        max_per_origin: 10,
        max_per_proxy: 1,
        max_per_session: 10,
        ..Default::default()
    });
    let p1 = Url::parse("http://proxy1:8080").unwrap();
    let p2 = Url::parse("http://proxy2:8080").unwrap();
    let _g1 = budgets
        .try_acquire("a", "oa", Some(&p1), "s1")
        .expect("proxy1 slot 1");
    // proxy2 unaffected.
    let _g2 = budgets
        .try_acquire("b", "ob", Some(&p2), "s2")
        .expect("proxy2 slot 1");
    // proxy1 full.
    match budgets.try_acquire("c", "oc", Some(&p1), "s3") {
        Err(BudgetKind::Proxy) => {}
        other => panic!("expected Proxy rejection, got {other:?}"),
    }
}

#[test]
fn concurrent_acquire_release_no_race() {
    let budgets = Arc::new(RenderBudgets::new(BudgetLimits {
        max_per_host: 4,
        max_per_origin: 100,
        max_per_proxy: 100,
        max_per_session: 100,
        ..Default::default()
    }));
    let mut handles = Vec::new();
    for t in 0..8 {
        let b = budgets.clone();
        handles.push(thread::spawn(move || {
            for i in 0..200 {
                let sess = format!("s-{t}-{i}");
                // Non-blocking retry loop — counts as a stress test for
                // the atomic CAS path.
                loop {
                    match b.try_acquire("hshared", "o", None, &sess) {
                        Ok(guard) => {
                            drop(guard);
                            break;
                        }
                        Err(_) => thread::yield_now(),
                    }
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(budgets.inflight(BudgetKind::Host, "hshared"), 0);
}

#[test]
fn rejection_counters_tick() {
    let budgets = RenderBudgets::new(BudgetLimits {
        max_per_host: 1,
        max_per_origin: 10,
        max_per_proxy: 10,
        max_per_session: 10,
        ..Default::default()
    });
    let _g = budgets.try_acquire("h", "o", None, "s").unwrap();
    let _ = budgets.try_acquire("h", "o", None, "s2").unwrap_err();
    let _ = budgets.try_acquire("h", "o", None, "s3").unwrap_err();
    let (host, _origin, _proxy, _session) = budgets.rejection_snapshot();
    assert_eq!(host, 2);
}
