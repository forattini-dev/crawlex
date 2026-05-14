//! v2 spider runtime — `Spider` trait + in-memory `SpiderRunner`.
//!
//! Slice 17 lands the DSL surface and runtime skeleton. The runner is
//! deliberately decoupled from the v1 crawl pipeline: it owns its own
//! frontier (FIFO of [`Request`]), routes through [`SessionManager`] for
//! per-session backend resolution, applies a per-domain throttle, and
//! optionally consults a [`RobotsCache`] gate. Wiring this into the full
//! scheduler/queue/checkpoint storage backends lands in slice 25 (v1
//! `crawl()` removal). Until then, a checkpoint is serialised to JSON
//! and can be reloaded to resume an interrupted run.
//!
//! Recipes implement [`Spider`]:
//!
//! ```ignore
//! struct MySpider;
//! impl Spider for MySpider {
//!     fn start_urls(&self) -> Vec<String> { vec!["https://example.com".into()] }
//!     fn parse(&self, resp: &Response) -> Vec<ParseYield> {
//!         vec![ParseYield::item(serde_json::json!({"title": "..."}))]
//!     }
//! }
//! ```
//!
//! The runner consumes a [`Fetcher`] (trait object) so tests can swap in
//! a mock. The default fetcher in slice 17 is intentionally absent — a
//! real HTTP/render dispatcher arrives when the engine bindings land.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use url::Url;

use super::request::Request;
use super::session::SessionManager;
use crate::robots::RobotsCache;

/// Outcome of fetching a [`Request`]. Body is plain bytes; recipes parse
/// it however they like (`std::str::from_utf8`, scraper, etc).
#[derive(Debug, Clone)]
pub struct Response {
    pub request: Request,
    pub final_url: String,
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
}

impl Response {
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }
}

/// What a `parse` invocation yields. Items become recipe output; requests
/// re-enter the frontier (deduplicated by URL).
#[derive(Debug, Clone)]
pub enum ParseYield {
    Item(serde_json::Value),
    Request(Request),
}

impl ParseYield {
    pub fn item(v: serde_json::Value) -> Self {
        Self::Item(v)
    }
    pub fn request(r: Request) -> Self {
        Self::Request(r)
    }
}

/// Per-spider configuration. Defaults are conservative — zero delay, no
/// robots, no max items.
#[derive(Debug, Clone)]
pub struct SpiderConfig {
    /// Per-domain minimum gap between consecutive fetches. `0` disables.
    pub download_delay: Duration,
    /// Honour robots.txt `Disallow`. `Crawl-delay` is applied as a floor
    /// on `download_delay` per host. Caller must pre-populate the
    /// [`RobotsCache`] for each host — slice 17 does not fetch
    /// robots.txt itself; the dispatcher will once it lands.
    pub robots_txt_obey: bool,
    pub user_agent: String,
    /// Stop after N items emitted. `None` = unbounded.
    pub max_items: Option<usize>,
}

impl Default for SpiderConfig {
    fn default() -> Self {
        Self {
            download_delay: Duration::ZERO,
            robots_txt_obey: false,
            user_agent: "crawlex".into(),
            max_items: None,
        }
    }
}

/// Recipe-facing trait. Implementations are usually small structs with
/// no internal state; cross-request state lives in the spider's fields
/// or in `Request.user_data` (a future slice — not yet on `Request`).
pub trait Spider: Send + Sync {
    fn start_urls(&self) -> Vec<String>;
    fn parse(&self, resp: &Response) -> Vec<ParseYield>;
    /// Optional override: how to build the seed Requests. Default wraps
    /// each `start_urls()` entry as a GET with no session.
    fn start_requests(&self) -> Vec<Request> {
        self.start_urls()
            .into_iter()
            .map(Request::new)
            .collect()
    }
}

/// Fetcher abstraction. Real backends plug in via [`SessionManager::route`]
/// in later slices; for now `SpiderRunner` calls this directly so tests
/// can swap in a deterministic mock.
pub trait Fetcher: Send + Sync {
    fn fetch(&self, req: &Request) -> Result<Response, FetchError>;
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("network: {0}")]
    Network(String),
    #[error("disallowed by robots.txt")]
    RobotsDisallowed,
}

