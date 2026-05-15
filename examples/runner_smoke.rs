//! Smoke-test driver for the `JobRunner` seam (PRD #15).
//!
//! Constructs a `JobRunner` over a `SpoofFetcher`, runs one Job, and
//! prints the `JobOutcome`. Useful for manual inspection while the
//! `Crawler::process_job` cutover is pending — confirms the runner
//! types compose and produce sensible outcomes against a real URL.
//!
//! Run: `cargo run --example runner_smoke --all-features -- <url>`
//! Default URL: https://example.com/

use std::sync::Arc;

use crawlex::events::MemorySink;
use crawlex::impersonate::{ImpersonateClient, Profile};
use crawlex::queue::{FetchMethod, Job};
use crawlex::runner::{Fetcher, JobRunner, SessionContext, SpoofFetcher};

#[tokio::main]
async fn main() {
    let url_arg = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com/".to_string());
    let url: url::Url = url_arg.parse().expect("valid URL");

    let client = Arc::new(ImpersonateClient::new(Profile::Chrome131Stable).expect("client"));
    let spoof = Arc::new(SpoofFetcher::new(client));
    let events = Arc::new(MemorySink::create());

    let runner = JobRunner::new(spoof as Arc<dyn Fetcher>)
        .with_events(events.clone() as Arc<dyn crawlex::events::EventSink>);

    let job = Job {
        id: 1,
        crawl_id: 0,
        url: url.clone(),
        depth: 0,
        priority: 0,
        method: FetchMethod::HttpSpoof,
        attempts: 0,
        last_error: None,
    };

    println!("→ runner.run({url}) ...");
    let outcome = runner.run(&job, &SessionContext::default()).await;

    println!("\n=== JobOutcome ===");
    if let Some(success) = &outcome.result {
        println!("status        : {}", success.status);
        println!("body_bytes    : {}", success.body_bytes);
        println!("links found   : {}", success.links.len());
        for (i, link) in success.links.iter().take(10).enumerate() {
            println!("  [{i}] {link}");
        }
        if success.links.len() > 10 {
            println!("  ... ({} more)", success.links.len() - 10);
        }
        println!("challenge sig : {}", success.signals.len());
    }
    if let Some(err) = &outcome.error {
        println!("error         : {err:?}");
    }
    println!("retry         : {:?}", outcome.retry);
    println!("timings       : {:?}", outcome.timings);

    println!("\n=== Lifecycle events (per-attempt subset from JobRunner) ===");
    for ev in events.take() {
        println!("  {:?}  {}", ev.event, ev.url.as_deref().unwrap_or(""));
    }
}
