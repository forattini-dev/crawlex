use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::impersonate::Profile;
use crate::proxy::RotationStrategy;
use crate::wait_strategy::WaitStrategy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub max_concurrent_render: usize,
    pub max_concurrent_http: usize,
    pub max_depth: Option<u32>,
    pub same_host_only: bool,
    pub include_subdomains: bool,
    /// Primary recon target. When set, the frontier accepts URLs whose
    /// registrable domain equals this value (and their subdomains) and
    /// rejects the rest; out-of-scope URLs are still recorded in
    /// `asset_refs` so the operator sees every cross-site reference
    /// without fetching the content. Coexists with `same_host_only` /
    /// `include_subdomains` — when `target_domain` is set it wins.
    #[serde(default)]
    pub target_domain: Option<String>,
    /// Per-stage toggles for `crawlex intel <target>` (Fase B+). A
    /// `crawlex crawl --target <domain>` run with `with_infra=true`
    /// consults this to decide which passive-intel stages to execute.
    #[serde(default)]
    pub infra_intel: InfraIntelConfig,
    /// Index into `identity::profiles::catalog()` picking the persona
    /// the render pool should project. `None` keeps the historical
    /// Intel-on-Linux default wired through `IdentityBundle::from_chromium`;
    /// `Some(i)` routes through `IdentityBundle::from_persona(catalog()[i], …)`
    /// so the operator can switch the OS/GPU face crawlex shows without
    /// rebuilding.
    #[serde(default)]
    pub identity_preset: Option<u8>,
    pub respect_robots_txt: bool,
    pub user_agent_profile: Profile,
    pub chrome_path: Option<String>,
    pub chrome_flags: Vec<String>,
    pub block_resources: Vec<String>,
    pub wait_strategy: WaitStrategy,
    pub rate_per_host_rps: Option<f64>,
    pub retry_max: u32,
    pub retry_backoff: Duration,
    pub queue_backend: QueueBackend,
    pub storage_backend: StorageBackend,
    pub output: OutputConfig,
    pub proxy: ProxyConfig,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub metrics_prometheus_port: Option<u16>,
    pub hook_scripts: Vec<String>,
    pub discovery_filter_regex: Option<String>,
    /// When true, only follow URLs classified as Page/Document/Api.
    pub follow_pages_only: bool,
    /// Seed the frontier with crt.sh-discovered subdomains of each seed host.
    pub crtsh_enabled: bool,
    /// Expand robots.txt Disallow/Allow paths into seed URLs.
    pub robots_paths_enabled: bool,
    /// Probe /.well-known/* endpoints and harvest URLs from their bodies.
    pub well_known_enabled: bool,
    /// Probe PWA manifest and service worker paths; parse manifest for URLs.
    pub pwa_enabled: bool,
    /// Query the Internet Archive CDX API to seed historical URLs.
    pub wayback_enabled: bool,
    /// Fetch favicon.ico and compute its Shodan-style mmh3 hash.
    pub favicon_enabled: bool,
    /// Resolve DNS records per host; seed related_hosts as new roots.
    pub dns_enabled: bool,
    /// Opt-in: measure DNS/TCP/TLS/TTFB/download on the HTTP path and store.
    pub collect_net_timings: bool,
    /// Opt-in: run the Web Vitals JS after render and store (CLS, LCP, etc.).
    pub collect_web_vitals: bool,
    /// Opt-in: extract peer TLS certificate (CN, SANs, fingerprint) and seed
    /// SANs as candidate subdomains.
    pub collect_peer_cert: bool,
    /// Opt-in: RDAP lookup per registrable domain.
    pub rdap_enabled: bool,
    /// Persist cookies per registrable domain across requests.
    pub cookies_enabled: bool,
    /// Render-session reuse boundary. Controls how aggressively browser
    /// state is shared between rendered pages.
    #[serde(default)]
    pub render_session_scope: RenderSessionScope,
    /// Follow 3xx redirects inline.
    pub follow_redirects: bool,
    /// Max redirects before returning the redirect response as-is.
    pub max_redirects: u8,
    /// Action script executed on every rendered page after the wait strategy.
    /// Only present when the render backend is compiled in; mini builds
    /// skip the field entirely.
    #[cfg(feature = "cdp-backend")]
    #[serde(skip)]
    pub actions: Option<Vec<crate::render::actions::Action>>,
    /// Declarative ScriptSpec (v1) run on every rendered page in place of
    /// `actions`. When both are set, `script_spec` wins — the CLI wires
    /// `conflicts_with` so operators can't accidentally ship both. The
    /// runner slots in between wait-strategy settle and the Lua
    /// `on_after_load` hook.
    #[cfg(feature = "cdp-backend")]
    #[serde(skip)]
    pub script_spec: Option<crate::script::ScriptSpec>,
    /// When true, the render pool runs `<chrome> --version` once at startup
    /// and rewrites `user_agent_profile` to the closest known profile. This
    /// avoids the "spoof says Chrome/131, render is Chrome/149" mismatch.
    pub profile_autodetect: bool,
    /// When set, overrides the UA string both in spoof request headers and
    /// in the Chrome `--user-agent` launch flag. Takes precedence over the
    /// profile's canned UA.
    pub user_agent_override: Option<String>,
    /// When true, and no system Chrome is found on PATH (and no `chrome_path`
    /// was set), auto-download a pinned Chromium-for-Testing build into
    /// `$XDG_CACHE_HOME/crawlex/chromium/` via `the CDP fetcher`.
    pub auto_fetch_chromium: bool,
    /// Per-verb action policy applied to every ScriptSpec / `--actions-file`
    /// execution. Default is `permissive` (all verbs allowed) so legacy
    /// scripts authored by the operator keep working; callers running
    /// untrusted scripts should swap in `ActionPolicy::strict()` or a
    /// JSON-loaded variant via `--action-policy <path>`.
    #[serde(default)]
    pub action_policy: crate::policy::ActionPolicy,
    /// How the crawler should treat detected captcha/challenge flows.
    /// `avoidance` keeps the product strictly prevention-only; `solver_ready`
    /// enriches challenge telemetry with widget metadata so a future solver
    /// integration can slot in without changing the capture contract.
    #[serde(default)]
    pub challenge_mode: ChallengeMode,
    /// Inject the SPA/PWA JS observer (history + fetch + XHR wrappers)
    /// on every rendered page and emit `snapshot.runtime_routes`
    /// artifacts post-settle. Cheap; on by default. Disabling also
    /// stops routes from feeding the crawler frontier.
    #[serde(default = "default_true")]
    pub collect_runtime_routes: bool,
    /// Emit `snapshot.network_endpoints` artifacts post-settle and
    /// forward observed endpoints to the frontier. Shares the
    /// observer bundle with `collect_runtime_routes` — disabling
    /// only suppresses the artifact/frontier wire.
    #[serde(default = "default_true")]
    pub collect_network_endpoints: bool,
    /// Enumerate IndexedDB databases/object stores via CDP post-settle
    /// and emit `snapshot.indexeddb`. Heavy; off by default.
    #[serde(default)]
    pub collect_indexeddb: bool,
    /// Enumerate Cache Storage caches/keys via CDP post-settle and
    /// emit `snapshot.cache_storage`. Heavy; off by default.
    #[serde(default)]
    pub collect_cache_storage: bool,
    /// Fetch the Web App Manifest (when discovered via `<link rel=manifest>`)
    /// and emit `snapshot.manifest`. Default on, cheap (one HTTP fetch).
    #[serde(default = "default_true")]
    pub collect_manifest: bool,
    /// Emit `snapshot.service_workers` post-settle from the
    /// registrations already captured in origin state. Default on.
    #[serde(default = "default_true")]
    pub collect_service_workers: bool,
    /// Max Chrome instances kept alive simultaneously. When exceeded,
    /// the LRU browser is evicted — its tabs + contexts torn down. Keyed
    /// on `(proxy_url | "")` in the render pool.
    #[serde(default = "default_max_browsers")]
    pub max_browsers: usize,
    /// Max idle + in-flight pages per BrowserContext the render pool
    /// keeps reusable. Higher = more parallel tabs per session, lower
    /// memory reuse.
    #[serde(default = "default_max_pages_per_context")]
    pub max_pages_per_context: usize,
    /// Inflight budgets enforced per-host/origin/proxy/session before a
    /// render job starts. Jobs that exceed a budget are re-queued with
    /// a small delay.
    #[serde(default)]
    pub render_budgets: crate::scheduler::BudgetLimits,
    /// Render session time-to-live: the cleanup task drops BrowserContexts
    /// that haven't been touched in this many seconds. Default 3600 (1h).
    #[serde(default = "default_session_ttl_secs")]
    pub session_ttl_secs: u64,
    /// When `true`, a session transitioning to `Blocked` is evicted
    /// immediately (without waiting for the TTL cleanup sweep). Default on
    /// so hostile sites don't tie up a BrowserContext until TTL fires.
    #[serde(default = "default_true")]
    pub drop_session_on_block: bool,
    /// When `true`, policy can automatically demote `render_session_scope`
    /// based on page signals (login pages → Origin, hard blocks → Url).
    /// Turn off to pin the scope the CLI/config declared. Default off —
    /// operators opt in.
    #[serde(default)]
    pub session_scope_auto: bool,
    /// Human motion engine preset. Trades throughput for trajectory
    /// realism. `fast` keeps the legacy ~15 rps baseline (linear path,
    /// minimal delay); `balanced` (default) wires WindMouse + Fitts + OU
    /// jitter at ~8 rps; `human` and `paranoid` favour stealth over
    /// speed. See `crate::render::motion::MotionProfile` for the params.
    ///
    /// Gated on `cdp-backend` because the mini build ships no browser
    /// primitives to feed — a mini operator tuning stealth params would
    /// be a no-op.
    #[cfg(feature = "cdp-backend")]
    #[serde(default)]
    pub motion_profile: crate::render::motion::MotionProfile,
    /// Pre-navigation warm-up hit. Cloudflare scores the *first* request to
    /// an origin more harshly because `__cf_bm`/`cf_clearance` cookies
    /// aren't bound yet — visiting a cheap URL first lets the cookie store
    /// catch up before the scored request. Opt-in via config.
    #[serde(default)]
    pub warmup: WarmupPolicy,
    /// Post-settle "reading" dwell. Real humans linger on a page proportional
    /// to its text length — reCAPTCHA v3 / DataDome flag instant post-load
    /// extraction as bot-like. When `Some(cfg)` with `enabled=true`, the
    /// render pool sleeps for `(words / wpm) * 60_000 + jitter` ms after the
    /// wait strategy settles. Off by default so existing throughput stays put.
    #[serde(default)]
    pub reading_dwell: Option<ReadingDwellConfig>,
    /// Limits for spoofed HTTP fetches. These caps protect high-concurrency
    /// crawls from slow bodies and compressed bombs.
    #[serde(default)]
    pub http_limits: HttpLimits,
    /// Content-addressed body store. When enabled, full response/rendered
    /// bodies live in blobs and `pages` stores hashes + paths. Legacy inline
    /// columns stay opt-in for older direct SQL consumers.
    #[serde(default)]
    pub content_store: ContentStoreConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentStoreConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub inline_legacy_columns: bool,
}