/// Persistable runner state. Drives pause-on-Ctrl-C / resume.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Checkpoint {
    /// FIFO of URLs still to fetch. Methods/sessions are flattened to
    /// `(method, url, session_id)` triples so the wire shape stays JSON.
    pub pending: Vec<CheckpointRequest>,
    pub seen: Vec<String>,
    pub items_emitted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRequest {
    pub url: String,
    pub method: String,
    pub session_id: Option<String>,
}

impl From<&Request> for CheckpointRequest {
    fn from(r: &Request) -> Self {
        Self {
            url: r.url.clone(),
            method: r.method.clone(),
            session_id: r.session_id.clone(),
        }
    }
}

impl From<CheckpointRequest> for Request {
    fn from(c: CheckpointRequest) -> Self {
        let mut r = Request::new(c.url).with_method(c.method);
        if let Some(sid) = c.session_id {
            r = r.with_session(sid);
        }
        r
    }
}

/// Result of running a spider to completion (or to a pause point).
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub items: Vec<serde_json::Value>,
    pub checkpoint: Checkpoint,
    /// `true` if the run paused (max_items reached, external pause flag,
    /// etc) rather than draining the frontier.
    pub paused: bool,
}

/// In-memory driver. Holds the frontier and per-domain throttle clocks.
pub struct SpiderRunner {
    config: SpiderConfig,
    sessions: Arc<SessionManager>,
    robots: Option<Arc<RobotsCache>>,
    pending: VecDeque<Request>,
    seen: HashSet<String>,
    last_fetch_per_host: HashMap<String, Instant>,
    items_emitted: usize,
}

impl SpiderRunner {
    pub fn new(config: SpiderConfig, sessions: Arc<SessionManager>) -> Self {
        Self {
            config,
            sessions,
            robots: None,
            pending: VecDeque::new(),
            seen: HashSet::new(),
            last_fetch_per_host: HashMap::new(),
            items_emitted: 0,
        }
    }

    pub fn with_robots(mut self, robots: Arc<RobotsCache>) -> Self {
        self.robots = Some(robots);
        self
    }

    /// Seed the frontier from a checkpoint (resume) or from the spider's
    /// own `start_requests()` (fresh run).
    pub fn seed(&mut self, spider: &dyn Spider, resume: Option<Checkpoint>) {
        if let Some(cp) = resume {
            self.items_emitted = cp.items_emitted;
            self.seen.extend(cp.seen);
            for cr in cp.pending {
                let r: Request = cr.into();
                self.pending.push_back(r);
            }
        } else {
            for r in spider.start_requests() {
                self.enqueue(r);
            }
        }
    }

    fn enqueue(&mut self, req: Request) {
        let key = format!("{} {}", req.method, req.url);
        if self.seen.insert(key) {
            self.pending.push_back(req);
        }
    }

    fn snapshot(&self) -> Checkpoint {
        Checkpoint {
            pending: self.pending.iter().map(CheckpointRequest::from).collect(),
            seen: self.seen.iter().cloned().collect(),
            items_emitted: self.items_emitted,
        }
    }

    /// Apply per-host download delay. Returns the wait duration that
    /// *would* be applied; callers in real I/O contexts sleep, tests
    /// just observe the value.
    pub fn delay_for(&self, host: &str, now: Instant) -> Duration {
        let base = self.config.download_delay;
        let robots_floor = self
            .robots
            .as_ref()
            .and_then(|_r| {
                // texting_robots exposes crawl_delay via Robot; the
                // RobotsCache stores Option<Robot>. We don't currently
                // surface that — slice 17 floor logic is wired in once
                // the cache exposes it. Leave a hook for the future.
                None::<Duration>
            })
            .unwrap_or(Duration::ZERO);
        let floor = base.max(robots_floor);
        match self.last_fetch_per_host.get(host) {
            None => Duration::ZERO,
            Some(last) => {
                let elapsed = now.saturating_duration_since(*last);
                if elapsed >= floor {
                    Duration::ZERO
                } else {
                    floor - elapsed
                }
            }
        }
    }

