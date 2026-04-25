//! Per-context Page pool — reuses Chrome tabs across `RenderPool::render`
//! calls within the same BrowserContext to amortise the ~300-500ms CDP
//! domain-enable + shim-inject setup cost.
//!
//! ## Contract
//!
//! - `acquire(ctx_key)` returns an idle `PooledPage` if one is available
//!   for the given `(browser_key, session_id)` pair, otherwise callers
//!   must `create_fresh()` and later `release_fresh()`.
//! - `release(page)` returns a clean page to the idle pool. Pages that
//!   exceeded `max_uses_per_page` or `page_ttl` are dropped instead.
//! - Callers must `release_dirty(ctx_key)` (drops the page silently) when
//!   the page was contaminated — e.g. challenge hit, fatal CDP error —
//!   so the next acquirer gets a fresh tab.
//! - `cleanup_idle` sweeps pages idle > `idle_ttl`.
//!
//! ## Thread safety
//!
//! One `Mutex<HashMap<ctx_key, VecDeque<PooledPage>>>` for idle pages +
//! one `AtomicUsize` per ctx for in-flight count. In-flight never races
//! with drop because `acquire`/`release` hold the mutex only to hand out
//! or stash an owned `PooledPage`.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "cdp-backend")]
use crate::render::chrome::page::Page;

/// Limits tuned via the `PagePool::new` knobs. Defaults mirror the
/// numbers quoted in the phase-5 plan: 4 pages per context, 100 reuses
/// per page, 300s lifetime, 30s idle eviction.
#[derive(Debug, Clone, Copy)]
pub struct PagePoolLimits {
    pub max_pages_per_context: usize,
    pub max_uses_per_page: u32,
    pub page_ttl: Duration,
    pub idle_ttl: Duration,
}

impl Default for PagePoolLimits {
    fn default() -> Self {
        Self {
            max_pages_per_context: 4,
            max_uses_per_page: 100,
            page_ttl: Duration::from_secs(300),
            idle_ttl: Duration::from_secs(30),
        }
    }
}

/// Single pooled Page handle. Wraps a `chromiumoxide::Page` with the
/// counters the pool needs to decide whether to keep or drop on release.
#[cfg(feature = "cdp-backend")]
pub struct PooledPage {
    pub page: Page,
    pub created_at: Instant,
    pub last_used: Instant,
    pub uses: u32,
    /// Composite key = `browser_key|session_id` (matches
    /// `RenderPool::browser_session_key`). Cheap clone.
    pub ctx_key: String,
}

#[cfg(feature = "cdp-backend")]
impl PooledPage {
    pub fn new(page: Page, ctx_key: String) -> Self {
        let now = Instant::now();
        Self {
            page,
            created_at: now,
            last_used: now,
            uses: 1,
            ctx_key,
        }
    }
}

/// Counter state tracked per context. Kept in an inner struct so we
/// avoid a `DashMap<_, Arc<AtomicUsize>>` (which would make the
/// "release decrements" path a two-step DashMap lookup + atomic).
#[derive(Debug, Default)]
pub struct CtxCounters {
    pub in_flight: AtomicUsize,
    pub total_created: AtomicUsize,
    pub total_reused: AtomicUsize,
}

#[cfg(feature = "cdp-backend")]
pub struct PagePool {
    idle: Mutex<HashMap<String, VecDeque<PooledPage>>>,
    counters: Mutex<HashMap<String, Arc<CtxCounters>>>,
    limits: PagePoolLimits,
}

#[cfg(feature = "cdp-backend")]
impl PagePool {
    pub fn new(limits: PagePoolLimits) -> Self {
        Self {
            idle: Mutex::new(HashMap::new()),
            counters: Mutex::new(HashMap::new()),
            limits,
        }
    }

    pub fn limits(&self) -> PagePoolLimits {
        self.limits
    }

    fn counters_for(&self, ctx_key: &str) -> Arc<CtxCounters> {
        let mut guard = self.counters.lock();
        if let Some(c) = guard.get(ctx_key) {
            return c.clone();
        }
        let c = Arc::new(CtxCounters::default());
        guard.insert(ctx_key.to_string(), c.clone());
        c
    }