impl Default for ContentStoreConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            root: None,
            inline_legacy_columns: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpLimits {
    #[serde(default = "default_http_request_timeout")]
    pub request_timeout: Duration,
    #[serde(default = "default_max_encoded_body_bytes")]
    pub max_encoded_body_bytes: Option<usize>,
    #[serde(default = "default_max_decoded_body_bytes")]
    pub max_decoded_body_bytes: Option<usize>,
    #[serde(default = "default_max_decompression_ratio")]
    pub max_decompression_ratio: usize,
    #[serde(default)]
    pub store_truncated_bodies: bool,
}

fn default_http_request_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_max_encoded_body_bytes() -> Option<usize> {
    Some(16 * 1024 * 1024)
}

fn default_max_decoded_body_bytes() -> Option<usize> {
    Some(32 * 1024 * 1024)
}

fn default_max_decompression_ratio() -> usize {
    100
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            request_timeout: default_http_request_timeout(),
            max_encoded_body_bytes: default_max_encoded_body_bytes(),
            max_decoded_body_bytes: default_max_decoded_body_bytes(),
            max_decompression_ratio: default_max_decompression_ratio(),
            store_truncated_bodies: false,
        }
    }
}

/// Reading dwell parameters. Gates a simulated "user reads the page"
/// delay between `settle_after_actions` and DOM serialization. WPM
/// defaults track typical adult prose reading (~250 wpm); jitter σ
/// keeps successive requests non-identical so fingerprint models can't
/// pin the exact cadence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadingDwellConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_reading_dwell_wpm")]
    pub wpm: u32,
    #[serde(default = "default_reading_dwell_jitter_ms")]
    pub jitter_ms: u64,
    #[serde(default = "default_reading_dwell_min_ms")]
    pub min_ms: u64,
    #[serde(default = "default_reading_dwell_max_ms")]
    pub max_ms: u64,
}

