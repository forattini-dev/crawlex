//! Performance KPIs — network timings for the HTTP path, Core Web Vitals and
//! runtime counters for the render path. Both populated per-page; stored in
//! `page_metrics` and exposed to hooks via `HookContext.user_data["metrics"]`.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Counters {
    pub requests_http: AtomicU64,
    pub requests_render: AtomicU64,
    pub pages_saved: AtomicU64,
    pub errors: AtomicU64,
    pub discovered_urls: AtomicU64,
    pub retries: AtomicU64,
    pub robots_blocked: AtomicU64,

    // ----- Fase 5: throughput --------------------------------------
    /// Gauges: tabs in-flight, active contexts, active browsers. Updated
    /// by `RenderPool` on every acquire/release.
    pub tabs_active: AtomicUsize,
    pub contexts_active: AtomicUsize,
    pub browsers_active: AtomicUsize,
    /// Total Chromium page-creation vs. reuse events. `reused / created`
    /// ratio surfaces how effective the PagePool is.
    pub pages_created: AtomicU64,
    pub pages_reused: AtomicU64,
    /// Budget rejections broken down by dimension. Incremented whenever
    /// `RenderBudgets::try_acquire` returns `Err(_)`.
    pub budget_rejections_host: AtomicU64,
    pub budget_rejections_origin: AtomicU64,
    pub budget_rejections_proxy: AtomicU64,
    pub budget_rejections_session: AtomicU64,
    /// Rolling render-latency + render-per-minute samples.
    pub render_samples: Mutex<RenderSamples>,
    /// Per-proxy challenge counter. Exposed so Prometheus can emit a
    /// gauge per proxy.
    pub challenges_per_proxy: Mutex<std::collections::HashMap<String, u64>>,
}

impl Counters {
    pub fn inc(&self, c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a completed render. `ok=true` means the render succeeded;
    /// `ok=false` is a render-level failure (timeout, CDP error). Always
    /// updates the latency histogram so p95 is comparable across mixes.
    pub fn record_render(&self, latency: Duration, ok: bool) {
        let mut s = self.render_samples.lock();
        s.push(latency.as_secs_f64() * 1_000.0, ok);
    }

    pub fn record_challenge(&self, proxy: Option<&url::Url>) {
        let key = proxy
            .map(|u| u.to_string())
            .unwrap_or_else(|| "_direct_".to_string());
        let mut g = self.challenges_per_proxy.lock();
        *g.entry(key).or_insert(0) += 1;
    }
}

/// Fixed-window rolling sample store. Keeps the last `window_secs` of
/// (timestamp, latency_ms, ok) rows. Sized so we never grow unbounded
/// even under heavy load: `cap` entries evict oldest first.
pub struct RenderSamples {
    window: Duration,
    samples: std::collections::VecDeque<(Instant, f64, bool)>,
    cap: usize,
}

impl Default for RenderSamples {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(60),
            samples: std::collections::VecDeque::with_capacity(512),
            cap: 10_000,
        }
    }
}

impl RenderSamples {
    fn evict(&mut self, now: Instant) {
        let cutoff = now - self.window;
        while let Some((t, _, _)) = self.samples.front() {
            if *t < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        while self.samples.len() > self.cap {
            self.samples.pop_front();
        }
    }

    fn push(&mut self, latency_ms: f64, ok: bool) {
        let now = Instant::now();
        self.evict(now);
        self.samples.push_back((now, latency_ms, ok));
    }

    /// Renders completed within the rolling window. Name kept generic
    /// so callers don't assume the window length (default 60s).
    pub fn renders_per_window(&mut self) -> usize {
        let now = Instant::now();
        self.evict(now);
        self.samples.len()
    }

    /// Approximate percentile from the rolling window. Sorts a clone of
    /// the latency vector — O(n log n) per scrape, which is fine: we
    /// serve metrics only on Prometheus scrapes, not in the hot path.
    pub fn percentile(&mut self, p: f64) -> Option<f64> {
        let now = Instant::now();
        self.evict(now);
        if self.samples.is_empty() {
            return None;
        }
        let mut lats: Vec<f64> = self.samples.iter().map(|(_, l, _)| *l).collect();
        lats.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((lats.len() as f64 - 1.0) * p).round() as usize;
        Some(lats[idx.min(lats.len() - 1)])
    }
}

/// Per-request network timings (HTTP spoof path). All durations are in
/// milliseconds, measured on the client side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkTimings {
    pub dns_ms: Option<u64>,
    pub tcp_connect_ms: Option<u64>,
    pub tls_handshake_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub download_ms: Option<u64>,
    pub total_ms: Option<u64>,
    pub status: Option<u16>,
    pub bytes: Option<u64>,
    pub alpn: Option<String>,
    pub tls_version: Option<String>,
    pub cipher: Option<String>,
}

