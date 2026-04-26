//! Embedded use of the `crawlex` library with native Rust hooks.
//!
//! Run:
//!   cargo run --example embedded_with_hooks --features cli,sqlite -- https://example.com
//!
//! The example:
//!   * Builds a memory-only Crawler (no on-disk artifacts).
//!   * Registers four typed Rust hooks demonstrating the core
//!     interventions: short-circuit (`Skip`), retry, header mutation,
//!     and discovery extension.
//!   * Streams NDJSON events on stdout via the default sink.
//!
//! No Lua, no JS, no separate binary — every hook is a normal Rust
//! `async` closure that holds whatever state it captures.

use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crawlex::hooks::{HookDecision, HookRegistry};
use crawlex::queue::FetchMethod;
use crawlex::{Config, Crawler, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let seed = env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".into());
    let seed = url::Url::parse(&seed).expect("invalid seed url");

    // ─── 1. Build a hook registry ────────────────────────────────────
    let hooks = HookRegistry::new();

    // (a) Short-circuit on robots.txt deny so the rest of the pipeline
    //     never touches the URL — counts skipped jobs into a closure
    //     captured `Arc<AtomicUsize>` we can read after the run.
    let skipped = Arc::new(AtomicUsize::new(0));
    let skipped_count = skipped.clone();
    hooks.on_before_each_request(move |ctx| {
        let skipped = skipped.clone();
        Box::pin(async move {
            if ctx.url.path().contains("/private/") {
                skipped.fetch_add(1, Ordering::SeqCst);
                return Ok(HookDecision::Skip);
            }
            Ok(HookDecision::Continue)
        })
    });

    // (b) Retry on rate-limit / temporary-failure status codes. The
    //     pipeline honours `Retry` only when `ctx.allow_retry` is true
    //     (it is, by default, for the first 3 attempts).
    hooks.on_after_first_byte(|ctx| {
        let status = ctx.response_status;
        Box::pin(async move {
            match status {
                Some(429) | Some(503) => Ok(HookDecision::Retry),
                _ => Ok(HookDecision::Continue),
            }
        })
    });

    // (c) Synthesise extra URLs from the page DOM. `ctx.captured_urls`
    //     is a mutable `Vec<Url>` populated by the link extractor before
    //     OnDiscovery fires — push to extend, drain to suppress.
    hooks.on_discovery(|ctx| {
        Box::pin(async move {
            // Try the canonical sitemap location for every host we
            // visit. The dedup layer drops duplicates so this is safe
            // to call unconditionally.
            if let Some(host) = ctx.url.host_str() {
                if let Ok(sitemap) = url::Url::parse(&format!("https://{host}/sitemap.xml")) {
                    ctx.captured_urls.push(sitemap);
                }
            }
            Ok(HookDecision::Continue)
        })
    });

    // (d) Tag every page with a custom marker the pipeline will fold
    //     into the `tech_fingerprint` table via `user_data`.
    hooks.on_response_body(|ctx| {
        Box::pin(async move {
            ctx.user_data.insert(
                "tagged_by_example".into(),
                serde_json::Value::String("embedded_with_hooks".into()),
            );
            Ok(HookDecision::Continue)
        })
    });

    // ─── 2. Build + run the crawler ─────────────────────────────────
    let config = Config::builder().max_concurrent_http(4).build()?;

    let crawler = Crawler::new(config)?.with_hooks(hooks);
    crawler
        .seed_with(vec![seed], FetchMethod::HttpSpoof)
        .await?;
    crawler.run().await?;

    eprintln!(
        "embedded_with_hooks done — skipped {} private URLs",
        skipped_count.load(Ordering::SeqCst)
    );
    Ok(())
}