fn default_reading_dwell_wpm() -> u32 {
    250
}
fn default_reading_dwell_jitter_ms() -> u64 {
    40
}
fn default_reading_dwell_min_ms() -> u64 {
    500
}
fn default_reading_dwell_max_ms() -> u64 {
    10_000
}

impl Default for ReadingDwellConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            wpm: default_reading_dwell_wpm(),
            jitter_ms: default_reading_dwell_jitter_ms(),
            min_ms: default_reading_dwell_min_ms(),
            max_ms: default_reading_dwell_max_ms(),
        }
    }
}

/// Optional warm-up navigation performed before the crawl target.
///
/// The render core substitutes `{origin}` and `{host}` into `url_template`
/// against the target URL, navigates there, sleeps `dwell_ms`, then
/// continues to the real target. When the rendered template equals the
/// target we skip the warm-up (no point paying for a duplicate request).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WarmupPolicy {
    /// Master switch. Default `false` so the behaviour is strictly opt-in.
    #[serde(default)]
    pub enabled: bool,
    /// URL template with `{origin}` / `{host}` placeholders. Typical values:
    /// `"{origin}"` (origin root) or `"{origin}/search"`.
    #[serde(default = "default_warmup_template")]
    pub url_template: String,
    /// How long to idle on the warm-up URL before hitting the target. 1.5s
    /// is enough for Cloudflare's `__cf_bm` cookie to appear on the session
    /// without materially slowing a crawl.
    #[serde(default = "default_warmup_dwell_ms")]
    pub dwell_ms: u64,
}

