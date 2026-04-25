//! Unit tests for the counter + eviction logic in `PagePool`. We can't
//! construct a real `chromiumoxide::Page` without launching Chrome, so
//! the heavy tests go through `throughput_live.rs` (ignored). Here we
//! exercise the pieces that don't touch a real Page: counters, limits,
//! cleanup, and dirty release.

#![cfg(feature = "cdp-backend")]

use crawlex::render::page_pool::{PagePool, PagePoolLimits};

#[test]
fn defaults_match_plan() {
    let l = PagePoolLimits::default();
    assert_eq!(l.max_pages_per_context, 4);
    assert_eq!(l.max_uses_per_page, 100);
    assert_eq!(l.page_ttl.as_secs(), 300);
    assert_eq!(l.idle_ttl.as_secs(), 30);
}

#[test]
fn register_fresh_and_release_dirty_balances_inflight() {
    let pool = PagePool::new(PagePoolLimits::default());
    pool.register_fresh("ctx-a");
    pool.register_fresh("ctx-a");
    pool.register_fresh("ctx-b");
    assert_eq!(pool.total_in_flight(), 3);
    pool.release_dirty_key("ctx-a");
    assert_eq!(pool.total_in_flight(), 2);
    pool.release_dirty_key("ctx-a");
    pool.release_dirty_key("ctx-b");
    assert_eq!(pool.total_in_flight(), 0);
}

#[test]
fn try_acquire_empty_returns_none() {
    let pool = PagePool::new(PagePoolLimits::default());
    assert!(pool.try_acquire("never-seen").is_none());
    assert_eq!(pool.total_in_flight(), 0);
}

#[test]
fn totals_reflect_created_and_reused() {
    let pool = PagePool::new(PagePoolLimits::default());
    pool.register_fresh("ctx");
    pool.register_fresh("ctx");
    let (created, reused) = pool.totals();
    assert_eq!(created, 2);
    assert_eq!(reused, 0);
}

#[test]
fn drop_context_cleans_counters_and_idle() {
    let pool = PagePool::new(PagePoolLimits::default());
    pool.register_fresh("doomed");
    assert!(pool.total_in_flight() > 0);
    pool.drop_context("doomed");
    // After drop, the context's counters are gone; total_in_flight
    // doesn't walk the dropped entry.
    assert_eq!(pool.total_in_flight(), 0);
}