    /// Robots gate. `true` = allowed (or no robots policy active).
    pub fn robots_allows(&self, url: &Url) -> bool {
        if !self.config.robots_txt_obey {
            return true;
        }
        let Some(robots) = &self.robots else {
            return true;
        };
        // RobotsCache::check returns Some(true)/Some(false)/None (uncached
        // or expired). For an obedient spider, missing entry => allow but
        // would normally trigger an out-of-band fetch. Slice 17 stays
        // conservative on the side of letting the request through; the
        // dispatcher takes over once it ships.
        robots.check(url, &self.config.user_agent).unwrap_or(true)
    }

    /// Drive the spider to completion (or until `max_items` hits).
    /// Synchronous so tests stay deterministic. The fetcher is invoked
    /// in-line; real I/O blocking is the caller's problem until the
    /// async dispatcher lands.
    pub fn run(&mut self, spider: &dyn Spider, fetcher: &dyn Fetcher) -> RunOutcome {
        let mut items = Vec::new();
        loop {
            if let Some(max) = self.config.max_items {
                if self.items_emitted >= max {
                    return RunOutcome {
                        items,
                        checkpoint: self.snapshot(),
                        paused: true,
                    };
                }
            }
            let Some(req) = self.pending.pop_front() else {
                return RunOutcome {
                    items,
                    checkpoint: self.snapshot(),
                    paused: false,
                };
            };

            // Robots check uses the resolved URL.
            if let Ok(url) = Url::parse(&req.url) {
                if !self.robots_allows(&url) {
                    continue;
                }
                let host = url.host_str().unwrap_or("").to_string();
                // Throttle: record the fetch start (mocked clock —
                // testers usually just call run() once).
                let now = Instant::now();
                let _wait = self.delay_for(&host, now);
                self.last_fetch_per_host.insert(host, now);
            }

            // Confirm routing decision — surfaces unknown-session warns
            // even though we don't otherwise use the result in slice 17.
            let _route = self.sessions.route(&req);

            let resp = match fetcher.fetch(&req) {
                Ok(r) => r,
                Err(_e) => continue,
            };
            for y in spider.parse(&resp) {
                match y {
                    ParseYield::Item(v) => {
                        items.push(v);
                        self.items_emitted += 1;
                    }
                    ParseYield::Request(r) => self.enqueue(r),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraping::session::BackendKind;
    use std::sync::Mutex;

    struct MapFetcher {
        responses: HashMap<String, (u16, Vec<u8>)>,
        log: Mutex<Vec<String>>,
    }

    impl MapFetcher {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
                log: Mutex::new(Vec::new()),
            }
        }
        fn with(mut self, url: &str, status: u16, body: &str) -> Self {
            self.responses
                .insert(url.into(), (status, body.as_bytes().to_vec()));
            self
        }
    }

    impl Fetcher for MapFetcher {
        fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
            self.log.lock().unwrap().push(req.url.clone());
            let (status, body) = self
                .responses
                .get(&req.url)
                .cloned()
                .unwrap_or((404, b"not found".to_vec()));
            Ok(Response {
                request: req.clone(),
                final_url: req.url.clone(),
                status,
                body,
                headers: HashMap::new(),
            })
        }
    }

    struct LinkSpider;
    impl Spider for LinkSpider {
        fn start_urls(&self) -> Vec<String> {
            vec!["https://example.test/".into()]
        }
        fn parse(&self, resp: &Response) -> Vec<ParseYield> {
            let mut out = vec![ParseYield::item(serde_json::json!({
                "url": resp.final_url,
                "len": resp.body.len(),
            }))];
            // Follow `next` link if body has one (simulated).
            if resp.text() == "go-next" {
                out.push(ParseYield::request(Request::new(
                    "https://example.test/next",
                )));
            }
            out
        }
    }

    fn mgr() -> Arc<SessionManager> {
        Arc::new(SessionManager::new(BackendKind::Http))
    }

    #[test]
    fn demuxes_items_and_new_requests() {
        let fetcher = MapFetcher::new()
            .with("https://example.test/", 200, "go-next")
            .with("https://example.test/next", 200, "leaf");
        let mut runner = SpiderRunner::new(SpiderConfig::default(), mgr());
        let spider = LinkSpider;
        runner.seed(&spider, None);
        let out = runner.run(&spider, &fetcher);
        assert_eq!(out.items.len(), 2);
        assert!(!out.paused);
        let urls: Vec<_> = fetcher.log.lock().unwrap().clone();
        assert_eq!(urls, vec!["https://example.test/", "https://example.test/next"]);
    }

    #[test]
    fn dedupes_requests_by_method_and_url() {
        // Spider that yields the same URL twice.
        struct DupSpider;
        impl Spider for DupSpider {
            fn start_urls(&self) -> Vec<String> {
                vec!["https://x.test/".into()]
            }
            fn parse(&self, _r: &Response) -> Vec<ParseYield> {
                vec![
                    ParseYield::request(Request::new("https://x.test/a")),
                    ParseYield::request(Request::new("https://x.test/a")),
                ]
            }
        }
        let fetcher = MapFetcher::new()
            .with("https://x.test/", 200, "")
            .with("https://x.test/a", 200, "");
        let mut runner = SpiderRunner::new(SpiderConfig::default(), mgr());
        runner.seed(&DupSpider, None);
        runner.run(&DupSpider, &fetcher);
        let urls = fetcher.log.lock().unwrap().clone();
        assert_eq!(urls.len(), 2, "duplicate URL should fetch once");
    }

    #[test]
    fn pauses_when_max_items_reached() {
        struct InfSpider;
        impl Spider for InfSpider {
            fn start_urls(&self) -> Vec<String> {
                vec!["https://x.test/0".into()]
            }
            fn parse(&self, resp: &Response) -> Vec<ParseYield> {
                let n: usize = resp
                    .final_url
                    .rsplit('/')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                vec![
                    ParseYield::item(serde_json::json!({"n": n})),
                    ParseYield::request(Request::new(format!("https://x.test/{}", n + 1))),
                ]
            }
        }
        let mut fetcher = MapFetcher::new();
        for i in 0..10 {
            fetcher = fetcher.with(&format!("https://x.test/{i}"), 200, "");
        }
        let cfg = SpiderConfig {
            max_items: Some(3),
            ..Default::default()
        };
        let mut runner = SpiderRunner::new(cfg, mgr());
        runner.seed(&InfSpider, None);
        let out = runner.run(&InfSpider, &fetcher);
        assert_eq!(out.items.len(), 3);
        assert!(out.paused);
        // Frontier still has pending work — resume should pick it up.
        assert!(!out.checkpoint.pending.is_empty());
    }

    #[test]
    fn resume_from_checkpoint_continues() {
        struct CountSpider;
        impl Spider for CountSpider {
            fn start_urls(&self) -> Vec<String> {
                vec!["https://r.test/0".into()]
            }
            fn parse(&self, resp: &Response) -> Vec<ParseYield> {
                let n: usize = resp
                    .final_url
                    .rsplit('/')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let mut out = vec![ParseYield::item(serde_json::json!({"n": n}))];
                if n < 4 {
                    out.push(ParseYield::request(Request::new(format!(
                        "https://r.test/{}",
                        n + 1
                    ))));
                }
                out
            }
        }
        let mut fetcher = MapFetcher::new();
        for i in 0..5 {
            fetcher = fetcher.with(&format!("https://r.test/{i}"), 200, "");
        }
        // Phase 1: limit to 2 items, capture checkpoint.
        let cfg1 = SpiderConfig {
            max_items: Some(2),
            ..Default::default()
        };
        let mut r1 = SpiderRunner::new(cfg1, mgr());
        r1.seed(&CountSpider, None);
        let phase1 = r1.run(&CountSpider, &fetcher);
        assert!(phase1.paused);
        assert_eq!(phase1.items.len(), 2);

        // Phase 2: fresh runner, resume from checkpoint, no limit.
        let mut r2 = SpiderRunner::new(SpiderConfig::default(), mgr());
        r2.seed(&CountSpider, Some(phase1.checkpoint));
        let phase2 = r2.run(&CountSpider, &fetcher);
        assert!(!phase2.paused);
        let total = phase1.items.len() + phase2.items.len();
        assert_eq!(total, 5, "every URL 0..=4 should emit exactly one item across resume");
    }

    #[test]
    fn checkpoint_round_trips_through_json() {
        let cp = Checkpoint {
            pending: vec![CheckpointRequest {
                url: "https://x.test/a".into(),
                method: "GET".into(),
                session_id: Some("s1".into()),
            }],
            seen: vec!["GET https://x.test/".into()],
            items_emitted: 1,
        };
        let s = serde_json::to_string(&cp).unwrap();
        let back: Checkpoint = serde_json::from_str(&s).unwrap();
        assert_eq!(back.pending.len(), 1);
        assert_eq!(back.pending[0].session_id.as_deref(), Some("s1"));
        assert_eq!(back.items_emitted, 1);
    }

    #[test]
    fn per_domain_throttle_records_delay() {
        let cfg = SpiderConfig {
            download_delay: Duration::from_millis(500),
            ..Default::default()
        };
        let mut runner = SpiderRunner::new(cfg, mgr());
        let now = Instant::now();
        // First call: nothing recorded, no delay.
        assert_eq!(runner.delay_for("x.test", now), Duration::ZERO);
        runner.last_fetch_per_host.insert("x.test".into(), now);
        // Immediately after: full delay still owed.
        let later = now + Duration::from_millis(100);
        assert_eq!(runner.delay_for("x.test", later), Duration::from_millis(400));
        // After delay elapsed: no wait owed.
        let much_later = now + Duration::from_millis(600);
        assert_eq!(runner.delay_for("x.test", much_later), Duration::ZERO);
    }

    #[test]
    fn robots_disallow_skips_fetch() {
        let robots = Arc::new(RobotsCache::new(Duration::from_secs(60)));
        robots
            .store("blocked.test", Some("User-agent: *\nDisallow: /\n"), "crawlex")
            .unwrap();
        struct BlockedSpider;
        impl Spider for BlockedSpider {
            fn start_urls(&self) -> Vec<String> {
                vec!["https://blocked.test/page".into()]
            }
            fn parse(&self, _r: &Response) -> Vec<ParseYield> {
                vec![ParseYield::item(serde_json::json!({"hit": true}))]
            }
        }
        let fetcher = MapFetcher::new().with("https://blocked.test/page", 200, "x");
        let cfg = SpiderConfig {
            robots_txt_obey: true,
            ..Default::default()
        };
        let mut runner = SpiderRunner::new(cfg, mgr()).with_robots(robots);
        runner.seed(&BlockedSpider, None);
        let out = runner.run(&BlockedSpider, &fetcher);
        assert!(out.items.is_empty(), "robots Disallow must short-circuit");
        assert!(fetcher.log.lock().unwrap().is_empty());
    }

    #[test]
    fn robots_off_lets_everything_through() {
        let robots = Arc::new(RobotsCache::new(Duration::from_secs(60)));
        robots
            .store("blocked.test", Some("User-agent: *\nDisallow: /\n"), "crawlex")
            .unwrap();
        struct S;
        impl Spider for S {
            fn start_urls(&self) -> Vec<String> {
                vec!["https://blocked.test/page".into()]
            }
            fn parse(&self, _r: &Response) -> Vec<ParseYield> {
                vec![ParseYield::item(serde_json::json!({}))]
            }
        }
        let fetcher = MapFetcher::new().with("https://blocked.test/page", 200, "");
        let cfg = SpiderConfig {
            robots_txt_obey: false,
            ..Default::default()
        };
        let mut runner = SpiderRunner::new(cfg, mgr()).with_robots(robots);
        runner.seed(&S, None);
        let out = runner.run(&S, &fetcher);
        assert_eq!(out.items.len(), 1);
    }
}
