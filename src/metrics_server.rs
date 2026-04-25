//! Minimal Prometheus scrape endpoint using a hand-rolled HTTP/1.0 responder.
//!
//! We deliberately avoid pulling in an HTTP server framework — the prom
//! exposition format is trivial text and this keeps the binary lean. Serves a
//! single endpoint (`/metrics`) on a TCP port, ignores method/query, echoes
//! counter values read from a shared `Counters`.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::metrics::Counters;

pub async fn serve(port: u16, counters: Arc<Counters>) -> std::io::Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "prometheus metrics listening");
    loop {
        let (mut sock, _) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(?e, "accept failed");
                continue;
            }
        };
        let c = counters.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let body = render(&c);
            let resp = format!(
                "HTTP/1.0 200 OK\r\n\
                 Content-Type: text/plain; version=0.0.4\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        });
    }
}

fn render(c: &Counters) -> String {
    let mut out = String::new();
    let pairs: &[(&str, &str, &std::sync::atomic::AtomicU64)] = &[
        (
            "crawlex_requests_http_total",
            "HTTP-spoof requests dispatched",
            &c.requests_http,
        ),
        (
            "crawlex_requests_render_total",
            "Chrome render requests dispatched",
            &c.requests_render,
        ),
        (
            "crawlex_pages_saved_total",
            "Pages persisted to storage",
            &c.pages_saved,
        ),
        ("crawlex_errors_total", "Request errors", &c.errors),
        (
            "crawlex_discovered_urls_total",
            "URLs discovered and enqueued",
            &c.discovered_urls,
        ),
        ("crawlex_retries_total", "Job retries scheduled", &c.retries),
        (
            "crawlex_robots_blocked_total",
            "Requests blocked by robots.txt",
            &c.robots_blocked,
        ),
        (
            "crawlex_pages_created_total",
            "Chrome tabs created (fresh)",
            &c.pages_created,
        ),
        (
            "crawlex_pages_reused_total",
            "Chrome tabs reused from the PagePool",
            &c.pages_reused,
        ),
        (
            "crawlex_budget_rejections_host_total",
            "Render jobs deferred due to per-host inflight budget",
            &c.budget_rejections_host,
        ),
        (
            "crawlex_budget_rejections_origin_total",
            "Render jobs deferred due to per-origin inflight budget",
            &c.budget_rejections_origin,
        ),
        (
            "crawlex_budget_rejections_proxy_total",
            "Render jobs deferred due to per-proxy inflight budget",
            &c.budget_rejections_proxy,
        ),
        (
            "crawlex_budget_rejections_session_total",
            "Render jobs deferred due to per-session inflight budget",
            &c.budget_rejections_session,
        ),
    ];
    for (name, help, value) in pairs {
        out.push_str(&format!("# HELP {name} {help}\n"));
        out.push_str(&format!("# TYPE {name} counter\n"));
        out.push_str(&format!("{name} {}\n", value.load(Ordering::Relaxed)));
    }

    let gauges: &[(&str, &str, &std::sync::atomic::AtomicUsize)] = &[
        (
            "crawlex_tabs_active",
            "Chrome tabs currently in-flight",
            &c.tabs_active,
        ),
        (
            "crawlex_contexts_active",
            "BrowserContexts currently live",
            &c.contexts_active,
        ),
        (
            "crawlex_browsers_active",
            "Chrome processes currently alive",
            &c.browsers_active,
        ),
    ];
    for (name, help, value) in gauges {
        out.push_str(&format!("# HELP {name} {help}\n"));
        out.push_str(&format!("# TYPE {name} gauge\n"));
        out.push_str(&format!("{name} {}\n", value.load(Ordering::Relaxed)));
    }

    // Rolling latency + throughput derived from RenderSamples.
    {
        let mut s = c.render_samples.lock();
        let rpm = s.renders_per_window();
        let p50 = s.percentile(0.50).unwrap_or(0.0);
        let p95 = s.percentile(0.95).unwrap_or(0.0);
        let p99 = s.percentile(0.99).unwrap_or(0.0);
        out.push_str(
            "# HELP crawlex_renders_per_min Renders completed in the rolling 60s window\n",
        );
        out.push_str("# TYPE crawlex_renders_per_min gauge\n");
        out.push_str(&format!("crawlex_renders_per_min {rpm}\n"));
        for (q, v) in [("p50", p50), ("p95", p95), ("p99", p99)] {
            let name = format!("crawlex_render_latency_ms_{q}");
            out.push_str(&format!(
                "# HELP {name} Render latency (ms) — {q} over the rolling 60s window\n"
            ));
            out.push_str(&format!("# TYPE {name} gauge\n"));
            out.push_str(&format!("{name} {v}\n"));
        }
    }

    // Per-proxy challenges (labelled gauge).
    {
        let g = c.challenges_per_proxy.lock();
        if !g.is_empty() {
            out.push_str(
                "# HELP crawlex_challenges_per_proxy_total Antibot challenges observed per proxy\n",
            );
            out.push_str("# TYPE crawlex_challenges_per_proxy_total counter\n");
            for (proxy, count) in g.iter() {
                let safe = proxy.replace('"', "\\\"");
                out.push_str(&format!(
                    "crawlex_challenges_per_proxy_total{{proxy=\"{safe}\"}} {count}\n"
                ));
            }
        }
    }
    out
}
