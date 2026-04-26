#[cfg(feature = "cdp-backend")]
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};
use url::Url;

use crate::config::{Config, QueueBackend, StorageBackend};
use crate::discovery::{
    assets::{classify_url, classify_with_mime, AssetKind},
    DiscoveryGraph,
};
use crate::events::{Event, EventKind, EventSink, NullSink};
use crate::frontier::{Dedupe, HostRateLimiter};
use crate::hooks::{HookContext, HookDecision, HookEvent, HookRegistry};
use crate::impersonate::ImpersonateClient;
use crate::policy::{Decision, DecisionReason, PolicyProfile, PolicyThresholds};
use crate::proxy::{ProxyOutcome, ProxyRouter, RouterThresholds};
use crate::queue::{FetchMethod, InMemoryQueue, Job, JobQueue};
#[cfg(feature = "cdp-backend")]
use crate::render::{RenderPool, Renderer};
use crate::robots::RobotsCache;
use crate::storage::{
    memory::MemoryStorage, ArtifactKind, ArtifactMeta, HostFacts, PageMetadata, Storage,
};
use crate::{Error, Result};

pub struct Crawler {
    config: Arc<Config>,
    hooks: Arc<HookRegistry>,
    queue: Arc<dyn JobQueue>,
    storage: Arc<dyn Storage>,
    dedupe: Arc<Dedupe>,
    rate_limiter: Arc<HostRateLimiter>,
    robots: Arc<RobotsCache>,
    graph: Arc<DiscoveryGraph>,
    client: Arc<ImpersonateClient>,
    /// Lazily constructed on first access. When `max_concurrent_render == 0`
    /// and no render job ever hits `process_job`, we never allocate the pool
    /// (no browser spawn, no user-data-dir, no fetcher wiring).
    ///
    /// Only exists when compiled with `cdp-backend` — the mini
    /// build skips this field entirely. Render-method jobs on mini return
    /// `Error::RenderDisabled` from `process_job` before touching this.
    #[cfg(feature = "cdp-backend")]
    render: Arc<std::sync::OnceLock<Arc<RenderPool>>>,
    #[cfg(feature = "cdp-backend")]
    render_sessions_seen: Arc<dashmap::DashSet<String>>,
    /// Per-session antibot contamination state. Keyed by session_id (HTTP
    /// path uses `"http"` as the sentinel since cookies live per-host in
    /// the impersonate client). Bumped via `SessionState::after_challenge`
    /// each time a challenge lands.
    session_states: Arc<dashmap::DashMap<String, crate::antibot::SessionState>>,
    render_requested: AtomicBool,
    proxy_router: Arc<ProxyRouter>,
    next_id: Arc<AtomicU64>,
    /// Hosts we've already run one-shot per-host probes against
    /// (favicon/dns/well-known/pwa/wayback).
    host_probed: Arc<dashmap::DashSet<String>>,
    host_facts: Arc<dashmap::DashMap<String, HostFacts>>,
    host_open_ports: Arc<dashmap::DashMap<String, Vec<u16>>>,
    rdap_done: Arc<dashmap::DashSet<String>>,
    counters: Arc<crate::metrics::Counters>,
    discovery_filter: Option<regex::Regex>,
    /// Sink for the NDJSON event bus. Defaults to `NullSink` (silent) so
    /// existing callers don't get sudden stdout pollution; CLI plumbs in
    /// `NdjsonStdoutSink` when `--emit ndjson` is set.
    events: Arc<dyn EventSink>,
    /// Stable run id surfaced in every emitted event. Generated at
    /// `Crawler::new` from process pid + monotonic counter.
    run_id: u64,
    /// Active policy profile + thresholds. Used by `process_job` to
    /// consult `PolicyEngine` at the three policy points.
    policy_profile: PolicyProfile,
    policy_thresholds: PolicyThresholds,
    #[cfg(feature = "lua-hooks")]
    lua: Option<Arc<crate::hooks::lua::LuaHookHost>>,
    /// Per-host/origin/proxy/session inflight budgets enforced on the
    /// render path. Shared with no-op semantics when limits are large
    /// — the hot path only bumps atomics.
    render_budgets: Arc<crate::scheduler::RenderBudgets>,
    /// Wave 1 #31 — per-session inter-arrival jitter. Populated via
    /// `motion_profile` at construction time; `Fast` yields an `Off`
    /// profile (no delay) so the existing live throughput gates stay
    /// green. Share is by Arc for clone_refs cheap fan-out.
    inter_arrival: Arc<crate::scheduler::InterArrivalJitter>,
    /// Wave 1 #33 — per-session cumulative depth cap (Pareto-shaped).
    /// `EndSession` decisions fire a `session.depth_capped` event and
    /// the caller rotates identity before the job re-enters the queue.
    session_depth: Arc<crate::scheduler::SessionDepthTracker>,
    /// Fase 6 — central session lifecycle registry. Shared with the
    /// cleanup task and (via `SessionDropTarget`) the render pool.
    session_registry: Arc<crate::identity::SessionRegistry>,
    /// Current operator-set render-session scope. Held under RwLock so
    /// `session_scope_auto` can demote on login pages / hard blocks
    /// without taking the whole Config on a write path.
    render_scope: Arc<parking_lot::RwLock<crate::config::RenderSessionScope>>,
}

#[derive(Debug)]
enum JobDisposition {
    Complete,
    Drop {
        reason: String,
    },
    Retry {
        reason: String,
        after: Duration,
    },
    Requeue {
        job: Job,
        delay: Duration,
        reason: String,
    },
    FailedPermanent {
        error: String,
    },
}

impl Crawler {
    pub fn new(config: Config) -> Result<Self> {
        let queue: Arc<dyn JobQueue> = match &config.queue_backend {
            QueueBackend::InMemory => Arc::new(InMemoryQueue::new()),
            #[cfg(feature = "sqlite")]
            QueueBackend::Sqlite { path } => {
                let q = crate::queue::sqlite::SqliteQueue::open(path)?;
                q.set_retry_max(config.retry_max);
                Arc::new(q)
            }
            #[cfg(not(feature = "sqlite"))]
            QueueBackend::Sqlite { .. } => {
                return Err(Error::Config("sqlite feature disabled".into()));
            }
        };
        let storage: Arc<dyn Storage> = match &config.storage_backend {
            StorageBackend::Memory => Arc::new(MemoryStorage::new()),
            #[cfg(feature = "sqlite")]
            StorageBackend::Sqlite { path } => Arc::new(
                crate::storage::sqlite::SqliteStorage::open_with_content_store(
                    path,
                    &config.content_store,
                )?,
            ),
            #[cfg(not(feature = "sqlite"))]
            StorageBackend::Sqlite { .. } => {
                return Err(Error::Config("sqlite feature disabled".into()));
            }
            StorageBackend::Filesystem { root } => Arc::new(
                crate::storage::filesystem::FilesystemStorage::open_with_content_store(
                    root,
                    &config.content_store,
                )?,
            ),
        };

        // `mut` is only consumed by the cdp-backend autodetect branch
        // below; the mini build keeps the binding immutable.
        #[cfg_attr(not(feature = "cdp-backend"), allow(unused_mut))]
        let mut identity_profile = config.user_agent_profile;
        #[cfg(feature = "cdp-backend")]
        if config.profile_autodetect && config.max_concurrent_render > 0 {
            if let Some(major) =
                crate::render::pool::RenderPool::detect_chrome_major(config.chrome_path.as_deref())
            {
                identity_profile = crate::impersonate::Profile::from_detected_major(major);
            }
        }
        let identity_bundle = crate::identity::IdentityBundle::from_profile_with_overrides(
            identity_profile,
            config.identity_preset,
            config.locale.as_deref(),
            config.timezone.as_deref(),
            config.user_agent_override.as_deref(),
            0,
        )
        .map_err(Error::Config)?;
        let mut client_mut = ImpersonateClient::new(identity_bundle.profile())?;
        client_mut.set_follow_redirects(config.follow_redirects);
        client_mut.set_max_redirects(config.max_redirects);
        client_mut.set_cookies_enabled(config.cookies_enabled);
        client_mut.set_http_limits(config.http_limits.clone());
        client_mut.set_identity_bundle(identity_bundle);
        let client = Arc::new(client_mut);
        let config_arc = Arc::new(config);
        #[cfg(feature = "cdp-backend")]
        let render: Arc<std::sync::OnceLock<Arc<RenderPool>>> =
            Arc::new(std::sync::OnceLock::new());
        let config = (*config_arc).clone();
        let proxy_urls: Vec<Url> = config
            .proxy
            .proxies
            .iter()
            .filter_map(|s| Url::parse(s).ok())
            .collect();
        let proxy_router = Arc::new(ProxyRouter::new(
            proxy_urls,
            config.proxy.strategy,
            RouterThresholds::default(),
        ));

        let mut policy_thresholds = PolicyThresholds::default();
        policy_thresholds.max_retries = policy_thresholds.max_retries.min(config.retry_max);

        Ok(Self {
            hooks: Arc::new(HookRegistry::new()),
            queue,
            storage,
            dedupe: Arc::new(Dedupe::new(1_000_000, 0.001)),
            rate_limiter: Arc::new(HostRateLimiter::new(config.rate_per_host_rps)),
            robots: Arc::new(RobotsCache::new(Duration::from_secs(24 * 3600))),
            graph: Arc::new(DiscoveryGraph::new()),
            client,
            #[cfg(feature = "cdp-backend")]
            render,
            #[cfg(feature = "cdp-backend")]
            render_sessions_seen: Arc::new(dashmap::DashSet::new()),
            session_states: Arc::new(dashmap::DashMap::new()),
            render_requested: AtomicBool::new(false),
            proxy_router,
            next_id: Arc::new(AtomicU64::new(1)),
            host_probed: Arc::new(dashmap::DashSet::new()),
            host_facts: Arc::new(dashmap::DashMap::new()),
            host_open_ports: Arc::new(dashmap::DashMap::new()),
            rdap_done: Arc::new(dashmap::DashSet::new()),
            counters: Arc::new(crate::metrics::Counters::default()),
            discovery_filter: match config.discovery_filter_regex.as_deref() {
                Some(pat) => Some(
                    regex::Regex::new(pat)
                        .map_err(|e| Error::Config(format!("bad discovery regex: {e}")))?,
                ),
                None => None,
            },
            events: Arc::new(NullSink),
            run_id: gen_run_id(),
            policy_profile: PolicyProfile::default(),
            policy_thresholds,
            #[cfg(feature = "lua-hooks")]
            lua: None,
            render_budgets: Arc::new(crate::scheduler::RenderBudgets::new(config.render_budgets)),
            inter_arrival: Arc::new(crate::scheduler::InterArrivalJitter::new({
                #[cfg(feature = "cdp-backend")]
                {
                    crate::scheduler::JitterProfile::from_motion_profile_str(
                        config.motion_profile.as_str(),
                    )
                }
                #[cfg(not(feature = "cdp-backend"))]
                {
                    crate::scheduler::JitterProfile::Soft
                }
            })),
            session_depth: Arc::new(crate::scheduler::SessionDepthTracker::new(
                config.render_budgets.max_per_session_total,
            )),
            session_registry: Arc::new(crate::identity::SessionRegistry::new(
                config.session_ttl_secs,
            )),
            render_scope: Arc::new(parking_lot::RwLock::new(config.render_session_scope)),
            config: config_arc,
        })
    }

    pub fn with_hooks(mut self, hooks: HookRegistry) -> Self {
        self.hooks = Arc::new(hooks);
        self
    }

    /// Plug an `EventSink` to receive NDJSON events for every relevant
    /// lifecycle point (run.started, job.started, fetch.completed,
    /// decision.made, job.failed, run.completed).
    pub fn with_events(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.events = sink;
        self
    }

    /// Override the active policy profile. Defaults to `Balanced`.
    pub fn with_policy_profile(mut self, profile: PolicyProfile) -> Self {
        let mut thresholds = PolicyThresholds::for_profile(profile);
        thresholds.max_retries = thresholds.max_retries.min(self.config.retry_max);
        self.policy_thresholds = thresholds;
        self.policy_profile = profile;
        self
    }

    pub fn run_id(&self) -> u64 {
        self.run_id
    }

    pub fn events(&self) -> &Arc<dyn EventSink> {
        &self.events
    }

