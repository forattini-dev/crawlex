//! Small BFS-style driver around `JobRunner` for inspecting multi-page
//! behaviour while `Crawler::process_job` is still inline.
//!
//! Reads N URLs from the runner's `FetchSuccess.links`, dedupes, polite
//! 250 ms inter-request delay, stops at `--max <N>` (default 15).
//!
//! Run:
//!   cargo run --example runner_crawl --all-features
//!   cargo run --example runner_crawl --all-features -- https://news.ycombinator.com 15

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use crawlex::events::MemorySink;
use crawlex::impersonate::{ImpersonateClient, Profile};
use crawlex::queue::{FetchMethod, Job};
use crawlex::runner::{Fetcher, JobRunner, SessionContext, SpoofFetcher};

#[tokio::main]
async fn main() {
    let seed = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://news.ycombinator.com/".to_string());
    let max_pages: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);

    let seed_url: url::Url = seed.parse().expect("valid seed URL");
    let host = seed_url.host_str().unwrap_or("").to_string();

    let client = Arc::new(ImpersonateClient::new(Profile::Chrome131Stable).expect("client"));
    let spoof = Arc::new(SpoofFetcher::new(client));
    let events = Arc::new(MemorySink::create());
    let runner = JobRunner::new(spoof as Arc<dyn Fetcher>)
        .with_events(events.clone() as Arc<dyn crawlex::events::EventSink>);

    let mut queue: VecDeque<url::Url> = VecDeque::from(vec![seed_url]);
    let mut seen: HashSet<String> = HashSet::new();
    let mut visited: Vec<(String, usize, usize, Duration)> = Vec::new();
    let mut next_id: u64 = 1;

    println!("seed={seed}  max={max_pages}  host_filter={host}\n");

    while let Some(url) = queue.pop_front() {
        if visited.len() >= max_pages {
            break;
        }
        let url_str = url.to_string();
        if !seen.insert(url_str.clone()) {
            continue;
        }
        // Same-host only — keep it polite and bounded.
        if url.host_str().unwrap_or("") != host {
            continue;
        }

        let job = Job {
            id: next_id,
            crawl_id: 0,
            url: url.clone(),
            depth: 0,
            priority: 0,
            method: FetchMethod::HttpSpoof,
            attempts: 0,
            last_error: None,
        };
        next_id += 1;

        let started = std::time::Instant::now();
        let outcome = runner.run(&job, &SessionContext::default()).await;
        let wall = started.elapsed();

        match (&outcome.result, &outcome.error) {
            (Some(success), None) => {
                println!(
                    "[{:>2}] {:>3}  {:>5}B  {:>4} links  {:>6.1}ms   {url_str}",
                    visited.len() + 1,
                    success.status,
                    success.body_bytes,
                    success.links.len(),
                    wall.as_secs_f64() * 1000.0
                );
                visited.push((
                    url_str,
                    success.status as usize,
                    success.body_bytes,
                    wall,
                ));
                // Enqueue same-host children.
                for raw in &success.links {
                    if let Ok(u) = raw.parse::<url::Url>() {
                        if u.host_str().unwrap_or("") == host && !seen.contains(u.as_str()) {
                            queue.push_back(u);
                        }
                    }
                }
            }
            (_, Some(err)) => {
                println!(
                    "[{:>2}] ERR {:?}   {url_str}",
                    visited.len() + 1,
                    err
                );
                visited.push((url_str, 0, 0, wall));
            }
            _ => {}
        }

        // Polite.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    println!("\n=== Summary ===");
    println!("pages crawled : {}", visited.len());
    let total_bytes: usize = visited.iter().map(|(_, _, b, _)| b).sum();
    let total_wall: Duration = visited.iter().map(|(_, _, _, w)| *w).sum();
    println!("total bytes   : {} KB", total_bytes / 1024);
    println!(
        "total fetch   : {:.2}s  (avg {:.0}ms/page)",
        total_wall.as_secs_f64(),
        if visited.is_empty() { 0.0 } else { total_wall.as_secs_f64() * 1000.0 / visited.len() as f64 }
    );
    let event_count = events.take().len();
    println!("events fired  : {event_count}");
}