/// Core Web Vitals + runtime metrics collected from a real browser render.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebVitals {
    pub nav_start_ms: Option<f64>,
    pub dom_content_loaded_ms: Option<f64>,
    pub load_event_ms: Option<f64>,
    pub first_paint_ms: Option<f64>,
    pub first_contentful_paint_ms: Option<f64>,
    pub largest_contentful_paint_ms: Option<f64>,
    pub cumulative_layout_shift: Option<f64>,
    pub total_blocking_time_ms: Option<f64>,
    pub time_to_interactive_ms: Option<f64>,
    pub interaction_to_next_paint_ms: Option<f64>,
    pub dom_nodes: Option<u64>,
    pub js_heap_used_bytes: Option<u64>,
    pub js_heap_total_bytes: Option<u64>,
    pub resource_count: Option<u64>,
    pub total_transfer_bytes: Option<u64>,
    pub total_decoded_bytes: Option<u64>,
    pub transfer_by_type: Option<std::collections::HashMap<String, u64>>,
    pub longest_task_ms: Option<f64>,
}

/// Per-resource network waterfall harvested via CDP Network events.
/// All millisecond-valued fields are offsets from `request_time` (seconds
/// since epoch), matching the Chrome DevTools Protocol semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceSample {
    pub url: String,
    pub mime_type: Option<String>,
    pub resource_type: Option<String>,
    pub status: Option<u16>,
    pub from_cache: Option<bool>,
    pub protocol: Option<String>,
    pub remote_ip: Option<String>,
    pub remote_port: Option<u16>,
    pub encoded_data_length: Option<f64>,
    pub transfer_size: Option<f64>,

    // Raw timing ticks (ms since request_time).
    pub request_time: Option<f64>,
    pub dns_start: Option<f64>,
    pub dns_end: Option<f64>,
    pub connect_start: Option<f64>,
    pub connect_end: Option<f64>,
    pub ssl_start: Option<f64>,
    pub ssl_end: Option<f64>,
    pub send_start: Option<f64>,
    pub send_end: Option<f64>,
    pub receive_headers_start: Option<f64>,
    pub receive_headers_end: Option<f64>,
    pub loading_finished_ms: Option<f64>,

    // Derived per-phase durations (ms) — nulls if we couldn't compute.
    pub dns_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub ssl_ms: Option<f64>,
    pub send_ms: Option<f64>,
    pub wait_ms: Option<f64>,
    pub receive_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageMetrics {
    pub net: NetworkTimings,
    pub vitals: WebVitals,
    pub resources: Vec<ResourceSample>,
}

/// JavaScript snippet evaluated in-page after wait strategy completes. Returns
/// a JSON object the Rust side deserializes into `WebVitals`. Uses
/// PerformanceObserver for modern vitals + `performance.timing` for legacy
/// numbers. Always returns an object (never throws).
/// Script that simulates a user click at viewport center and measures the
/// time from input dispatch to next animation frame — approximating INP /
/// responsiveness. Returns null if the page doesn't respond within 1s.
pub const INP_PROBE_JS: &str = r#"
(async () => {
  try {
    const x = Math.floor(window.innerWidth / 2);
    const y = Math.floor(window.innerHeight / 2);
    const evInit = { bubbles: true, cancelable: true, view: window, clientX: x, clientY: y };
    const target = document.elementFromPoint(x, y) || document.body;
    const t0 = performance.now();
    target.dispatchEvent(new PointerEvent('pointerdown', evInit));
    target.dispatchEvent(new MouseEvent('mousedown', evInit));
    target.dispatchEvent(new PointerEvent('pointerup', evInit));
    target.dispatchEvent(new MouseEvent('mouseup', evInit));
    target.dispatchEvent(new MouseEvent('click', evInit));
    const nextFrame = await new Promise(r => {
      const done = () => r(performance.now());
      requestAnimationFrame(() => requestAnimationFrame(done));
      setTimeout(() => r(null), 1000);
    });
    if (nextFrame == null) return null;
    return nextFrame - t0;
  } catch (_) { return null; }
})()
"#;