fn default_warmup_template() -> String {
    "{origin}".to_string()
}

fn default_warmup_dwell_ms() -> u64 {
    1500
}

impl Default for WarmupPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            url_template: default_warmup_template(),
            dwell_ms: default_warmup_dwell_ms(),
        }
    }
}

impl WarmupPolicy {
    /// Substitute `{origin}` and `{host}` placeholders in `url_template`
    /// against `target`. Pure string op — no I/O, no URL parsing of the
    /// rendered output, so the caller decides what to do with garbage.
    ///
    /// `origin` is `scheme://host[:port]` (no trailing slash); `host` is
    /// the bare hostname. An empty `url_template` yields an empty string —
    /// the caller treats that as "skip warm-up".
    pub fn render_template(&self, target: &url::Url) -> String {
        let host = target.host_str().unwrap_or("");
        // Build `scheme://host[:port]` manually — `Url::origin()` stringifies
        // to `scheme://host:port` with the default-port elided which is what
        // we want, but it returns an `Origin` enum so we'd have to match on
        // `Tuple`. Hand-rolling is simpler and keeps the behaviour stable
        // across `url` crate versions.
        let mut origin = String::new();
        origin.push_str(target.scheme());
        origin.push_str("://");
        origin.push_str(host);
        if let Some(port) = target.port() {
            origin.push(':');
            origin.push_str(&port.to_string());
        }
        self.url_template
            .replace("{origin}", &origin)
            .replace("{host}", host)
    }
}

fn default_true() -> bool {
    true
}

fn default_max_browsers() -> usize {
    4
}

fn default_max_pages_per_context() -> usize {
    4
}