    /// Try to pull an idle page for this context. Returns `None` if no
    /// idle page is available — caller must create a fresh one via the
    /// usual `browser.new_page(...)` path and call `register_fresh`.
    pub fn try_acquire(&self, ctx_key: &str) -> Option<PooledPage> {
        let now = Instant::now();
        let mut guard = self.idle.lock();
        let q = guard.get_mut(ctx_key)?;
        // Pop front; evict stale entries while we're at it.
        while let Some(mut candidate) = q.pop_front() {
            if candidate.uses >= self.limits.max_uses_per_page
                || now.duration_since(candidate.created_at) > self.limits.page_ttl
            {
                // Drop silently — Page::Drop spawns a close task.
                continue;
            }
            candidate.uses = candidate.uses.saturating_add(1);
            candidate.last_used = now;
            let ctrs = self.counters_for(ctx_key);
            ctrs.in_flight.fetch_add(1, Ordering::Relaxed);
            ctrs.total_reused.fetch_add(1, Ordering::Relaxed);
            return Some(candidate);
        }
        None
    }

    /// Register a page that the caller just created via
    /// `browser.new_page(...)`. Bumps the in-flight counter; the caller
    /// must eventually `release` or `release_dirty_key`.
    pub fn register_fresh(&self, ctx_key: &str) {
        let ctrs = self.counters_for(ctx_key);
        ctrs.in_flight.fetch_add(1, Ordering::Relaxed);
        ctrs.total_created.fetch_add(1, Ordering::Relaxed);
    }

    /// Return a page to the idle pool. Drops it instead when:
    /// - `uses >= max_uses_per_page`
    /// - `created_at` is older than `page_ttl`
    /// - the idle queue for this ctx is already at capacity
    ///
    /// Always decrements the in-flight counter. Returns `true` if the
    /// page was kept, `false` if it was dropped.
    pub fn release(&self, pooled: PooledPage) -> bool {
        let ctx_key = pooled.ctx_key.clone();
        let ctrs = self.counters_for(&ctx_key);
        ctrs.in_flight.fetch_sub(1, Ordering::Relaxed);
        let now = Instant::now();
        if pooled.uses >= self.limits.max_uses_per_page
            || now.duration_since(pooled.created_at) > self.limits.page_ttl
        {
            return false;
        }
        let mut guard = self.idle.lock();
        let q = guard.entry(ctx_key).or_default();
        if q.len() >= self.limits.max_pages_per_context {
            // Pool full — drop the incoming page rather than evict a
            // sibling; keeps the LRU predictable.
            return false;
        }
        q.push_back(pooled);
        true
    }