pub const WEB_VITALS_JS: &str = r#"
(async () => {
  const safe = (fn, def) => { try { return fn(); } catch (_) { return def; } };
  const t = (performance.timing && performance.timing.navigationStart) ? performance.timing : null;
  const getEntries = (type) => safe(() => performance.getEntriesByType(type), []);
  const navEntry = safe(() => performance.getEntriesByType('navigation')[0], null);

  // Core timings (ms since navigationStart).
  const nav_start_ms = t ? t.navigationStart : 0;
  const dcl = navEntry ? navEntry.domContentLoadedEventEnd
           : (t ? t.domContentLoadedEventEnd - nav_start_ms : null);
  const loaded = navEntry ? navEntry.loadEventEnd
           : (t ? t.loadEventEnd - nav_start_ms : null);

  // Paint Timing: prefer the observer-populated value (captures events before
  // this script runs), fall back to entry table.
  const paints = getEntries('paint');
  let fp = null, fcp = null;
  for (const p of paints) {
    if (p.name === 'first-paint') fp = p.startTime;
    if (p.name === 'first-contentful-paint') fcp = p.startTime;
  }
  if (fcp == null && typeof window.__mb_fcp === 'number') fcp = window.__mb_fcp;

  // LCP / CLS / TBT / longest — observer values if present (installed before
  // load), else best-effort from entry buffers.
  let lcp = (typeof window.__mb_lcp === 'number' && window.__mb_lcp > 0)
    ? window.__mb_lcp : null;
  if (lcp == null) {
    try {
      const lcpEntries = getEntries('largest-contentful-paint');
      if (lcpEntries.length) lcp = lcpEntries[lcpEntries.length - 1].startTime;
    } catch (_) {}
  }
  let cls = (typeof window.__mb_cls === 'number') ? window.__mb_cls : 0;
  if (cls === 0) {
    try {
      for (const ls of getEntries('layout-shift')) {
        if (!ls.hadRecentInput) cls += ls.value;
      }
    } catch (_) {}
  }
  let tbt = (typeof window.__mb_tbt === 'number') ? window.__mb_tbt : 0;
  if (tbt === 0) {
    try {
      for (const lt of getEntries('longtask')) {
        const blocking = lt.duration - 50;
        if (blocking > 0 && (fcp == null || lt.startTime >= fcp)) tbt += blocking;
      }
    } catch (_) {}
  }
  let longest = (typeof window.__mb_longest === 'number') ? window.__mb_longest : 0;

  // Resource timings aggregated by initiatorType.
  let resource_count = 0;
  let transfer = 0;
  let decoded = 0;
  const by_type = {};
  try {
    for (const r of getEntries('resource')) {
      resource_count++;
      transfer += r.transferSize || 0;
      decoded += r.decodedBodySize || 0;
      const k = r.initiatorType || 'other';
      by_type[k] = (by_type[k] || 0) + (r.transferSize || 0);
    }
  } catch (_) {}

  // DOM size.
  let dom_nodes = 0;
  try { dom_nodes = document.getElementsByTagName('*').length; } catch (_) {}

  // JS heap — Chrome only, not in standards.
  const mem = safe(() => performance.memory, null);

  return {
    nav_start_ms,
    dom_content_loaded_ms: dcl,
    load_event_ms: loaded,
    first_paint_ms: fp,
    first_contentful_paint_ms: fcp,
    largest_contentful_paint_ms: lcp,
    cumulative_layout_shift: cls,
    total_blocking_time_ms: tbt,
    longest_task_ms: longest,
    dom_nodes,
    js_heap_used_bytes: mem ? mem.usedJSHeapSize : null,
    js_heap_total_bytes: mem ? mem.totalJSHeapSize : null,
    resource_count,
    total_transfer_bytes: transfer,
    total_decoded_bytes: decoded,
    transfer_by_type: by_type,
  };
})()
"#;