    async fn apply_job_disposition(&self, job_id: u64, disposition: JobDisposition) -> Result<()> {
        match disposition {
            JobDisposition::Complete => self.queue.complete(job_id).await,
            JobDisposition::Drop { reason } => {
                tracing::debug!(job_id, reason = %reason, "job dropped by policy");
                self.queue.complete(job_id).await
            }
            JobDisposition::Retry { reason, after } => {
                self.counters.inc(&self.counters.retries);
                self.queue
                    .fail(job_id, &reason, duration_to_queue_secs(after))
                    .await
            }
            JobDisposition::Requeue { job, delay, reason } => {
                tracing::debug!(
                    job_id,
                    new_job_id = job.id,
                    delay_ms = delay.as_millis() as u64,
                    reason = %reason,
                    "job requeued"
                );
                self.queue.requeue_after(job_id, job, delay).await
            }
            JobDisposition::FailedPermanent { error } => {
                self.queue.fail_permanently(job_id, &error).await
            }
        }
    }

    fn execute_policy_decision(
        &self,
        job: &Job,
        host: &str,
        proxy_for_job: Option<&Url>,
        decision: Decision,
        reason: DecisionReason,
        ctx: &mut HookContext,
    ) -> Result<Option<JobDisposition>> {
        let disposition = match decision {
            Decision::Render
                if self.config.max_concurrent_render > 0 && cfg!(feature = "cdp-backend") =>
            {
                ctx.user_data
                    .insert("escalated_to_render".into(), serde_json::Value::Bool(true));
                let escalated = Job {
                    id: self.next_id.fetch_add(1, Ordering::Relaxed),
                    url: job.url.clone(),
                    depth: job.depth,
                    priority: job.priority.saturating_add(10),
                    method: FetchMethod::Render,
                    attempts: 0,
                    last_error: None,
                };
                tracing::info!(url=%job.url, why=%reason.code, "policy: escalated to render");
                Some(JobDisposition::Requeue {
                    job: escalated,
                    delay: Duration::ZERO,
                    reason: format!("policy:render:{}", reason.code),
                })
            }
            Decision::Render => {
                let ignored_reason = if self.config.max_concurrent_render == 0 {
                    "max_concurrent_render=0"
                } else {
                    "cdp-backend-disabled"
                };
                tracing::info!(
                    url=%job.url,
                    reason = ignored_reason,
                    "policy wanted render but render is unavailable; staying on http"
                );
                ctx.user_data.insert(
                    "policy_render_ignored".into(),
                    serde_json::Value::String(ignored_reason.into()),
                );
                None
            }
            Decision::Retry { after_ms } => Some(JobDisposition::Retry {
                reason: reason.to_string(),
                after: Duration::from_millis(after_ms),
            }),
            Decision::SwitchProxy => {
                let retry_after_secs = if let Some(current) = proxy_for_job {
                    if self
                        .proxy_router
                        .best_alternative(current, host, 0)
                        .is_some()
                    {
                        0
                    } else {
                        let base_ms =
                            self.config.retry_backoff.as_millis().min(u64::MAX as u128) as u64;
                        backoff_seconds(job.attempts, base_ms.max(1))
                    }
                } else {
                    let base_ms =
                        self.config.retry_backoff.as_millis().min(u64::MAX as u128) as u64;
                    backoff_seconds(job.attempts, base_ms.max(1))
                };
                Some(JobDisposition::Retry {
                    reason: reason.to_string(),
                    after: Duration::from_secs(retry_after_secs),
                })
            }
            Decision::Defer { until_ms } => {
                let mut deferred = job.clone();
                deferred.id = self.next_id.fetch_add(1, Ordering::Relaxed);
                Some(JobDisposition::Requeue {
                    job: deferred,
                    delay: Duration::from_millis(until_ms),
                    reason: format!("policy:defer:{}", reason.code),
                })
            }
            Decision::Drop => Some(JobDisposition::Drop {
                reason: reason.to_string(),
            }),
            Decision::CollectArtifacts => {
                ctx.user_data
                    .insert("collect_artifacts".into(), serde_json::Value::Bool(true));
                if self.policy_thresholds.always_capture_artifacts {
                    ctx.user_data.insert(
                        "increase_observability".into(),
                        serde_json::Value::Bool(true),
                    );
                }
                None
            }
            Decision::IncreaseObservability => {
                ctx.user_data.insert(
                    "increase_observability".into(),
                    serde_json::Value::Bool(true),
                );
                None
            }
            Decision::HumanHandoff { reason, .. } => {
                ctx.user_data
                    .insert("human_handoff".into(), serde_json::Value::String(reason));
                None
            }
            Decision::Http => None,
        };
        Ok(disposition)
    }

    pub fn storage(&self) -> Arc<dyn Storage> {
        self.storage.clone()
    }

    pub fn graph(&self) -> Arc<DiscoveryGraph> {
        self.graph.clone()
    }

    pub async fn seed<I, S>(&self, urls: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.seed_with(urls, FetchMethod::Auto).await
    }