    /// Decrement the in-flight counter without putting a page back into
    /// the pool. Used when a page was contaminated (challenge, fatal
    /// error) and the caller has already triggered `page.close()`.
    pub fn release_dirty_key(&self, ctx_key: &str) {
        let ctrs = self.counters_for(ctx_key);
        ctrs.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// Drop idle pages that have been sitting longer than `idle_ttl`.
    /// Returns the number of pages evicted.
    pub fn cleanup_idle(&self) -> usize {
        let now = Instant::now();
        let ttl = self.limits.idle_ttl;
        let mut guard = self.idle.lock();
        let mut evicted = 0usize;
        for q in guard.values_mut() {
            let before = q.len();
            q.retain(|p| now.duration_since(p.last_used) <= ttl);
            evicted += before - q.len();
        }
        evicted
    }

    /// Drop every idle page for a context (e.g. when the owning
    /// BrowserContext is being torn down).
    pub fn drop_context(&self, ctx_key: &str) {
        self.idle.lock().remove(ctx_key);
        self.counters.lock().remove(ctx_key);
    }

    /// Sum of in-flight counters across all contexts. Cheap: walks the
    /// counters map once.
    pub fn total_in_flight(&self) -> usize {
        self.counters
            .lock()
            .values()
            .map(|c| c.in_flight.load(Ordering::Relaxed))
            .sum()
    }

    pub fn total_idle(&self) -> usize {
        self.idle.lock().values().map(|q| q.len()).sum()
    }

    /// (created, reused) totals — used by metrics to report how often
    /// the pool actually reused a tab.
    pub fn totals(&self) -> (usize, usize) {
        let guard = self.counters.lock();
        let created: usize = guard
            .values()
            .map(|c| c.total_created.load(Ordering::Relaxed))
            .sum();
        let reused: usize = guard
            .values()
            .map(|c| c.total_reused.load(Ordering::Relaxed))
            .sum();
        (created, reused)
    }
}

/// RAII lease around a `PooledPage`. On normal completion call
/// `release_clean()` to return the page to the idle pool (or drop it
/// if it hit the ttl/uses limit). On error or challenge contamination
/// drop the lease without calling `release_clean` — the `Drop` impl
/// decrements the in-flight counter but does NOT put the page back
/// into the pool, so the next acquirer gets a fresh tab.
#[cfg(feature = "cdp-backend")]
pub struct PageLease {
    pool: Arc<PagePool>,
    pooled: Option<PooledPage>,
    ctx_key: String,
    /// Set to true by `release_clean` to suppress the dirty-release
    /// path in `Drop`.
    consumed: bool,
}

#[cfg(feature = "cdp-backend")]
impl PageLease {
    pub fn new(pool: Arc<PagePool>, pooled: PooledPage) -> Self {
        let ctx_key = pooled.ctx_key.clone();
        Self {
            pool,
            pooled: Some(pooled),
            ctx_key,
            consumed: false,
        }
    }

    pub fn page(&self) -> &Page {
        &self.pooled.as_ref().expect("pooled page present").page
    }

    /// Return the page to the pool. Must be called for healthy tabs.
    pub fn release_clean(mut self) -> bool {
        self.consumed = true;
        if let Some(p) = self.pooled.take() {
            self.pool.release(p)
        } else {
            false
        }
    }

    /// Mark the page as dirty. Caller is expected to have already
    /// triggered `page.close()`. The lease's Drop will decrement the
    /// in-flight counter without returning the page to the pool.
    pub fn release_dirty(mut self) {
        self.consumed = true;
        self.pooled = None;
        self.pool.release_dirty_key(&self.ctx_key);
    }
}

#[cfg(feature = "cdp-backend")]
impl Drop for PageLease {
    fn drop(&mut self) {
        if self.consumed {
            return;
        }
        // Treat any non-explicit drop as dirty — the caller short-
        // circuited via `?` on an error path.
        self.pooled.take();
        self.pool.release_dirty_key(&self.ctx_key);
    }
}

// ----- Test surface --------------------------------------------------
//
// The integration tests in `tests/page_pool.rs` can't cheaply create a
// real `chromiumoxide::Page` without launching Chrome. We expose a
// trait-free snapshot API that exercises the counter + limits logic by
// constructing the pool and driving release/acquire through a mock
// page type. In the mini build (where `chromiumoxide::Page` doesn't
// exist), none of this compiles.

#[cfg(all(test, feature = "cdp-backend"))]
mod tests {
    use super::*;

    #[test]
    fn limits_default_sane() {
        let l = PagePoolLimits::default();
        assert_eq!(l.max_pages_per_context, 4);
        assert_eq!(l.max_uses_per_page, 100);
        assert_eq!(l.page_ttl, Duration::from_secs(300));
        assert_eq!(l.idle_ttl, Duration::from_secs(30));
    }

    #[test]
    fn counters_isolated_per_ctx() {
        let pool = PagePool::new(PagePoolLimits::default());
        pool.register_fresh("ctx-a");
        pool.register_fresh("ctx-a");
        pool.register_fresh("ctx-b");
        let a = pool.counters_for("ctx-a");
        let b = pool.counters_for("ctx-b");
        assert_eq!(a.in_flight.load(Ordering::Relaxed), 2);
        assert_eq!(b.in_flight.load(Ordering::Relaxed), 1);
        pool.release_dirty_key("ctx-a");
        assert_eq!(a.in_flight.load(Ordering::Relaxed), 1);
    }
}