fn default_session_ttl_secs() -> u64 {
    crate::identity::DEFAULT_SESSION_TTL_SECS
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputConfig {
    pub html_dir: Option<String>,
    pub graph_path: Option<String>,
    pub metadata_path: Option<String>,
    pub screenshot_dir: Option<String>,
    pub screenshot: bool,
    /// Capture mode string: `viewport`, `fullpage`, or `element:<selector>`.
    /// `None` or an unrecognised value falls back to `fullpage` (the legacy
    /// default). Parsed by `parse_screenshot_mode`.
    #[serde(default)]
    pub screenshot_mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub proxies: Vec<String>,
    pub proxy_file: Option<String>,
    pub strategy: RotationStrategy,
    pub sticky_per_host: bool,
    pub health_check_interval: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueueBackend {
    InMemory,
    Sqlite { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageBackend {
    Memory,
    Sqlite { path: String },
    Filesystem { root: String },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderSessionScope {
    #[default]
    RegistrableDomain,
    Host,
    Origin,
    Url,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeMode {
    Avoidance,
    #[default]
    SolverReady,
}

/// Granular toggles for the infrastructure-intel orchestrator (Fase B+).
/// Every flag defaults to ON so `crawlex intel <target>` produces a full
/// recon report by default; operators strip stages down when they
/// already have data or hit rate limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraIntelConfig {
    /// Enumerate subdomains via crt.sh + CertSpotter + HackerTarget +
    /// passive DNS aggregators. Produces rows in `domains`.
    pub subdomains: bool,
    /// Full DNS record set per domain: A/AAAA/CNAME/MX/TXT/NS/SOA/CAA
    /// + wildcard-detection probe. Produces rows in `dns_records`.
    pub dns: bool,
    /// WHOIS / RDAP for domain + parent TLD. Produces rows in
    /// `whois_records`.
    pub whois: bool,
    /// TLS handshake + X.509 deep parse (SAN, wildcard, sig algo,
    /// pubkey class, self-signed, validity window). Produces rows in
    /// `certs` + `cert_seen_on`.
    pub cert: bool,
    /// Certificate Transparency log query (crt.sh JSON API). Pulls
    /// ALL historical certs issued for target + its subdomains.
    pub ct_logs: bool,
    /// HTTP server fingerprint: ServerType / Framework / WAF / CDN /
    /// CloudProvider enums derived from headers/cookies/error pages.
    pub server_fp: bool,
    /// Reverse IP lookups (RDAP IP for ASN + HackerTarget reverseip)
    /// and cloud/CDN IP-range classification. Produces updates on
    /// `ip_addresses`.
    pub reverse_ip: bool,
    /// Fase D: active network probes — ICMP ping, traceroute, port
    /// scan. Requires CAP_NET_RAW/root for raw sockets; falls back to
    /// TCP-connect when unprivileged. Off by default because it is
    /// detectable.
    pub network_probe: bool,
}

impl Default for InfraIntelConfig {
    fn default() -> Self {
        Self {
            subdomains: true,
            dns: true,
            whois: true,
            cert: true,
            ct_logs: true,
            server_fp: true,
            reverse_ip: true,
            network_probe: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_concurrent_render: 0,
            max_concurrent_http: 500,
            max_depth: Some(5),
            same_host_only: false,
            include_subdomains: true,
            target_domain: None,
            infra_intel: InfraIntelConfig::default(),
            identity_preset: None,
            respect_robots_txt: true,
            user_agent_profile: Profile::Chrome131Stable,
            chrome_path: None,
            chrome_flags: Vec::new(),
            block_resources: Vec::new(),
            wait_strategy: WaitStrategy::NetworkIdle { idle_ms: 500 },
            rate_per_host_rps: None,
            retry_max: 3,
            retry_backoff: Duration::from_millis(500),
            queue_backend: QueueBackend::InMemory,
            storage_backend: StorageBackend::Memory,
            output: OutputConfig::default(),
            proxy: ProxyConfig::default(),
            locale: None,
            timezone: None,
            metrics_prometheus_port: None,
            hook_scripts: Vec::new(),
            discovery_filter_regex: None,
            follow_pages_only: true,
            crtsh_enabled: false,
            robots_paths_enabled: true,
            well_known_enabled: true,
            pwa_enabled: true,
            wayback_enabled: false,
            favicon_enabled: true,
            dns_enabled: false,
            collect_net_timings: false,
            collect_web_vitals: false,
            collect_peer_cert: false,
            rdap_enabled: false,
            cookies_enabled: true,
            render_session_scope: RenderSessionScope::RegistrableDomain,
            follow_redirects: true,
            max_redirects: 10,
            #[cfg(feature = "cdp-backend")]
            actions: None,
            #[cfg(feature = "cdp-backend")]
            script_spec: None,
            profile_autodetect: true,
            user_agent_override: None,
            auto_fetch_chromium: true,
            action_policy: crate::policy::ActionPolicy::permissive(),
            challenge_mode: ChallengeMode::SolverReady,
            collect_runtime_routes: true,
            collect_network_endpoints: true,
            collect_indexeddb: false,
            collect_cache_storage: false,
            collect_manifest: true,
            collect_service_workers: true,
            max_browsers: default_max_browsers(),
            max_pages_per_context: default_max_pages_per_context(),
            render_budgets: crate::scheduler::BudgetLimits::default(),
            session_ttl_secs: default_session_ttl_secs(),
            drop_session_on_block: true,
            session_scope_auto: false,
            #[cfg(feature = "cdp-backend")]
            motion_profile: crate::render::motion::MotionProfile::default(),
            warmup: WarmupPolicy::default(),
            reading_dwell: None,
            http_limits: HttpLimits::default(),
            content_store: ContentStoreConfig::default(),
        }
    }
}

impl Config {
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    inner: Config,
}

impl ConfigBuilder {
    pub fn max_concurrent_render(mut self, n: usize) -> Self {
        self.inner.max_concurrent_render = n;
        self
    }
    pub fn max_concurrent_http(mut self, n: usize) -> Self {
        self.inner.max_concurrent_http = n;
        self
    }
    pub fn respect_robots_txt(mut self, v: bool) -> Self {
        self.inner.respect_robots_txt = v;
        self
    }
    pub fn user_agent_profile(mut self, p: Profile) -> Self {
        self.inner.user_agent_profile = p;
        self
    }
    pub fn wait_strategy(mut self, w: WaitStrategy) -> Self {
        self.inner.wait_strategy = w;
        self
    }
    pub fn queue(mut self, q: QueueBackend) -> Self {
        self.inner.queue_backend = q;
        self
    }
    pub fn storage(mut self, s: StorageBackend) -> Self {
        self.inner.storage_backend = s;
        self
    }
    pub fn proxy(mut self, p: ProxyConfig) -> Self {
        self.inner.proxy = p;
        self
    }
    pub fn build(self) -> crate::Result<Config> {
        Ok(self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).expect("test url parses")
    }

    #[test]
    fn warmup_default_is_disabled() {
        // The whole point of warmup is that it's opt-in: enabling it
        // silently would surprise operators with extra requests per target.
        let p = WarmupPolicy::default();
        assert!(!p.enabled);
        assert_eq!(p.url_template, "{origin}");
        assert_eq!(p.dwell_ms, 1500);
    }

    #[test]
    fn config_default_warmup_is_disabled() {
        let c = Config::default();
        assert!(!c.warmup.enabled);
    }

    #[test]
    fn render_template_empty_yields_empty_string() {
        // Empty template is the caller's "skip warmup" sentinel; we don't
        // substitute anything, so the result stays empty.
        let p = WarmupPolicy {
            enabled: true,
            url_template: String::new(),
            dwell_ms: 0,
        };
        assert_eq!(p.render_template(&url("https://example.com/foo")), "");
    }

    #[test]
    fn render_template_origin_placeholder() {
        let p = WarmupPolicy {
            enabled: true,
            url_template: "{origin}".to_string(),
            dwell_ms: 0,
        };
        // Default port elided — we want "https://example.com", not
        // "https://example.com:443", to match what the target actually uses.
        assert_eq!(
            p.render_template(&url("https://example.com/foo/bar")),
            "https://example.com"
        );
    }

    #[test]
    fn render_template_origin_preserves_non_default_port() {
        let p = WarmupPolicy {
            enabled: true,
            url_template: "{origin}".to_string(),
            dwell_ms: 0,
        };
        // Non-default port is load-bearing for cookie scoping — the warmup
        // and target need to agree on it.
        assert_eq!(
            p.render_template(&url("https://example.com:8443/x")),
            "https://example.com:8443"
        );
    }

    #[test]
    fn render_template_host_placeholder() {
        let p = WarmupPolicy {
            enabled: true,
            url_template: "{host}".to_string(),
            dwell_ms: 0,
        };
        assert_eq!(
            p.render_template(&url("https://sub.example.com/foo")),
            "sub.example.com"
        );
    }

    #[test]
    fn render_template_mixed_placeholders() {
        let p = WarmupPolicy {
            enabled: true,
            url_template: "{origin}/search?q={host}".to_string(),
            dwell_ms: 0,
        };
        assert_eq!(
            p.render_template(&url("https://example.com/x")),
            "https://example.com/search?q=example.com"
        );
    }

    #[test]
    fn render_template_literal_without_placeholders_passes_through() {
        // Operators can hard-code a third-party warmup (e.g. google.com)
        // if they want Referer-style cover — no placeholder substitution
        // required for that case.
        let p = WarmupPolicy {
            enabled: true,
            url_template: "https://www.google.com/".to_string(),
            dwell_ms: 0,
        };
        assert_eq!(
            p.render_template(&url("https://example.com/foo")),
            "https://www.google.com/"
        );
    }
}