    pub async fn seed_with<I, S>(&self, urls: I, method: FetchMethod) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if matches!(method, FetchMethod::Auto | FetchMethod::Render) {
            self.render_requested.store(true, Ordering::Relaxed);
        }
        for u in urls {
            let parsed = Url::parse(u.as_ref())?;
            if !self.dedupe.insert_url_set(&parsed) {
                continue;
            }
            let job = Job {
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                url: parsed,
                depth: 0,
                priority: 0,
                method,
                attempts: 0,
                last_error: None,
            };
            self.queue.push(job).await?;
        }
        // Seed crt.sh subdomains (idempotent per registrable due to dedupe).
        if self.config.crtsh_enabled {
            // Snapshot seeds to query; each unique registrable hit once.
            let mut seen_roots = std::collections::HashSet::new();
            let all_jobs = self.queue.len().await.unwrap_or(0);
            let _ = all_jobs;
            for origin_host in self.seed_hosts_snapshot().await {
                if let Some(root) = crate::discovery::subdomains::registrable_domain(&origin_host.0)
                {
                    if seen_roots.insert(root.clone()) {
                        if let Err(e) = self.seed_crtsh(&origin_host.1).await {
                            tracing::debug!(?e, root=%root, "crtsh seed failed");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Read pending (host, url) pairs without disturbing the queue state.
    /// Used at startup to drive crt.sh / DNS enrichment before run() picks
    /// jobs off the queue.
    async fn seed_hosts_snapshot(&self) -> Vec<(String, Url)> {
        let urls = match self.queue.peek_pending_urls().await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(?e, "peek_pending_urls failed; skipping snapshot");
                return Vec::new();
            }
        };
        urls.into_iter()
            .filter_map(|u| u.host_str().map(|h| (h.to_string(), u.clone())))
            .collect()
    }

    pub async fn run(&self) -> Result<()> {
        // Emit run.started — first signal a CLI/SDK consumer sees that the
        // crawl is live. Carries the policy profile so consumers can
        // validate they're observing the run they expected.
        self.events.emit(
            &Event::of(EventKind::RunStarted)
                .with_run(self.run_id)
                .with_data(&serde_json::json!({
                    "policy_profile": self.policy_profile,
                    "max_concurrent_http": self.config.max_concurrent_http,
                    "max_concurrent_render": self.config.max_concurrent_render,
                })),
        );
        let _run_guard = RunCompletedGuard {
            sink: self.events.clone(),
            run_id: self.run_id,
        };
        let has_render_jobs = match self.queue.has_pending_render_jobs().await {
            Ok(v) => v,
            Err(e) => {
                warn!(?e, "queue has_pending_render_jobs unavailable; proceeding without startup precheck");
                false
            }
        };
        if has_render_jobs {
            self.render_requested.store(true, Ordering::Relaxed);
        }
        if self.config.max_concurrent_render == 0 && has_render_jobs {
            warn!("queue has pending render jobs but max_concurrent_render=0: render will be permanently disabled");
        }
        // Fail fast if the render path is enabled but no Chrome is reachable.
        // When max_concurrent_render == 0 the operator explicitly opted out,
        // so we stay silent. Mini build skips this entirely — there's no
        // render pool to preflight.
        #[cfg(feature = "cdp-backend")]
        if self.config.max_concurrent_render > 0 && self.render_requested.load(Ordering::Relaxed) {
            match self.render_pool().preflight().await {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        "render preflight failed — render jobs will error until \
                         --chrome-path is set or Chrome/Chromium is installed on PATH"
                    );
                }
            }
        }
        // Fase 6 — session TTL cleanup task. Only meaningful when the
        // render path is enabled; HTTP-only runs create no BrowserContexts
        // so expiring the registry has nothing to tear down. The task
        // aborts when the `Crawler` drops (via the handle in scope).
        #[cfg(feature = "cdp-backend")]
        let _cleanup_handle: Option<tokio::task::JoinHandle<()>> =
            if self.config.max_concurrent_render > 0 {
                let pool: Arc<dyn crate::identity::SessionDropTarget> = self.render_pool().clone();
                let archive: Arc<dyn crate::identity::SessionArchive> =
                    Arc::new(crate::identity::StorageArchive(self.storage.clone()));
                let tick =
                    std::time::Duration::from_secs((self.config.session_ttl_secs / 4).max(30));
                Some(crate::identity::spawn_cleanup_task(
                    self.session_registry.clone(),
                    pool,
                    Some(archive),
                    tick,
                ))
            } else {
                None
            };
        #[cfg(feature = "cdp-backend")]
        struct CleanupAbort(Option<tokio::task::JoinHandle<()>>);
        #[cfg(feature = "cdp-backend")]
        impl Drop for CleanupAbort {
            fn drop(&mut self) {
                if let Some(h) = self.0.take() {
                    h.abort();
                }
            }
        }
        #[cfg(feature = "cdp-backend")]
        let _cleanup_abort = CleanupAbort(_cleanup_handle);

        if let Some(port) = self.config.metrics_prometheus_port {
            let c = self.counters.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::metrics_server::serve(port, c).await {
                    tracing::warn!(?e, "prometheus server exited");
                }
            });
        }
        // Proxy health checker: only runs when the operator asked for it
        // (interval > 0) and a proxy list exists.
        if let Some(interval) = self.config.proxy.health_check_interval {
            let proxies: Vec<Url> = self
                .config
                .proxy
                .proxies
                .iter()
                .filter_map(|s| Url::parse(s).ok())
                .collect();
            crate::proxy::health::spawn(self.proxy_router.clone(), proxies, interval);
        }
        // Proxy router persistence: hydrate from SQLite at startup and spawn
        // a throttled flush loop so score changes trickle back to disk. Only
        // runs when the sqlite storage backend is active — filesystem /
        // in-memory builds keep the router but lose state across restarts.
        #[cfg(feature = "sqlite")]
        if !self.proxy_router.is_empty() {
            if let Some(any) = self.storage.as_any_ref() {
                if let Some(sq) = any.downcast_ref::<crate::storage::sqlite::SqliteStorage>() {
                    if let Err(e) =
                        crate::proxy::router::hydrate_from_storage(&self.proxy_router, sq).await
                    {
                        tracing::debug!(?e, "proxy router hydrate failed");
                    }
                }
            }
            // Spawn a throttled flush loop. The flush task re-downcasts the
            // storage handle each tick so we don't have to track a separate
            // Arc<SqliteStorage>; all we need is a live Arc<dyn Storage>
            // that still points at a SqliteStorage.
            let storage_for_flush = self.storage.clone();
            let router_for_flush = self.proxy_router.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let (scores, affinity) = router_for_flush.drain_pending();
                    if scores.is_empty() && affinity.is_empty() {
                        continue;
                    }
                    let Some(any) = storage_for_flush.as_any_ref() else {
                        continue;
                    };
                    let Some(sq) = any.downcast_ref::<crate::storage::sqlite::SqliteStorage>()
                    else {
                        continue;
                    };
                    if !scores.is_empty() {
                        let rows = crate::proxy::router::pack_score_rows(scores);
                        if let Err(e) = sq.save_proxy_scores(rows).await {
                            tracing::debug!(?e, "proxy flush: scores");
                        }
                    }
                    if !affinity.is_empty() {
                        let rows: Vec<(String, i64, String)> = affinity
                            .into_iter()
                            .map(|(h, b, u)| (h, b as i64, u.to_string()))
                            .collect();
                        if let Err(e) = sq.save_host_affinity(rows).await {
                            tracing::debug!(?e, "proxy flush: affinity");
                        }
                    }
                }
            });
        }
        let sem = Arc::new(Semaphore::new(self.config.max_concurrent_http));
        // `JoinSet` replaces the old `Vec<Pin<Box<Future>>>` + `select_all`
        // pattern. Two reasons:
        //   1. `select_all` is O(N) per poll and was called O(N) times per
        //      iteration → O(N²) bookkeeping at high concurrency. JoinSet
        //      uses an intrusive linked list so `join_next()` is O(1).
        //   2. JoinSet automatically drops aborted tasks; we don't have to
        //      track handle lifetimes manually.
        let mut tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
        loop {
            let Some(job) = self.queue.pop().await? else {
                let pending = self.queue.pending_count().await.unwrap_or(0);
                if tasks.is_empty() && pending == 0 {
                    break;
                }
                let delay = self
                    .queue
                    .next_ready_delay()
                    .await
                    .unwrap_or(None)
                    .unwrap_or_else(|| std::time::Duration::from_millis(250))
                    .min(std::time::Duration::from_millis(250));
                let delay = if delay.is_zero() {
                    std::time::Duration::from_millis(1)
                } else {
                    delay
                };
                if tasks.is_empty() {
                    tokio::time::sleep(delay).await;
                } else {
                    tokio::select! {
                        _ = tasks.join_next() => {}
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                continue;
            };

            let permit = sem.clone().acquire_owned().await.unwrap();
            let this = self.clone_refs();
            tasks.spawn(async move {
                let _permit = permit;
                let job_id = job.id;
                let attempts = job.attempts;
                let job_url = job.url.to_string();
                let job_method = format!("{:?}", job.method);
                let job_for_policy = job.clone();
                this.events.emit(
                    &Event::of(EventKind::JobStarted)
                        .with_run(this.run_id)
                        .with_url(&job_url)
                        .with_data(&serde_json::json!({
                            "job_id": job_id,
                            "method": job_method,
                            "depth": job.depth,
                            "priority": job.priority,
                            "attempts": attempts,
                        })),
                );
                match this.process_job(job).await {
                    Ok(disposition) => {
                        if let Err(e) = this.apply_job_disposition(job_id, disposition).await {
                            warn!(?e, job_id, "apply job disposition failed");
                        }
                    }
                    Err(e) => {
                        warn!(?e, job_id, "job failed");
                        let err_kind = e.kind();
                        let err_msg = e.to_string();
                        this.events.emit(
                            &Event::of(EventKind::JobFailed)
                                .with_run(this.run_id)
                                .with_url(&job_url)
                                .with_why(format!("error:{err_kind}"))
                                .with_data(&serde_json::json!({
                                    "job_id": job_id,
                                    "kind": err_kind,
                                    "error": err_msg.clone(),
                                    "attempts": attempts,
                                })),
                        );
                        let disposition = if let Error::RenderDisabled(_) = e {
                            JobDisposition::FailedPermanent { error: err_msg }
                        } else {
                            let host = job_for_policy.url.host_str().unwrap_or("").to_string();
                            let p_ctx = crate::policy::PolicyContext {
                                url: &job_for_policy.url,
                                host: &host,
                                initial_method: job_for_policy.method,
                                response_status: None,
                                response_headers: None,
                                response_body: None,
                                proxy_score: None,
                                attempts,
                                render_budget_left: None,
                                host_cooldown_ms_left: 0,
                                thresholds: &this.policy_thresholds,
                            };
                            let (decision, reason) =
                                crate::policy::PolicyEngine::decide_post_error(&p_ctx, err_kind);
                            this.events.emit(
                                &Event::of(EventKind::DecisionMade)
                                    .with_run(this.run_id)
                                    .with_url(&job_url)
                                    .with_why(reason.code.clone())
                                    .with_data(&serde_json::json!({
                                        "decision": decision.as_tag(),
                                        "reason": reason.clone(),
                                        "error_kind": err_kind,
                                        "job_id": job_id,
                                        "attempts": attempts,
                                    })),
                            );
                            let mut err_ctx =
                                HookContext::new(job_for_policy.url.clone(), job_for_policy.depth);
                            let disposition_reason = reason.clone().with_detail(err_msg.clone());
                            match this.execute_policy_decision(
                                &job_for_policy,
                                &host,
                                None,
                                decision,
                                disposition_reason,
                                &mut err_ctx,
                            ) {
                                Ok(Some(disposition)) => disposition,
                                Ok(None) => JobDisposition::FailedPermanent {
                                    error: reason.to_string(),
                                },
                                Err(policy_err) => JobDisposition::FailedPermanent {
                                    error: format!(
                                        "post-error policy execution failed: {policy_err}"
                                    ),
                                },
                            }
                        };
                        if let Err(fe) = this.apply_job_disposition(job_id, disposition).await {
                            warn!(?fe, job_id, "apply job disposition failed");
                        }
                    }
                }
            });

            // Light backpressure: drain one finished task before queueing
            // more if we're at the concurrency cap. The semaphore permit
            // handles the actual gating; this just keeps `tasks` from
            // unbounded growth in pathological cases.
            if tasks.len() >= self.config.max_concurrent_http {
                let _ = tasks.join_next().await;
            }
        }
        // Drain anything still in flight before returning.
        while let Some(_res) = tasks.join_next().await {}
        // Fase 6 — flush registry to archive so `sessions list` sees
        // rows even for runs that never TTL-expired a session.
        self.flush_sessions_on_run_end().await;
        Ok(())
    }

    /// Evict a session everywhere: pool (drop BrowserContext), registry,
    /// and archive sink. Emits `session.evicted`. Safe to call for an
    /// unknown id — every step is a no-op in that case.
    async fn evict_session(&self, id: &str, reason: crate::identity::EvictionReason) {
        #[cfg(feature = "cdp-backend")]
        {
            if self.config.max_concurrent_render > 0 {
                self.render_pool().drop_session(id).await;
            }
        }
        if let Some(entry) = self.session_registry.evict(id) {
            let _ = self.storage.archive_session(&entry, reason).await;
            self.events.emit(
                &Event::of(EventKind::SessionEvicted)
                    .with_run(self.run_id)
                    .with_session(id.to_string())
                    .with_why(format!("evict:{}", reason.as_str()))
                    .with_data(&serde_json::json!({
                        "reason": reason.as_str(),
                        "state": entry.state.as_str(),
                        "urls_visited": entry.urls_visited,
                        "challenges_seen": entry.challenges_seen,
                        "scope_key": entry.scope_key,
                    })),
            );
        }
    }

    /// Archive every remaining registry entry with `RunEnded` reason.
    /// Invoked by `run()` right before returning so operators see a
    /// complete row-per-session trail in `sessions_archive`.
    async fn flush_sessions_on_run_end(&self) {
        let snapshots = self.session_registry.list(None);
        for snap in snapshots {
            if let Some(entry) = self.session_registry.evict(&snap.id) {
                let _ = self
                    .storage
                    .archive_session(&entry, crate::identity::EvictionReason::RunEnded)
                    .await;
                self.events.emit(
                    &Event::of(EventKind::SessionEvicted)
                        .with_run(self.run_id)
                        .with_session(entry.id.clone())
                        .with_why("evict:run_ended".to_string())
                        .with_data(&serde_json::json!({
                            "reason": "run_ended",
                            "state": entry.state.as_str(),
                            "urls_visited": entry.urls_visited,
                            "challenges_seen": entry.challenges_seen,
                            "scope_key": entry.scope_key,
                        })),
                );
            }
        }
    }

    fn clone_refs(&self) -> Self {
        Self {
            config: self.config.clone(),
            hooks: self.hooks.clone(),
            queue: self.queue.clone(),
            storage: self.storage.clone(),
            dedupe: self.dedupe.clone(),
            rate_limiter: self.rate_limiter.clone(),
            robots: self.robots.clone(),
            graph: self.graph.clone(),
            client: self.client.clone(),
            #[cfg(feature = "cdp-backend")]
            render: self.render.clone(),
            #[cfg(feature = "cdp-backend")]
            render_sessions_seen: self.render_sessions_seen.clone(),
            session_states: self.session_states.clone(),
            render_requested: AtomicBool::new(self.render_requested.load(Ordering::Relaxed)),
            proxy_router: self.proxy_router.clone(),
            next_id: self.next_id.clone(),
            host_probed: self.host_probed.clone(),
            host_facts: self.host_facts.clone(),
            host_open_ports: self.host_open_ports.clone(),
            rdap_done: self.rdap_done.clone(),
            counters: self.counters.clone(),
            discovery_filter: self.discovery_filter.clone(),
            events: self.events.clone(),
            run_id: self.run_id,
            policy_profile: self.policy_profile,
            policy_thresholds: self.policy_thresholds.clone(),
            #[cfg(feature = "lua-hooks")]
            lua: self.lua.clone(),
            render_budgets: self.render_budgets.clone(),
            inter_arrival: self.inter_arrival.clone(),
            session_depth: self.session_depth.clone(),
            session_registry: self.session_registry.clone(),
            render_scope: self.render_scope.clone(),
        }
    }

    #[cfg(feature = "lua-hooks")]
    pub fn set_lua_scripts(&mut self, scripts: Vec<std::path::PathBuf>) -> Result<()> {
        if scripts.is_empty() {
            return Ok(());
        }
        let host = Arc::new(
            crate::hooks::lua::LuaHookHost::new_with_storage(scripts, Some(self.storage.clone()))
                .map_err(|e| Error::Config(format!("lua: {e}")))?,
        );
        self.render_pool().set_lua_host(host.clone());
        self.lua = Some(host);
        Ok(())
    }

    pub fn counters(&self) -> Arc<crate::metrics::Counters> {
        self.counters.clone()
    }

    /// Get-or-init the render pool. Only compiled with
    /// `cdp-backend` — HTTP-only builds never call this.
    #[cfg(feature = "cdp-backend")]
    fn render_pool(&self) -> &Arc<RenderPool> {
        self.render.get_or_init(|| {
            let pool = Arc::new(RenderPool::new_with_scope(
                self.config.clone(),
                self.storage.clone(),
                self.render_scope.clone(),
            ));
            pool.set_counters(self.counters.clone());
            pool
        })
    }

    async fn fire_all(&self, event: HookEvent, ctx: &mut HookContext) -> Result<HookDecision> {
        let rust_decision = self.hooks.fire(event, ctx).await?;
        #[cfg(feature = "lua-hooks")]
        if let Some(l) = self.lua.as_ref() {
            // AfterLoad / AfterIdle are dispatched with Page access directly
            // from the render pool; don't double-fire them here.
            if !matches!(event, HookEvent::AfterLoad | HookEvent::AfterIdle) {
                let lua_decision = l.fire(event, ctx).await?;
                if !matches!(lua_decision, HookDecision::Continue) {
                    return Ok(lua_decision);
                }
            }
        }
        Ok(rust_decision)
    }

    /// Persist + route a detected antibot challenge.
    ///
    /// Steps:
    /// 1. Update per-session contamination state.
    /// 2. Feed `ProxyOutcome::ChallengeHit` to the router so the scorer
    ///    penalizes this proxy (fills a ponta frouxa from phase 4.3).
    /// 3. Record the signal via `storage.record_challenge` (best-effort —
    ///    SQLite writer failures don't derail the fetch path).
    /// 4. Ask `PolicyEngine::decide_post_challenge` for the recovery
    ///    action and emit a `challenge.detected` event.
    ///
    /// Returns the `SessionAction` so the caller can carry out the
    /// drop/rotate/kill.
    async fn handle_challenge(
        &self,
        signal: &crate::antibot::ChallengeSignal,
    ) -> crate::policy::SessionAction {
        // 1. Update session state
        let current = self
            .session_states
            .get(&signal.session_id)
            .map(|e| *e.value())
            .unwrap_or_default();
        let provisional = current.after_challenge(signal.level);

        // 2. ProxyRouter ChallengeHit feedback
        if let Some(proxy) = signal.proxy.as_ref() {
            self.proxy_router
                .record_outcome(proxy, ProxyOutcome::ChallengeHit);
        }

        // 3. Persist (best-effort)
        if let Err(e) = self.storage.record_challenge(signal).await {
            tracing::debug!(?e, "record_challenge failed");
        }

        // 4. Decide + emit event
        let action = crate::policy::PolicyEngine::decide_post_challenge(
            signal,
            current,
            signal.proxy.as_ref(),
        );
        let next = if matches!(action, crate::policy::SessionAction::GiveUp) {
            crate::antibot::SessionState::Blocked
        } else {
            provisional
        };
        self.session_states.insert(signal.session_id.clone(), next);
        // Fase 6 — update central registry + emit state_changed. Entry
        // is created on-demand so tests / tight paths don't have to
        // thread registry registration through every call site.
        let scope = *self.render_scope.read();
        let _ = self
            .session_registry
            .get_or_create(&signal.session_id, scope, &signal.url);
        if let Some(p) = signal.proxy.as_ref() {
            self.session_registry.record_proxy(&signal.session_id, p);
        }
        self.session_registry.bump_challenge(&signal.session_id);
        if let Some((from, to)) = self.session_registry.mark(&signal.session_id, next) {
            self.events.emit(
                &Event::of(EventKind::SessionStateChanged)
                    .with_run(self.run_id)
                    .with_session(signal.session_id.clone())
                    .with_url(signal.url.as_str())
                    .with_why(format!("challenge:{}", signal.vendor.as_str()))
                    .with_data(&serde_json::json!({
                        "from": from.as_str(),
                        "to": to.as_str(),
                        "reason": format!("challenge:{}", signal.vendor.as_str()),
                    })),
            );
        }
        // Fase 6 — scope auto-demotion on hostile signals. Only rewrites
        // the crawler-local scope pointer; existing sessions keep their
        // id until evicted (which they will be for HardBlock below).
        if self.config.session_scope_auto {
            let scope_now = *self.render_scope.read();
            let signal_for_scope =
                crate::policy::ScopeSignal::AntibotHostility(signal.vendor, signal.level);
            match crate::policy::decide_scope(scope_now, &signal_for_scope) {
                crate::policy::ScopeDecision::DemoteTo(s)
                | crate::policy::ScopeDecision::Force(s)
                    if s != scope_now =>
                {
                    *self.render_scope.write() = s;
                }
                _ => {}
            }
        }
        // Fase 6 — drop-on-block. When the post-challenge decision is
        // GiveUp (or the state bumped to Blocked) and the operator
        // hasn't opted out via `--keep-blocked-sessions`, evict now so
        // Chrome doesn't keep the hostile context alive until TTL.
        if self.config.drop_session_on_block
            && matches!(next, crate::antibot::SessionState::Blocked)
        {
            self.evict_session(&signal.session_id, crate::identity::EvictionReason::Blocked)
                .await;
        }
        let sitekey = signal
            .metadata
            .get("sitekey")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let title = signal
            .metadata
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let solver_intent = match self.config.challenge_mode {
            crate::config::ChallengeMode::Avoidance => "avoid_only",
            crate::config::ChallengeMode::SolverReady
                if matches!(
                    signal.vendor,
                    crate::antibot::ChallengeVendor::CloudflareTurnstile
                        | crate::antibot::ChallengeVendor::Recaptcha
                        | crate::antibot::ChallengeVendor::RecaptchaEnterprise
                        | crate::antibot::ChallengeVendor::HCaptcha
                        | crate::antibot::ChallengeVendor::GenericCaptcha
                ) =>
            {
                "may_need_solver"
            }
            crate::config::ChallengeMode::SolverReady => "none",
        };
        self.events.emit(
            &Event::of(EventKind::ChallengeDetected)
                .with_run(self.run_id)
                .with_session(signal.session_id.clone())
                .with_url(signal.url.as_str())
                .with_why(format!(
                    "antibot:{}:{}",
                    signal.vendor.as_str(),
                    signal.level.as_str()
                ))
                .with_data(&serde_json::json!({
                    "vendor": signal.vendor.as_str(),
                    "level": signal.level.as_str(),
                    "surface": signal.metadata.get("surface").and_then(|v| v.as_str()),
                    "session_action": action.as_str(),
                    "session_state_before": current.as_str(),
                    "session_state_after": next.as_str(),
                    "proxy": signal.proxy.as_ref().map(|p| p.to_string()),
                    "status_code": signal.metadata.get("status_code").and_then(|v| v.as_u64()),
                    "title": title,
                    "widget_present": signal.metadata.get("widget_present").and_then(|v| v.as_bool()).unwrap_or(matches!(signal.level, crate::antibot::ChallengeLevel::WidgetPresent)),
                    "sitekey": sitekey,
                    "sitekey_present": signal.metadata.get("sitekey").and_then(|v| v.as_str()).is_some(),
                    "action": signal.metadata.get("action").and_then(|v| v.as_str()),
                    "iframe_srcs": signal.metadata.get("iframe_srcs").cloned().unwrap_or_else(|| serde_json::json!([])),
                    "solver_intent": solver_intent,
                    "metadata": signal.metadata,
                })),
        );
        action
    }

    async fn process_job(&self, job: Job) -> Result<JobDisposition> {
        // Render-dependent jobs short-circuit with a stable error when
        // the binary was built without the browser backend (crawlex-mini)
        // — the CLI parse lets `--method render` through so the contract
        // is near-parity, but runtime refuses deterministically.
        #[cfg(not(feature = "cdp-backend"))]
        if matches!(job.method, FetchMethod::Render) {
            return Err(Error::RenderDisabled(
                "render-disabled: this build (crawlex-mini) has no browser backend. \
                 Use crawlex (full) or `--method spoof`."
                    .into(),
            ));
        }
        if matches!(job.method, FetchMethod::Render) && self.config.max_concurrent_render == 0 {
            return Err(Error::RenderDisabled(
                "set --max-concurrent-render > 0 or use --method spoof".into(),
            ));
        }
        if matches!(job.method, FetchMethod::Auto | FetchMethod::Render) {
            self.render_requested.store(true, Ordering::Relaxed);
        }

        let host = job.url.host_str().unwrap_or("").to_string();
        self.rate_limiter.acquire(&host).await;

        // One-shot per-host probes (favicon, DNS, .well-known, PWA, wayback).
        if !host.is_empty() && self.host_probed.insert(host.to_ascii_lowercase()) {
            self.per_host_probes(&job.url).await;
        }

        let mut ctx = HookContext::new(job.url.clone(), job.depth);
        ctx.proxy = self.proxy_router.pick(&host, 0);

        match self
            .fire_all(HookEvent::BeforeEachRequest, &mut ctx)
            .await?
        {
            HookDecision::Skip | HookDecision::Abort => {
                return Ok(JobDisposition::Complete);
            }
            _ => {}
        }

        if self.config.respect_robots_txt {
            if let Err(e) = self.ensure_robots(&job.url).await {
                debug!(?e, "robots fetch failed; allowing by default");
            }
            let ua = self.client.identity_bundle().ua.as_str();
            if let Some(false) = self.robots.check(&job.url, ua) {
                ctx.robots_allowed = Some(false);
                if matches!(
                    self.fire_all(HookEvent::OnRobotsDecision, &mut ctx).await?,
                    HookDecision::Continue | HookDecision::Skip | HookDecision::Abort
                ) && ctx.robots_allowed == Some(false)
                {
                    info!(url=%job.url, "robots.txt disallow; skipping");
                    self.events.emit(
                        &Event::of(EventKind::DecisionMade)
                            .with_run(self.run_id)
                            .with_url(job.url.as_str())
                            .with_why("robots:disallow".to_string())
                            .with_data(&serde_json::json!({
                                "decision": "drop",
                                "reason": {
                                    "code": "robots:disallow",
                                    "detail": "robots_txt"
                                },
                            })),
                    );
                    return Ok(JobDisposition::Drop {
                        reason: "robots:disallow".to_string(),
                    });
                }
            } else {
                ctx.robots_allowed = Some(true);
                let _ = self.fire_all(HookEvent::OnRobotsDecision, &mut ctx).await?;
            }
        }

        let mut effective_method = job.method;
        {
            let proxy_score = ctx
                .proxy
                .as_ref()
                .and_then(|p| self.proxy_router.score_for(p));
            let p_ctx = crate::policy::PolicyContext {
                url: &job.url,
                host: &host,
                initial_method: job.method,
                response_status: None,
                response_headers: None,
                response_body: None,
                proxy_score,
                attempts: job.attempts,
                render_budget_left: None,
                host_cooldown_ms_left: 0,
                thresholds: &self.policy_thresholds,
            };
            let (decision, reason) = crate::policy::PolicyEngine::decide_pre_fetch(&p_ctx);
            self.events.emit(
                &Event::of(EventKind::DecisionMade)
                    .with_run(self.run_id)
                    .with_url(job.url.as_str())
                    .with_why(reason.code.clone())
                    .with_data(&serde_json::json!({
                        "phase": "pre_fetch",
                        "decision": decision.as_tag(),
                        "reason": reason.clone(),
                        "attempts": job.attempts,
                    })),
            );
            match decision {
                Decision::Http => {
                    effective_method = FetchMethod::HttpSpoof;
                }
                Decision::Render if matches!(job.method, FetchMethod::Render) => {
                    effective_method = FetchMethod::Render;
                }
                other => {
                    let proxy_snapshot = ctx.proxy.clone();
                    if let Some(disposition) = self.execute_policy_decision(
                        &job,
                        &host,
                        proxy_snapshot.as_ref(),
                        other,
                        reason,
                        &mut ctx,
                    )? {
                        return Ok(disposition);
                    }
                }
            }
        }

        let prefetch_observability = ctx
            .user_data
            .get("increase_observability")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let use_render = matches!(effective_method, FetchMethod::Render);
        if use_render {
            self.counters.inc(&self.counters.requests_render);
        } else {
            self.counters.inc(&self.counters.requests_http);
        }

        // Wave 1 #31/#33 — crawl-pattern shaping. Keyed on registrable
        // domain so a single browsing session (cookie jar scope) shares
        // the same inter-arrival clock + depth counter.
        let pattern_key = {
            let host = job.url.host_str().unwrap_or_default().to_ascii_lowercase();
            crate::discovery::subdomains::registrable_domain(&host).unwrap_or(host)
        };
        // Inter-arrival jitter: only pad when the previous dispatch on
        // this session happened too recently. `Off` (motion=fast) is a
        // no-op. We sleep before the render dispatch so the host sees
        // the human-shaped cadence, not the synthetic burst.
        {
            let wait = self.inter_arrival.delay_for_next(&pattern_key);
            if !wait.is_zero() {
                tracing::trace!(
                    target: "crawlex::pattern::jitter",
                    session = %pattern_key,
                    wait_ms = wait.as_millis() as u64,
                    "inter-arrival pad"
                );
                tokio::time::sleep(wait).await;
            }
        }
        // Session depth cap: Pareto-shaped hard stop. `EndSession`
        // emits a decision event and requeues the job so the next pop
        // can land on a fresh session id via identity rotation (handled
        // upstream — we simply defer here).
        {
            match self.session_depth.observe(&pattern_key) {
                crate::scheduler::SessionDecision::Continue => {}
                crate::scheduler::SessionDecision::EndAfter => {
                    // Proceed with this job; the *next* dispatch on the
                    // same session will see `EndSession`.
                    tracing::debug!(
                        target: "crawlex::pattern::depth",
                        session = %pattern_key,
                        depth = self.session_depth.depth(&pattern_key),
                        "session depth approaching Pareto cap"
                    );
                }
                crate::scheduler::SessionDecision::EndSession => {
                    self.events.emit(
                        &Event::of(EventKind::DecisionMade)
                            .with_run(self.run_id)
                            .with_url(job.url.as_str())
                            .with_why("session_depth:pareto_cap".to_string())
                            .with_data(&serde_json::json!({
                                "decision": "end_session",
                                "session": pattern_key,
                                "depth": self.session_depth.depth(&pattern_key),
                            })),
                    );
                    // Requeue with a fresh id + short backoff; upstream
                    // identity rotation will land the retry on a new
                    // session key.
                    self.session_depth.reset(&pattern_key);
                    let mut requeue = job.clone();
                    requeue.id = self.next_id.fetch_add(1, Ordering::Relaxed);
                    return Ok(JobDisposition::Requeue {
                        job: requeue,
                        delay: std::time::Duration::from_millis(150),
                        reason: "session_depth:pareto_cap".to_string(),
                    });
                }
            }
        }

        let mut metrics = crate::metrics::PageMetrics::default();

        #[cfg(feature = "cdp-backend")]
        let render_started = std::time::Instant::now();
        // Reserve budget slots before the render. Drops on exit of the
        // function scope (all four counters decrement). Rejection = job
        // requeued with a short backoff so the sibling renders draining
        // the budget can release first.
        #[cfg(feature = "cdp-backend")]
        let _budget_guard = if use_render {
            let (host_key, origin_key) = crate::scheduler::host_and_origin(&job.url);
            let session_key = crate::discovery::subdomains::registrable_domain(&host_key)
                .unwrap_or_else(|| host_key.clone());
            match self.render_budgets.try_acquire(
                &host_key,
                &origin_key,
                ctx.proxy.as_ref(),
                &session_key,
            ) {
                Ok(guard) => Some(guard),
                Err(kind) => {
                    match kind {
                        crate::scheduler::BudgetKind::Host => {
                            self.counters
                                .budget_rejections_host
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        crate::scheduler::BudgetKind::Origin => {
                            self.counters
                                .budget_rejections_origin
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        crate::scheduler::BudgetKind::Proxy => {
                            self.counters
                                .budget_rejections_proxy
                                .fetch_add(1, Ordering::Relaxed);
                        }
                        crate::scheduler::BudgetKind::Session => {
                            self.counters
                                .budget_rejections_session
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    self.events.emit(
                        &Event::of(EventKind::DecisionMade)
                            .with_run(self.run_id)
                            .with_url(job.url.as_str())
                            .with_why(format!("budget:{}", kind.as_str()))
                            .with_data(&serde_json::json!({
                                "decision": "defer",
                                "kind": kind.as_str(),
                            })),
                    );
                    let mut requeue = job.clone();
                    requeue.id = self.next_id.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(
                        url = %job.url,
                        kind = kind.as_str(),
                        "render budget exhausted — deferring"
                    );
                    return Ok(JobDisposition::Requeue {
                        job: requeue,
                        delay: std::time::Duration::from_millis(100),
                        reason: format!("budget:{}", kind.as_str()),
                    });
                }
            }
        } else {
            None
        };
        let (html_opt, final_url, status_code, cdp_urls, peer_cert) = if use_render {
            #[cfg(feature = "cdp-backend")]
            {
                // Dispatch to the ScriptSpec-aware render path when a spec
                // is configured; otherwise fall back to the legacy Actions
                // pipeline. The two share `render_core`, so semantics
                // (session state, challenge detect, screenshot, Lua hook)
                // stay in lockstep.
                let render_result = if let Some(spec) = self.config.script_spec.as_ref() {
                    match self
                        .render_pool()
                        .render_with_script(
                            &job.url,
                            &self.config.wait_strategy,
                            spec,
                            Some(self.events.clone()),
                            Some(self.run_id),
                            ctx.proxy.as_ref(),
                        )
                        .await
                    {
                        Ok((rp, outcome)) => {
                            let artifact_count: usize =
                                outcome.steps.iter().map(|step| step.artifacts.len()).sum();
                            tracing::debug!(
                                target: "crawlex::script",
                                url = %job.url,
                                steps = outcome.steps.len(),
                                artifacts = artifact_count,
                                captures = outcome.captures.len(),
                                exports = outcome.exports.len(),
                                failed_assertion = ?outcome.failed_assertion,
                                "script-spec run complete"
                            );
                            Ok((rp, Some(artifact_count)))
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    self.render_pool()
                        .render(
                            &job.url,
                            &self.config.wait_strategy,
                            self.config.collect_web_vitals || prefetch_observability,
                            self.config.output.screenshot,
                            self.config.actions.as_deref(),
                            ctx.proxy.as_ref(),
                        )
                        .await
                        .map(|rp| (rp, None))
                };
                match render_result {
                    Ok((rp, artifact_count)) => {
                        let is_new_session =
                            self.render_sessions_seen.insert(rp.session_id.clone());
                        // Fase 6 — register in central registry. Called
                        // for every render (idempotent + bumps urls_visited).
                        let scope = *self.render_scope.read();
                        let _ =
                            self.session_registry
                                .get_or_create(&rp.session_id, scope, &job.url);
                        if let Some(p) = ctx.proxy.as_ref() {
                            self.session_registry.record_proxy(&rp.session_id, p);
                        }
                        // Pós-render OK: promote Clean → Warm (monotonic).
                        let current = self
                            .session_states
                            .get(&rp.session_id)
                            .map(|e| *e.value())
                            .unwrap_or_default();
                        if matches!(current, crate::antibot::SessionState::Clean)
                            && rp.challenge.is_none()
                        {
                            self.session_states
                                .insert(rp.session_id.clone(), crate::antibot::SessionState::Warm);
                            if let Some((from, to)) = self
                                .session_registry
                                .mark(&rp.session_id, crate::antibot::SessionState::Warm)
                            {
                                self.events.emit(
                                    &Event::of(EventKind::SessionStateChanged)
                                        .with_run(self.run_id)
                                        .with_session(rp.session_id.clone())
                                        .with_url(job.url.as_str())
                                        .with_why("render_ok".to_string())
                                        .with_data(&serde_json::json!({
                                            "from": from.as_str(),
                                            "to": to.as_str(),
                                            "reason": "render_ok",
                                        })),
                                );
                            }
                        }
                        if is_new_session {
                            self.events.emit(
                                &Event::of(EventKind::SessionCreated)
                                    .with_run(self.run_id)
                                    .with_session(rp.session_id.clone())
                                    .with_url(job.url.as_str())
                                    .with_data(&serde_json::json!({
                                        "engine": "render",
                                        "scope": scope,
                                    })),
                            );
                        }
                        // Build a compact `vitals` summary so a stream
                        // consumer sees Core Web Vitals + TTFB inline,
                        // without round-tripping through SQLite. Falls
                        // back to whatever fields the renderer populated
                        // on `RenderedPage::vitals`.
                        let vitals_summary = {
                            let mut m = crate::metrics::PageMetrics::default();
                            m.vitals = rp.vitals.clone();
                            crate::events::VitalsSummary::from_metrics(&m)
                        };
                        self.events.emit(
                            &Event::of(EventKind::RenderCompleted)
                                .with_run(self.run_id)
                                .with_session(rp.session_id.clone())
                                .with_url(job.url.as_str())
                                .with_data(&serde_json::json!({
                                    "final_url": rp.final_url.as_str(),
                                    "status": rp.status,
                                    "manifest": rp.manifest_url.is_some(),
                                    "service_workers": rp.service_worker_urls.len(),
                                    "screenshot": rp.screenshot_png.is_some(),
                                    "resources": rp.resources.len(),
                                    "runtime_routes": rp.runtime_routes.len(),
                                    "network_endpoints": rp.network_endpoints.len(),
                                    "is_spa": rp.is_spa,
                                    "artifacts": artifact_count.unwrap_or(0),
                                    "vitals": vitals_summary,
                                })),
                        );
                        ctx.user_data.insert(
                            "session_id".into(),
                            serde_json::Value::String(rp.session_id.clone()),
                        );
                        let mut render_facts = crate::storage::HostFacts::default();
                        if rp.manifest_url.is_some() {
                            render_facts.manifest_present = Some(true);
                        }
                        if !rp.service_worker_urls.is_empty() {
                            render_facts.service_worker_present = Some(true);
                        }
                        if render_facts.manifest_present.is_some()
                            || render_facts.service_worker_present.is_some()
                        {
                            if let Some(host) = rp.final_url.host_str().or(job.url.host_str()) {
                                self.record_host_facts(host, &render_facts);
                                let _ = self.storage.save_host_facts(host, &render_facts).await;
                            }
                        }
                        if self.config.collect_web_vitals || prefetch_observability {
                            metrics.vitals = rp.vitals.clone();
                            metrics.resources = rp.resources.clone();
                        }
                        if let Some(png) = rp.screenshot_png.as_ref() {
                            let path = self
                                .storage
                                .save_screenshot(&job.url, png)
                                .await
                                .ok()
                                .flatten();
                            let _ = self.write_screenshot_output(&job.url, png);
                            let sha = hex::encode(sha2::Sha256::digest(png));
                            let saved = crate::events::ArtifactSavedData {
                                kind: crate::storage::ArtifactKind::ScreenshotFullPage
                                    .wire_str()
                                    .to_string(),
                                mime: "image/png".to_string(),
                                size: png.len() as u64,
                                sha256: sha,
                                name: None,
                                step_id: None,
                                step_kind: None,
                                selector: None,
                                final_url: Some(rp.final_url.to_string()),
                                path,
                            };
                            self.events.emit(
                                &Event::of(EventKind::ArtifactSaved)
                                    .with_run(self.run_id)
                                    .with_session(rp.session_id.clone())
                                    .with_url(job.url.as_str())
                                    .with_data(&saved),
                            );
                        }
                        // Render-path antibot handling. `RenderedPage::challenge`
                        // was populated by the pool after settle+content via
                        // `antibot::detect_from_html`. Persist snapshot with
                        // `challenge_<vendor>_<session>` prefix and route to
                        // policy for the action.
                        if let Some(signal) = rp.challenge.as_ref() {
                            let action = self.handle_challenge(signal).await;
                            let _ = self.write_challenge_snapshot(
                                &signal.vendor,
                                &signal.session_id,
                                &rp.html_post_js,
                                rp.screenshot_png.as_deref(),
                            );
                            tracing::info!(
                                url=%job.url,
                                vendor=signal.vendor.as_str(),
                                level=signal.level.as_str(),
                                action=action.as_str(),
                                "antibot challenge detected on render path"
                            );
                            self.counters.record_challenge(ctx.proxy.as_ref());
                        } else if let Some(p) = ctx.proxy.as_ref() {
                            // Render-path `record_outcome` — closes the
                            // Fase 4.3 ponta-frouxa. Challenge path is
                            // already handled above via `handle_challenge`.
                            let status = rp.status;
                            let latency_ms = render_started.elapsed().as_secs_f64() * 1_000.0;
                            let outcome = if (500..600).contains(&status) {
                                ProxyOutcome::Status(status)
                            } else if status == 0 {
                                // Chrome never got a main-document status —
                                // treat as a reset on the wire.
                                ProxyOutcome::Reset
                            } else {
                                ProxyOutcome::Success { latency_ms }
                            };
                            self.proxy_router.record_outcome(p, outcome);
                        }
                        let _ = self.fire_all(HookEvent::AfterLoad, &mut ctx).await?;
                        let _ = self.fire_all(HookEvent::AfterIdle, &mut ctx).await?;
                        (
                            Some(rp.html_post_js),
                            rp.final_url,
                            rp.status,
                            rp.captured_urls,
                            None,
                        )
                    }
                    Err(e) => {
                        self.counters.inc(&self.counters.errors);
                        // Feed the render failure back into the router.
                        // "timeout" in the error string is our signal for
                        // Chrome navigation timeouts; everything else is a
                        // generic reset so the score still degrades.
                        if let Some(p) = ctx.proxy.as_ref() {
                            let msg = e.to_string().to_ascii_lowercase();
                            let outcome = if msg.contains("timeout") {
                                ProxyOutcome::Timeout
                            } else if msg.contains("connect") || msg.contains("dns") {
                                ProxyOutcome::ConnectFailed
                            } else {
                                ProxyOutcome::Reset
                            };
                            self.proxy_router.record_outcome(p, outcome);
                        }
                        ctx.error = Some(e.to_string());
                        let _ = self.fire_all(HookEvent::OnError, &mut ctx).await;
                        return Err(e);
                    }
                }
            }
            #[cfg(not(feature = "cdp-backend"))]
            {
                // Unreachable: the early `FetchMethod::Render` guard at
                // the top of `process_job` already short-circuited. Kept
                // so the type checker sees a value for `(html_opt, ...)`
                // on the mini build.
                return Err(Error::RenderDisabled(
                    "render-disabled: crawlex-mini".into(),
                ));
            }
        } else {
            let cdp_empty: Vec<Url> = Vec::new();
            let _ = cdp_empty;
            // Use the URL's classified asset kind so we emit the right
            // Sec-Fetch-Dest / Accept / mode for JS / CSS / API / etc.
            let dest = classify_url(&job.url).sec_fetch_dest();
            let proxy_for_job = ctx.proxy.clone();
            let fetch_started = std::time::Instant::now();
            let collect_net_timings = self.config.collect_net_timings || prefetch_observability;
            let fetch = if proxy_for_job.is_some() && collect_net_timings {
                self.client
                    .get_timed_via(&job.url, proxy_for_job.as_ref(), dest)
                    .await
            } else if proxy_for_job.is_some() {
                self.client
                    .get_via(&job.url, proxy_for_job.as_ref(), dest)
                    .await
            } else if collect_net_timings {
                self.client.get_timed_with_dest(&job.url, dest).await
            } else if matches!(dest, crate::discovery::assets::SecFetchDest::Document) {
                self.client.get(&job.url).await
            } else {
                self.client.get_with_dest(&job.url, dest).await
            };
            let resp = match fetch {
                Ok(r) => r,
                Err(e) => {
                    self.counters.inc(&self.counters.errors);
                    // Feed the failure back into the router so the proxy's
                    // score reflects reality. Kind-specific outcome so
                    // connect failures and timeouts land on different
                    // counters.
                    if let Some(p) = proxy_for_job.as_ref() {
                        let outcome = match e.kind() {
                            "dns" | "tls" | "io" => ProxyOutcome::ConnectFailed,
                            "request-timeout" => ProxyOutcome::Timeout,
                            _ => ProxyOutcome::Reset,
                        };
                        self.proxy_router.record_outcome(p, outcome);
                    }
                    ctx.error = Some(e.to_string());
                    let _ = self.fire_all(HookEvent::OnError, &mut ctx).await;
                    return Err(e);
                }
            };
            // Record success/status outcome so the router learns from real
            // traffic on the same axis as health probes.
            if let Some(p) = proxy_for_job.as_ref() {
                let status = resp.status.as_u16();
                let outcome = if (200..400).contains(&status) {
                    ProxyOutcome::Success {
                        latency_ms: fetch_started.elapsed().as_secs_f64() * 1_000.0,
                    }
                } else {
                    ProxyOutcome::Status(status)
                };
                self.proxy_router.record_outcome(p, outcome);
            }
            if collect_net_timings {
                metrics.net = resp.timings.clone();
            }
            ctx.response_status = Some(resp.status.as_u16());
            ctx.response_headers = Some(resp.headers.clone());
            ctx.body = Some(resp.body.clone());

            // FetchMethod::Auto: consult the Policy Engine post-fetch. The
            // engine returns a structured Decision + DecisionReason; the
            // execution helper maps it to a JobDisposition when it needs to
            // stop normal response processing.
            if matches!(job.method, FetchMethod::Auto) {
                let proxy_score = proxy_for_job
                    .as_ref()
                    .and_then(|p| self.proxy_router.score_for(p));
                let p_ctx = crate::policy::PolicyContext {
                    url: &job.url,
                    host: &host,
                    initial_method: job.method,
                    response_status: Some(resp.status.as_u16()),
                    response_headers: Some(&resp.headers),
                    response_body: Some(&resp.body),
                    proxy_score,
                    attempts: job.attempts,
                    render_budget_left: None,
                    host_cooldown_ms_left: 0,
                    thresholds: &self.policy_thresholds,
                };
                let (decision, reason) = crate::policy::PolicyEngine::decide_post_fetch(&p_ctx);
                self.events.emit(
                    &Event::of(EventKind::DecisionMade)
                        .with_run(self.run_id)
                        .with_url(job.url.as_str())
                        .with_why(reason.code.clone())
                        .with_data(&serde_json::json!({
                            "decision": decision.as_tag(),
                            "reason": reason.clone(),
                            "status": resp.status.as_u16(),
                        })),
                );
                if let Some(disposition) = self.execute_policy_decision(
                    &job,
                    &host,
                    proxy_for_job.as_ref(),
                    decision,
                    reason,
                    &mut ctx,
                )? {
                    return Ok(disposition);
                }
            }

            let _ = self.fire_all(HookEvent::AfterFirstByte, &mut ctx).await?;
            let _ = self.fire_all(HookEvent::OnResponseBody, &mut ctx).await?;

            // Antibot detection on the HTTP path. Runs even when the
            // response already looked fine to `decide_post_fetch` —
            // `detect_from_http_response` picks up subtler signals
            // (datadome cookie on a 200, _px3 cookie, etc).
            if let Some(raw) = crate::antibot::detect_from_http_response(
                resp.status.as_u16(),
                &resp.body,
                &resp.headers,
                &job.url,
            ) {
                let signal =
                    raw.into_signal(&resp.final_url, "http".to_string(), proxy_for_job.clone());
                let action = self.handle_challenge(&signal).await;
                tracing::debug!(
                    url=%job.url,
                    vendor=signal.vendor.as_str(),
                    level=signal.level.as_str(),
                    action=action.as_str(),
                    "antibot challenge detected on http path"
                );
            }

            self.storage
                .save_raw_response(
                    &job.url,
                    &resp.final_url,
                    resp.status.as_u16(),
                    &resp.headers,
                    &resp.body,
                    resp.body_truncated,
                )
                .await?;

            // Surface per-request timings on the stream so consumers
            // don't have to round-trip through the SQLite `page_metrics`
            // table to inspect a fetch's network breakdown.
            self.events.emit(
                &Event::of(EventKind::FetchCompleted)
                    .with_run(self.run_id)
                    .with_url(job.url.as_str())
                    .with_data(&crate::events::FetchCompletedData {
                        final_url: resp.final_url.to_string(),
                        status: resp.status.as_u16(),
                        bytes: Some(resp.body.len() as u64),
                        body_truncated: resp.body_truncated,
                        dns_ms: resp.timings.dns_ms,
                        tcp_connect_ms: resp.timings.tcp_connect_ms,
                        tls_handshake_ms: resp.timings.tls_handshake_ms,
                        ttfb_ms: resp.timings.ttfb_ms,
                        download_ms: resp.timings.download_ms,
                        total_ms: resp.timings.total_ms,
                        alpn: resp.timings.alpn.clone(),
                        tls_version: resp.timings.tls_version.clone(),
                        cipher: resp.timings.cipher.clone(),
                    }),
            );

            let ct_header = resp
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let is_html = ct_header
                .as_deref()
                .map(|s| s.contains("text/html") || s.contains("xhtml"))
                .unwrap_or(false);

            let html = if is_html {
                let (decoded_html, charset) =
                    crate::impersonate::decode::decode_html_to_string(&resp.headers, &resp.body);
                ctx.user_data
                    .insert("charset".into(), serde_json::Value::String(charset));
                Some(decoded_html)
            } else {
                None
            };
            (
                html,
                resp.final_url,
                resp.status.as_u16(),
                Vec::<Url>::new(),
                resp.peer_cert,
            )
        };

        let ct = ctx
            .response_headers
            .as_ref()
            .and_then(|h| h.get("content-type"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let kind = classify_with_mime(&final_url, ct.as_deref());
        let meta = PageMetadata {
            final_url: final_url.clone(),
            status: status_code,
            bytes: html_opt.as_ref().map(|h| h.len() as u64).unwrap_or(0),
            rendered: use_render,
            kind,
        };
        ctx.user_data.insert(
            "asset_kind".into(),
            serde_json::Value::String(kind.as_str().into()),
        );
        let force_observability = ctx
            .user_data
            .get("increase_observability")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let collect_artifacts = ctx
            .user_data
            .get("collect_artifacts")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let has_metrics = self.config.collect_net_timings
            || self.config.collect_web_vitals
            || force_observability;
        if has_metrics {
            if let Ok(m_json) = serde_json::to_value(&metrics) {
                ctx.user_data.insert("metrics".into(), m_json);
            }
            let _ = self.storage.save_metrics(&job.url, &metrics).await;
        }
        let artifact_session_id = ctx
            .user_data
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| crate::storage::session_id_for_url(&job.url));
        if collect_artifacts {
            if let Some(body) = ctx.body.as_ref() {
                let artifact = ArtifactMeta {
                    url: &job.url,
                    final_url: Some(&final_url),
                    session_id: &artifact_session_id,
                    kind: ArtifactKind::SnapshotResponseBody,
                    name: Some("response_body"),
                    step_id: None,
                    step_kind: None,
                    selector: None,
                    mime: ct.as_deref(),
                };
                let _ = self.storage.save_artifact(&artifact, body.as_ref()).await;
            }
        }

        if let Some(final_host) = final_url.host_str() {
            if !final_host.is_empty()
                && !job
                    .url
                    .host_str()
                    .is_some_and(|h| h.eq_ignore_ascii_case(final_host))
                && self.host_probed.insert(final_host.to_ascii_lowercase())
            {
                self.per_host_probes(&final_url).await;
            }
        }

        let host_facts = self.host_facts_snapshot(&final_url, &job.url);
        let open_ports = self.open_ports_snapshot(&final_url, &job.url);
        if html_opt.is_none() {
            let tech_report = crate::discovery::tech_fingerprint::analyze_with_facts(
                &job.url,
                &final_url,
                ctx.response_headers.as_ref(),
                None,
                crate::discovery::tech_fingerprint::TechFingerprintFacts {
                    peer_cert: peer_cert.as_ref(),
                    dns_json: host_facts.as_ref().and_then(|f| f.dns_json.as_deref()),
                    open_ports: open_ports.as_deref().unwrap_or(&[]),
                    manifest_present: host_facts
                        .as_ref()
                        .and_then(|f| f.manifest_present)
                        .unwrap_or(false),
                    service_worker_present: host_facts
                        .as_ref()
                        .and_then(|f| f.service_worker_present)
                        .unwrap_or(false),
                },
            );
            self.save_and_emit_tech_report(&job.url, tech_report).await;
        }

        // When the response body is JavaScript, mine it for API endpoints.
        if matches!(kind, AssetKind::Js) {
            if let Some(bytes) = ctx.body.as_ref() {
                let text = String::from_utf8_lossy(bytes).to_string();
                let base = job.url.clone();
                let js_urls = tokio::task::spawn_blocking(move || {
                    crate::discovery::js_endpoints::extract(&base, &text)
                })
                .await
                .unwrap_or_default();
                let next_depth = job.depth + 1;
                for to in js_urls {
                    if !self.should_follow(&job.url, &to) {
                        continue;
                    }
                    if !self.dedupe.insert_url_set(&to) {
                        continue;
                    }
                    self.graph.add_edge(&job.url, &to);
                    let _ = self.storage.save_edge(&job.url, &to).await;
                    let job2 = Job {
                        id: self.next_id.fetch_add(1, Ordering::Relaxed),
                        url: to,
                        depth: next_depth,
                        priority: 0,
                        method: job.method,
                        attempts: 0,
                        last_error: None,
                    };
                    let _ = self.queue.push(job2).await;
                }
            }
        }

        if let Some(html) = html_opt {
            self.counters.inc(&self.counters.pages_saved);
            self.storage.save_rendered(&job.url, &html, &meta).await?;
            if collect_artifacts {
                let artifact = ArtifactMeta {
                    url: &job.url,
                    final_url: Some(&final_url),
                    session_id: &artifact_session_id,
                    kind: ArtifactKind::SnapshotPostJsHtml,
                    name: Some("post_js_html"),
                    step_id: None,
                    step_kind: None,
                    selector: None,
                    mime: Some("text/html"),
                };
                let _ = self.storage.save_artifact(&artifact, html.as_bytes()).await;
            }

            // Fase C — classified outbound-asset harvest + frontier links.
            // Parse HTML once on the blocking pool and feed link/asset
            // extraction plus tech fingerprinting from the same tree.
            let target_root = self.config.target_domain.clone().unwrap_or_else(|| {
                job.url
                    .host_str()
                    .and_then(crate::discovery::subdomains::registrable_domain)
                    .unwrap_or_default()
            });
            let url_for_parse = job.url.clone();
            let final_url_for_parse = final_url.clone();
            let headers_for_parse = ctx.response_headers.clone();
            let peer_cert_for_parse = peer_cert.clone();
            let dns_json_for_parse = host_facts
                .as_ref()
                .and_then(|f| f.dns_json.as_deref())
                .map(str::to_string);
            let open_ports_for_parse = open_ports.clone().unwrap_or_default();
            let manifest_present = host_facts
                .as_ref()
                .and_then(|f| f.manifest_present)
                .unwrap_or(false);
            let service_worker_present = host_facts
                .as_ref()
                .and_then(|f| f.service_worker_present)
                .unwrap_or(false);
            let html_for_parse = html;
            let (asset_refs, mut links, tech_report) = tokio::task::spawn_blocking(move || {
                let doc = scraper::Html::parse_document(&html_for_parse);
                let tech_report =
                    crate::discovery::tech_fingerprint::analyze_with_facts_from_document(
                        &url_for_parse,
                        &final_url_for_parse,
                        headers_for_parse.as_ref(),
                        &html_for_parse,
                        &doc,
                        crate::discovery::tech_fingerprint::TechFingerprintFacts {
                            peer_cert: peer_cert_for_parse.as_ref(),
                            dns_json: dns_json_for_parse.as_deref(),
                            open_ports: &open_ports_for_parse,
                            manifest_present,
                            service_worker_present,
                        },
                    );
                let asset_refs = crate::discovery::asset_refs::extract_asset_refs_from_document(
                    &url_for_parse,
                    &doc,
                    &target_root,
                );
                let links =
                    crate::discovery::links::extract_links_from_document(&url_for_parse, &doc);
                (asset_refs, links, tech_report)
            })
            .await
            .map_err(|e| Error::Other(anyhow::anyhow!("html parse join: {e}")))?;
            self.save_and_emit_tech_report(&job.url, tech_report).await;
            if !asset_refs.is_empty() {
                let _ = self.storage.save_asset_refs(&asset_refs).await;
            }
            links.extend(cdp_urls);
            ctx.captured_urls = links;
            let _ = self.fire_all(HookEvent::OnDiscovery, &mut ctx).await?;

            let next_depth = job.depth + 1;
            let cap_ok = self
                .config
                .max_depth
                .map(|m| next_depth <= m)
                .unwrap_or(true);

            if cap_ok {
                let discovered = std::mem::take(&mut ctx.captured_urls);
                for to in discovered {
                    if !self.should_follow(&job.url, &to) {
                        continue;
                    }
                    if !self.dedupe.insert_url_set(&to) {
                        continue;
                    }
                    self.graph.add_edge(&job.url, &to);
                    let _ = self.storage.save_edge(&job.url, &to).await;
                    let job2 = Job {
                        id: self.next_id.fetch_add(1, Ordering::Relaxed),
                        url: to,
                        depth: next_depth,
                        priority: -(next_depth as i32),
                        method: job.method,
                        attempts: 0,
                        last_error: None,
                    };
                    self.queue.push(job2).await?;
                }
            }
        }

        let _ = self.fire_all(HookEvent::OnJobEnd, &mut ctx).await?;
        Ok(JobDisposition::Complete)
    }

    async fn save_and_emit_tech_report(
        &self,
        job_url: &Url,
        tech_report: crate::discovery::tech_fingerprint::TechFingerprintReport,
    ) {
        let _ = self.storage.save_tech_fingerprint(&tech_report).await;
        if tech_report.technologies.is_empty() {
            return;
        }
        let confidence_max = tech_report
            .technologies
            .iter()
            .map(|t| t.confidence)
            .max()
            .unwrap_or(0);
        let technologies: Vec<_> = tech_report
            .technologies
            .iter()
            .map(|t| {
                serde_json::json!({
                    "slug": &t.slug,
                    "name": &t.name,
                    "category": &t.category,
                    "confidence": t.confidence,
                })
            })
            .collect();
        self.events.emit(
            &Event::of(EventKind::TechFingerprintDetected)
                .with_run(self.run_id)
                .with_url(job_url.as_str())
                .with_data(&serde_json::json!({
                    "host": &tech_report.host,
                    "url": &tech_report.url,
                    "technologies": technologies,
                    "confidence_max": confidence_max,
                })),
        );
    }

    fn record_host_facts(&self, host: &str, facts: &HostFacts) {
        if host.is_empty() {
            return;
        }
        let key = host.to_ascii_lowercase();
        if let Some(mut current) = self.host_facts.get_mut(&key) {
            merge_host_facts(&mut *current, facts);
        } else {
            self.host_facts.insert(key, facts.clone());
        }
    }

    fn host_facts_snapshot(&self, final_url: &Url, original_url: &Url) -> Option<HostFacts> {
        if let Some(final_host) = final_url.host_str() {
            if let Some(facts) = self.host_facts.get(&final_host.to_ascii_lowercase()) {
                return Some(facts.clone());
            }
            if !original_url
                .host_str()
                .is_some_and(|h| h.eq_ignore_ascii_case(final_host))
            {
                return None;
            }
        }
        if let Some(original_host) = original_url.host_str() {
            if let Some(facts) = self.host_facts.get(&original_host.to_ascii_lowercase()) {
                return Some(facts.clone());
            }
        }
        None
    }

    fn record_open_ports(&self, host: &str, ports: Vec<u16>) {
        if host.is_empty() || ports.is_empty() {
            return;
        }
        let key = host.to_ascii_lowercase();
        if let Some(mut current) = self.host_open_ports.get_mut(&key) {
            let mut merged: std::collections::BTreeSet<u16> = current.iter().copied().collect();
            merged.extend(ports);
            *current = merged.into_iter().collect();
        } else {
            let mut sorted = ports;
            sorted.sort_unstable();
            sorted.dedup();
            self.host_open_ports.insert(key, sorted);
        }
    }

    fn open_ports_snapshot(&self, final_url: &Url, original_url: &Url) -> Option<Vec<u16>> {
        if let Some(final_host) = final_url.host_str() {
            if let Some(ports) = self.host_open_ports.get(&final_host.to_ascii_lowercase()) {
                return Some(ports.clone());
            }
            if !original_url
                .host_str()
                .is_some_and(|h| h.eq_ignore_ascii_case(final_host))
            {
                return None;
            }
        }
        if let Some(original_host) = original_url.host_str() {
            if let Some(ports) = self
                .host_open_ports
                .get(&original_host.to_ascii_lowercase())
            {
                return Some(ports.clone());
            }
        }
        None
    }

    fn should_follow(&self, from: &Url, to: &Url) -> bool {
        if to.scheme() != "http" && to.scheme() != "https" {
            return false;
        }
        let from_host = from.host_str().unwrap_or("");
        let to_host = to.host_str().unwrap_or("");
        // Require at least one dot — rejects bogus "href=https" style hosts
        // and keeps localhost-only crawls explicit.
        if !to_host.contains('.') {
            return false;
        }
        // Target-scoped mode overrides the legacy same_host/include_subdomains
        // flags: when `target_domain` is set, only URLs whose registrable
        // domain equals the target (exact match, subdomains included) are
        // crawled. Out-of-scope URLs are still recorded in `asset_refs`
        // via the page-harvest path — this gate only prevents the
        // FETCH, not the bookkeeping.
        if let Some(target) = &self.config.target_domain {
            match registrable(to_host) {
                Some(th) if th.eq_ignore_ascii_case(target) => {}
                _ => return false,
            }
        } else {
            if self.config.same_host_only && from_host != to_host {
                return false;
            }
            if self.config.include_subdomains {
                if let (Some(fh), Some(th)) = (registrable(from_host), registrable(to_host)) {
                    if fh != th {
                        return false;
                    }
                }
            }
        }
        if self.config.follow_pages_only {
            let kind = classify_url(to);
            match kind {
                AssetKind::Page
                | AssetKind::Document
                | AssetKind::Api
                | AssetKind::Sitemap
                | AssetKind::Json => {}
                _ => return false,
            }
        }
        // User-supplied allowlist regex via --on-discovery-filter-regex. If
        // present, the URL must match to be enqueued. Compiled once at
        // Crawler::new.
        if let Some(re) = self.discovery_filter.as_ref() {
            if !re.is_match(to.as_str()) {
                return false;
            }
        }
        true
    }

    #[cfg(feature = "cdp-backend")]
    /// Persist an extra snapshot pair (HTML + optional PNG) when an
    /// antibot challenge was detected. Keyed by vendor + session so
    /// triage tools can group attempts without scanning every artifact.
    /// Best-effort: all filesystem errors become debug logs.
    #[cfg(feature = "cdp-backend")]
    fn write_challenge_snapshot(
        &self,
        vendor: &crate::antibot::ChallengeVendor,
        session_id: &str,
        html: &str,
        png: Option<&[u8]>,
    ) -> Result<()> {
        let Some(dir) = self.config.output.screenshot_dir.as_deref() else {
            return Ok(());
        };
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::debug!(?e, dir, "mkdir challenge snapshot dir failed");
            return Ok(());
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let safe_sid: String = session_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let stem = format!("challenge_{}_{}_{}", vendor.as_str(), safe_sid, ts);
        let html_path = std::path::Path::new(dir).join(format!("{stem}.html"));
        if let Err(e) = std::fs::write(&html_path, html.as_bytes()) {
            tracing::debug!(?e, path=%html_path.display(), "challenge html write failed");
        }
        if let Some(png) = png {
            let png_path = std::path::Path::new(dir).join(format!("{stem}.png"));
            if let Err(e) = std::fs::write(&png_path, png) {
                tracing::debug!(?e, path=%png_path.display(), "challenge png write failed");
            }
        }
        Ok(())
    }

    #[cfg(feature = "cdp-backend")]
    fn write_screenshot_output(&self, url: &Url, png: &[u8]) -> Result<()> {
        let Some(dir) = self.config.output.screenshot_dir.as_deref() else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::Storage(format!("mkdir screenshot-dir {dir}: {e}")))?;
        let name = format!(
            "{}.png",
            hex::encode(Sha256::digest(url.as_str().as_bytes()))
        );
        let path = std::path::Path::new(dir).join(name);
        std::fs::write(&path, png)
            .map_err(|e| Error::Storage(format!("write screenshot {}: {e}", path.display())))?;
        Ok(())
    }

    async fn ensure_robots(&self, url: &Url) -> Result<()> {
        let host = match url.host_str() {
            Some(h) => h.to_string(),
            None => return Ok(()),
        };
        let ua = self.client.identity_bundle().ua.as_str();
        if self.robots.check(url, ua).is_some() {
            return Ok(());
        }
        let robots_url = Url::parse(&format!("{}://{}/robots.txt", url.scheme(), host))?;
        match self.client.get(&robots_url).await {
            Ok(r) if r.status.is_success() => {
                let body = String::from_utf8_lossy(&r.body).into_owned();
                self.robots.store(&host, Some(&body), ua)?;
                let facts = HostFacts {
                    robots_present: Some(true),
                    ..HostFacts::default()
                };
                self.record_host_facts(&host, &facts);
                let _ = self.storage.save_host_facts(&host, &facts).await;
                self.seed_sitemaps_from_robots(&body, url).await;
                if self.config.robots_paths_enabled {
                    self.seed_robots_paths(&body, url).await;
                }
            }
            _ => {
                self.robots.store(&host, None, ua)?;
                let facts = HostFacts {
                    robots_present: Some(false),
                    ..HostFacts::default()
                };
                self.record_host_facts(&host, &facts);
                let _ = self.storage.save_host_facts(&host, &facts).await;
            }
        }
        Ok(())
    }

    async fn per_host_probes(&self, origin_url: &Url) {
        let origin_root = match Url::parse(&format!(
            "{}://{}/",
            origin_url.scheme(),
            origin_url.host_str().unwrap_or("")
        )) {
            Ok(u) => u,
            Err(_) => return,
        };
        let host = origin_root.host_str().unwrap_or("").to_string();
        let mut facts = crate::storage::HostFacts::default();

        // DNS enumeration — cheap, surfaces related infrastructure hosts.
        // Active port probing reuses the same DNS answer when explicitly
        // enabled, so we don't resolve the host twice.
        if (self.config.dns_enabled || self.config.infra_intel.network_probe) && !host.is_empty() {
            let dns = crate::discovery::dns::lookup(&host).await;
            let mut resolved_ips: Vec<std::net::IpAddr> = dns
                .a
                .iter()
                .copied()
                .chain(dns.aaaa.iter().copied())
                .collect();
            resolved_ips.sort_unstable();
            resolved_ips.dedup();
            let mut cloud_tags: Vec<String> = resolved_ips
                .iter()
                .filter_map(|ip| crate::discovery::network_probe::cloud_lookup(*ip))
                .map(|tag| match tag.service {
                    Some(service) => format!("{}:{service}", tag.provider),
                    None => tag.provider.to_string(),
                })
                .collect();
            cloud_tags.sort();
            cloud_tags.dedup();
            if self.config.dns_enabled && !dns.related_hosts.is_empty() {
                for rel in &dns.related_hosts {
                    // Only seed related hosts that sit under the registrable.
                    if !related_host_is_interesting(&host, rel) {
                        continue;
                    }
                    if let Ok(u) = Url::parse(&format!("https://{rel}/")) {
                        if self.dedupe.insert_url_set(&u) {
                            let job = Job {
                                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                                url: u,
                                depth: 0,
                                priority: 2,
                                method: FetchMethod::HttpSpoof,
                                attempts: 0,
                                last_error: None,
                            };
                            let _ = self.queue.push(job).await;
                        }
                    }
                }
            }
            if self.config.dns_enabled || !cloud_tags.is_empty() {
                facts.dns_json = serde_json::to_string(&serde_json::json!({
                    "a":    dns.a.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                    "aaaa": dns.aaaa.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                    "cname": &dns.cname,
                    "mx":   &dns.mx,
                    "txt":  &dns.txt,
                    "ns":   &dns.ns,
                    "caa":  &dns.caa,
                    "cloud": &cloud_tags,
                }))
                .ok();
            }
            if self.config.infra_intel.network_probe {
                let mut open_ports = std::collections::BTreeSet::new();
                for ip in resolved_ips {
                    let probes = crate::discovery::network_probe::tcp_probe_ports(
                        ip,
                        crate::discovery::network_probe::TOP_PORTS,
                        std::time::Duration::from_millis(800),
                    )
                    .await;
                    for probe in probes {
                        if matches!(
                            probe.state,
                            crate::discovery::network_probe::PortState::Open
                        ) {
                            open_ports.insert(probe.port);
                        }
                    }
                }
                if !open_ports.is_empty() {
                    self.record_open_ports(&host, open_ports.into_iter().collect());
                }
            }
        }

        // Favicon fingerprint (shodan-style mmh3). Also piggyback the peer
        // cert extraction from the same TLS session when enabled.
        if self.config.favicon_enabled || self.config.collect_peer_cert {
            if let Ok(fav_url) = origin_root.join("/favicon.ico") {
                if let Ok(resp) = self
                    .client
                    .get_with_dest(&fav_url, crate::discovery::assets::SecFetchDest::Image)
                    .await
                {
                    if self.config.favicon_enabled
                        && resp.status.is_success()
                        && !resp.body.is_empty()
                    {
                        let h = crate::discovery::favicon::favicon_mmh3(&resp.body);
                        facts.favicon_mmh3 = Some(h);
                    }
                    if self.config.collect_peer_cert {
                        if let Some(cert) = resp.peer_cert.as_ref() {
                            facts.cert_sha256 = cert.sha256.clone();
                            facts.cert_subject_cn = cert.subject_cn.clone();
                            facts.cert_issuer_cn = cert.issuer_cn.clone();
                            facts.cert_not_before = cert.not_before.clone();
                            facts.cert_not_after = cert.not_after.clone();
                            facts.cert_sans_json = serde_json::to_string(&cert.sans).ok();
                            // Seed SANs that fall under our registrable.
                            for san in &cert.sans {
                                let target = san.trim_start_matches("*.");
                                if !related_host_is_interesting(&host, target) {
                                    continue;
                                }
                                if let Ok(u) = Url::parse(&format!("https://{target}/")) {
                                    if self.dedupe.insert_url_set(&u) {
                                        let job = Job {
                                            id: self.next_id.fetch_add(1, Ordering::Relaxed),
                                            url: u,
                                            depth: 0,
                                            priority: 2,
                                            method: FetchMethod::HttpSpoof,
                                            attempts: 0,
                                            last_error: None,
                                        };
                                        let _ = self.queue.push(job).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // RDAP lookup per registrable (one request per domain, cheap).
        if self.config.rdap_enabled {
            if let Some(root) = crate::discovery::subdomains::registrable_domain(&host) {
                if self.rdap_done.insert(root.clone()) {
                    if let Ok(reg) = crate::discovery::whois::lookup(&self.client, &root).await {
                        facts.registrar = reg.registrar.clone();
                        facts.registrant_org = reg.registrant_org.clone();
                        facts.registration_created = reg.created.clone();
                        facts.registration_expires = reg.expires.clone();
                        facts.rdap_json = serde_json::to_string(&reg).ok();
                        // Seed nameserver hosts (often under same registrar,
                        // sometimes reveals infra).
                        for ns in &reg.name_servers {
                            if !related_host_is_interesting(&host, ns) {
                                continue;
                            }
                            if let Ok(u) = Url::parse(&format!("https://{ns}/")) {
                                if self.dedupe.insert_url_set(&u) {
                                    let job = Job {
                                        id: self.next_id.fetch_add(1, Ordering::Relaxed),
                                        url: u,
                                        depth: 0,
                                        priority: 1,
                                        method: FetchMethod::HttpSpoof,
                                        attempts: 0,
                                        last_error: None,
                                    };
                                    let _ = self.queue.push(job).await;
                                }
                            }
                        }
                    }
                }
            }
        }

        // .well-known probes.
        if self.config.well_known_enabled {
            for wk in crate::discovery::well_known::probe_urls(&origin_root) {
                let Ok(resp) = self.client.get(&wk).await else {
                    continue;
                };
                if !resp.status.is_success() || resp.body.is_empty() {
                    continue;
                }
                let body = String::from_utf8_lossy(&resp.body);
                let is_security_txt = wk.path().ends_with("/security.txt");
                let urls = if is_security_txt {
                    let st = crate::discovery::security_txt::parse(&body);
                    let fields: Vec<Url> = crate::discovery::security_txt::url_fields(&st)
                        .into_iter()
                        .filter_map(|s| Url::parse(s).ok())
                        .collect();
                    if let Ok(json) = serde_json::to_string(&st) {
                        // Store alongside other host facts as a JSON blob inside
                        // rdap_json temporarily? No — piggyback on rdap_json is
                        // wrong. We keep it out-of-band: dns_json column carries
                        // only dns. We log it via tracing for now; a dedicated
                        // column can be added later if we actually query it.
                        tracing::debug!(host=%host, security_txt=%json, "security.txt parsed");
                    }
                    fields
                } else {
                    crate::discovery::well_known::extract_urls_from_body(&body)
                };
                for u in urls {
                    if !self.should_follow(&origin_root, &u) {
                        continue;
                    }
                    if self.dedupe.insert_url_set(&u) {
                        let job = Job {
                            id: self.next_id.fetch_add(1, Ordering::Relaxed),
                            url: u,
                            depth: 1,
                            priority: 3,
                            method: FetchMethod::HttpSpoof,
                            attempts: 0,
                            last_error: None,
                        };
                        let _ = self.queue.push(job).await;
                    }
                }
            }
        }

        // PWA manifest + service worker probes.
        if self.config.pwa_enabled {
            for pwa_url in crate::discovery::pwa::probe_urls(&origin_root) {
                let Ok(resp) = self.client.get(&pwa_url).await else {
                    continue;
                };
                if !resp.status.is_success() || resp.body.is_empty() {
                    continue;
                }
                let path = pwa_url.path();
                let body = String::from_utf8_lossy(&resp.body);
                let urls = if path.ends_with("manifest.json") || path.ends_with(".webmanifest") {
                    facts.manifest_present = Some(true);
                    crate::discovery::pwa::extract_urls_from_manifest(&pwa_url, &body)
                } else if path.ends_with(".js") {
                    facts.service_worker_present = Some(true);
                    crate::discovery::pwa::extract_service_workers_from_js(&pwa_url, &body)
                } else {
                    Vec::new()
                };
                for u in urls {
                    if !self.should_follow(&origin_root, &u) {
                        continue;
                    }
                    if self.dedupe.insert_url_set(&u) {
                        let job = Job {
                            id: self.next_id.fetch_add(1, Ordering::Relaxed),
                            url: u,
                            depth: 1,
                            priority: 2,
                            method: FetchMethod::HttpSpoof,
                            attempts: 0,
                            last_error: None,
                        };
                        let _ = self.queue.push(job).await;
                    }
                }
            }
        }

        // Wayback Machine historical URL seeding.
        if self.config.wayback_enabled {
            if let Some(root) = crate::discovery::subdomains::registrable_domain(&host) {
                if let Ok(urls) = crate::discovery::wayback::wayback_urls(&self.client, &root).await
                {
                    for u in urls {
                        if !self.should_follow(&origin_root, &u) {
                            continue;
                        }
                        if self.dedupe.insert_url_set(&u) {
                            let job = Job {
                                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                                url: u,
                                depth: 1,
                                priority: -2,
                                method: FetchMethod::HttpSpoof,
                                attempts: 0,
                                last_error: None,
                            };
                            let _ = self.queue.push(job).await;
                        }
                    }
                }
            }
        }

        self.record_host_facts(&host, &facts);
        let _ = self.storage.save_host_facts(&host, &facts).await;
    }

    async fn seed_robots_paths(&self, body: &str, origin: &Url) {
        let paths = crate::discovery::robots_paths::extract_paths(body);
        let urls = crate::discovery::robots_paths::seed_urls(origin, &paths);
        for u in urls {
            if !self.should_follow(origin, &u) {
                continue;
            }
            if !self.dedupe.insert_url_set(&u) {
                continue;
            }
            let job = Job {
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                url: u,
                depth: 1,
                priority: 5, // Prioritize — these paths are often high-value.
                method: FetchMethod::HttpSpoof,
                attempts: 0,
                last_error: None,
            };
            let _ = self.queue.push(job).await;
        }
    }

    /// Seed crt.sh subdomains for the registrable domain of `origin`. Runs at
    /// most once per registrable; deduped via the frontier bloom filter.
    pub async fn seed_crtsh(&self, origin: &Url) -> Result<()> {
        if !self.config.crtsh_enabled {
            return Ok(());
        }
        let Some(host) = origin.host_str() else {
            return Ok(());
        };
        let Some(root) = crate::discovery::subdomains::registrable_domain(host) else {
            return Ok(());
        };
        let subs = crate::discovery::subdomains::crtsh_subdomains(&self.client, &root).await?;
        let scheme = origin.scheme();
        for sub in subs {
            let Ok(u) = Url::parse(&format!("{scheme}://{sub}/")) else {
                continue;
            };
            if !self.dedupe.insert_url_set(&u) {
                continue;
            }
            let job = Job {
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                url: u,
                depth: 0,
                priority: 3,
                method: FetchMethod::HttpSpoof,
                attempts: 0,
                last_error: None,
            };
            let _ = self.queue.push(job).await;
        }
        Ok(())
    }

    async fn seed_sitemaps_from_robots(&self, body: &str, origin: &Url) {
        let sitemaps = crate::discovery::sitemap::sitemap_urls_from_robots(body);
        for sm in sitemaps {
            let Ok(resp) = self.client.get(&sm).await else {
                continue;
            };
            if !resp.status.is_success() {
                continue;
            }
            let xml = String::from_utf8_lossy(&resp.body);
            let urls = crate::discovery::sitemap::urls_from_sitemap_xml(&xml);
            for u in urls {
                if !self.should_follow(origin, &u) {
                    continue;
                }
                if !self.dedupe.insert_url_set(&u) {
                    continue;
                }
                let job = Job {
                    id: self.next_id.fetch_add(1, Ordering::Relaxed),
                    url: u,
                    depth: 1,
                    priority: -1,
                    method: FetchMethod::HttpSpoof,
                    attempts: 0,
                    last_error: None,
                };
                let _ = self.queue.push(job).await;
            }
        }
    }
}

fn registrable(host: &str) -> Option<String> {
    crate::discovery::subdomains::registrable_domain(host)
}

/// Exponential backoff with jitter, capped at 300s. `attempts` is the count
/// *before* this failure — 0 on first try, N on Nth retry. `base_ms` is the
/// configured starting delay (typically from `Config.retry_backoff`).
fn backoff_seconds(attempts: u32, base_ms: u64) -> u64 {
    let multiplier = 1u64.checked_shl(attempts.min(8)).unwrap_or(256);
    let jitter = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0))
        % 500;
    (base_ms.saturating_mul(multiplier) + jitter).min(300_000) / 1_000
}

fn duration_to_queue_secs(delay: Duration) -> u64 {
    if delay.is_zero() {
        return 0;
    }
    let millis = delay.as_millis();
    ((millis.saturating_add(999)) / 1_000).min(u64::MAX as u128) as u64
}

fn related_host_is_interesting(origin: &str, related: &str) -> bool {
    let Some(oroot) = crate::discovery::subdomains::registrable_domain(origin) else {
        return false;
    };
    let Some(rroot) = crate::discovery::subdomains::registrable_domain(related) else {
        return false;
    };
    oroot == rroot
}

fn merge_host_facts(dst: &mut HostFacts, src: &HostFacts) {
    macro_rules! merge_some {
        ($field:ident) => {
            if src.$field.is_some() {
                dst.$field = src.$field.clone();
            }
        };
    }

    merge_some!(favicon_mmh3);
    merge_some!(dns_json);
    merge_some!(robots_present);
    merge_some!(manifest_present);
    merge_some!(service_worker_present);
    merge_some!(cert_sha256);
    merge_some!(cert_subject_cn);
    merge_some!(cert_issuer_cn);
    merge_some!(cert_not_before);
    merge_some!(cert_not_after);
    merge_some!(cert_sans_json);
    merge_some!(rdap_json);
    merge_some!(registrar);
    merge_some!(registrant_org);
    merge_some!(registration_created);
    merge_some!(registration_expires);
}

/// Generate a stable run_id for this Crawler instance. Format: pid in
/// upper bits, monotonic counter in lower bits — collision-free per
/// process, easy to correlate with logs that already log pid.
fn gen_run_id() -> u64 {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id() as u64;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    (pid << 32) | (n & 0xffff_ffff)
}

/// RAII guard that emits `run.completed` when `run()` returns, regardless
/// of whether it returned `Ok` or `Err`. Keeps the contract honest: every
/// `run.started` is always followed by exactly one `run.completed`.
struct RunCompletedGuard {
    sink: Arc<dyn EventSink>,
    run_id: u64,
}

impl Drop for RunCompletedGuard {
    fn drop(&mut self) {
        self.sink
            .emit(&Event::of(EventKind::RunCompleted).with_run(self.run_id));
        self.sink.flush();
    }
}
