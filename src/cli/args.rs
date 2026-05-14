use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "crawlex",
    version,
    about = "Stealth crawler with Chrome-perfect fingerprint"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level command tree — resource-first, verb last.
///
/// Grammar: `crawlex <resource> <verb> [<name>] [flags]`.
///
/// Reading-order: noun first, then the action you want to do on it.
/// An operator spells out which thing they're touching before the
/// action, which lets `--help` on the noun list all applicable
/// verbs in one screen.
///
/// Examples:
///   crawlex pages       run       --seed https://example.com/
///   crawlex crawl       resume
///   crawlex fingerprint run       www.stone.com.br --include-subdomains
///   crawlex fingerprint show      stone.com.br     --limit 30
///   crawlex fingerprint export    stone.com.br     --out stone.json --pretty
///   crawlex graph       export    --storage-path crawlex.db --out graph.json
///   crawlex queue       stats     --queue-path queue.sqlite
///   crawlex queue       purge     --queue-path queue.sqlite
///   crawlex queue       export    --queue-path queue.sqlite --out q.json
///   crawlex sessions    list      --storage-path crawlex.db
///   crawlex session     drop      --id abcd --storage-path crawlex.db
///   crawlex telemetry   show      --db crawlex.db
///   crawlex stealth     test
///   crawlex stealth     inspect
#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(subcommand)]
    Pages(PagesVerb),
    #[command(subcommand)]
    Crawl(CrawlVerb),
    #[command(subcommand)]
    Fingerprint(FingerprintVerb),
    #[command(subcommand)]
    Graph(GraphVerb),
    #[command(subcommand)]
    Queue(QueueVerb),
    #[command(subcommand)]
    Sessions(SessionsVerb),
    #[command(subcommand)]
    Session(SessionVerb),
    #[command(subcommand)]
    Telemetry(TelemetryVerb),
    #[command(subcommand)]
    Stealth(StealthVerb),
    /// v2 scraping framework — `Spider` runtime entry point. Slice 19
    /// wires the `--replay-dir` / `--replay-db` cache flags. The
    /// `spider run` command is a thin stub until the engine bindings
    /// land (slice 25); flag parsing is the contract under test.
    #[command(subcommand)]
    Spider(SpiderVerb),
    /// Fetch EasyList (or another adblock domain source) and merge the
    /// extracted domain rules into the override file consulted by the
    /// adblock gate at runtime. Slice 20.
    UpdateBlocklist(UpdateBlocklistArgs),
    /// Convert a curl invocation (typically copied from Chrome devtools)
    /// into an equivalent crawlex config (TOML/JSON) or Node SDK snippet.
    /// Slice 21.
    FromCurl(FromCurlArgs),
    /// Interactive REPL for ad-hoc scraping. Drops into a prompt where
    /// `.fetch <url>`, `.css <sel>`, `.xpath <expr>`, `.findByText <text>`,
    /// `.findByRegex <pattern>`, `.save <id>`, `.open`, `.help`, `.exit`
    /// drive the session. State persists across commands within the
    /// session; readline history persists across sessions. Slice 22.
    Shell(ShellArgs),
    /// JSON-RPC 2.0 MCP server over stdio. Exposes tools for HTTP fetch
    /// (`get`, `bulk_get`), dynamic/stealth fetch (`fetch`, `stealth_fetch`),
    /// session lifecycle (`open_session`, `close_session`, `list_sessions`)
    /// and DOM extraction (`css_query`, `xpath_query`). Slice 24.
    Mcp(McpArgs),
}

#[derive(Args, Debug, Clone)]
pub struct McpArgs {
    /// Greeting/handshake server name advertised in the MCP `initialize`
    /// response. Default `crawlex`.
    #[arg(long, default_value = "crawlex")]
    pub name: String,
}

#[derive(Args, Debug, Clone)]
pub struct ShellArgs {
    /// Use the stealth (full ImpersonateClient + persona) backend on
    /// every `.fetch`. Off by default — the bare HTTP backend is faster
    /// for one-off probes.
    #[arg(long, default_value_t = false)]
    pub stealth: bool,
    /// Override the readline history file. Defaults to
    /// `$XDG_DATA_HOME/crawlex/shell_history` (or
    /// `~/.local/share/crawlex/shell_history`).
    #[arg(long)]
    pub history_file: Option<String>,
    /// Directory used to persist adaptive fingerprints written by
    /// `.save <identifier>`. Defaults to `./.crawlex`.
    #[arg(long, default_value = "./.crawlex")]
    pub adaptive_dir: String,
    /// Spider id used as the adaptive-store key (one file per spider).
    /// Defaults to `shell`.
    #[arg(long, default_value = "shell")]
    pub spider_id: String,
}

#[derive(Args, Debug, Clone)]
pub struct FromCurlArgs {
    /// The full curl command, quoted. Example:
    /// `crawlex from-curl "curl 'https://e.com' -H 'x: 1'"`.
    pub command: String,
    /// Output shape — `toml` (default), `json`, or `node` (Node SDK
    /// snippet importing `crawlex`).
    #[arg(long, default_value = "toml")]
    pub format: String,
}

#[derive(Args, Debug, Clone)]
pub struct UpdateBlocklistArgs {
    /// Source URL. Defaults to the canonical EasyList feed.
    #[arg(long, default_value = "https://easylist.to/easylist/easylist.txt")]
    pub url: String,
    /// Where to write the merged override file. Defaults to the
    /// platform user-config path
    /// (`$XDG_CONFIG_HOME/crawlex/blocklist.txt` on Linux).
    #[arg(long)]
    pub out: Option<String>,
    /// Read the source from a local file instead of fetching it. Useful
    /// for offline / CI / test scenarios.
    #[arg(long)]
    pub from_file: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum SpiderVerb {
    /// Run a spider end-to-end. Optional replay flags short-circuit the
    /// network for development iteration — first run records, subsequent
    /// runs replay from cache.
    Run(SpiderRunArgs),
}

#[derive(Args, Debug, Clone)]
pub struct SpiderRunArgs {
    /// Recipe identifier — looked up in the registry built by
    /// `defineSpider` / the Rust trait `Spider`. Slice 19 only parses
    /// the value; resolution lands in slice 25.
    pub spider: String,
    /// Record-on-first-hit, replay-on-subsequent cache rooted at this
    /// directory. Mutually exclusive with `--replay-db`.
    #[arg(long, conflicts_with = "replay_db")]
    pub replay_dir: Option<String>,
    /// Use the reddb-style per-spider store under `--replay-data-dir`
    /// (default `./.crawlex`) as the replay cache. Mutually exclusive
    /// with `--replay-dir`.
    #[arg(long, default_value_t = false)]
    pub replay_db: bool,
    /// Directory holding the reddb-style per-spider replay file. Only
    /// consulted when `--replay-db` is set.
    #[arg(long, default_value = "./.crawlex")]
    pub replay_data_dir: String,
}

#[derive(Subcommand, Debug)]
pub enum PagesVerb {
    /// Start a new page crawl from the given seeds.
    Run(CrawlArgs),
    /// List persisted pages, optionally filtered by canonical status.
    List(PagesListArgs),
}

#[derive(Args, Debug)]
pub struct PagesListArgs {
    #[arg(long)]
    pub storage_path: String,
    /// Canonical per-URL status filter — one of `queued`, `completed`,
    /// `disallowed`, `skipped`, `errored`, `cancelled`. Omit for all.
    #[arg(long)]
    pub status: Option<String>,
    /// Row cap. 0 = unlimited (single batch, no `next_cursor`).
    #[arg(long, default_value_t = 0)]
    pub limit: usize,
    /// Opaque cursor token from a prior `pages list` response's
    /// `next_cursor`. Mutually consistent with `--status` — replaying
    /// a cursor under a different filter is rejected.
    #[arg(long)]
    pub cursor: Option<String>,
    /// Output JSON (always JSON today; accepted for SDK `runJson`
    /// compatibility — the flag is a no-op).
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum CrawlVerb {
    /// Resume a paused / interrupted crawl from its persisted queue.
    Resume(ResumeArgs),
}

#[derive(Subcommand, Debug)]
pub enum FingerprintVerb {
    /// Run the passive+active recon stages against a target.
    Run(IntelArgs),
    /// Read-only summary of persisted intel for a target.
    Show(IntelShowArgs),
    /// Dump every intel table for a target as JSON.
    Export(IntelExportArgs),
}

#[derive(Subcommand, Debug)]
pub enum GraphVerb {
    /// Export the discovery edges (JSON / DOT, picked by extension).
    Export(ExportGraphArgs),
}

#[derive(Subcommand, Debug)]
pub enum QueueVerb {
    /// Size + by-state counters.
    Stats(QueueStatsArgs),
    /// Delete every row.
    Purge(QueuePurgeArgs),
    /// Dump rows as JSON.
    Export(QueueExportArgs),
}

#[derive(Subcommand, Debug)]
pub enum SessionsVerb {
    /// Enumerate archived sessions.
    List(SessionsListArgs),
}

#[derive(Subcommand, Debug)]
pub enum SessionVerb {
    /// Evict a session by id.
    Drop(SessionDropArgs),
}

#[derive(Subcommand, Debug)]
pub enum TelemetryVerb {
    /// Antibot challenge-rate aggregation tables (SQLite views).
    Show(TelemetryShowArgs),
}

#[derive(Subcommand, Debug)]
pub enum StealthVerb {
    /// Verify ALPN/cipher/JA4 against the built-in expectations.
    Test,
    /// Print the active IdentityBundle fingerprint summary.
    Inspect(InspectArgs),
    /// Browse the TLS fingerprint catalog (vendored + captured + mined).
    #[command(subcommand)]
    Catalog(CatalogVerb),
}

#[derive(Subcommand, Debug)]
pub enum CatalogVerb {
    /// List every fingerprint registered in the catalog.
    /// Filter by browser via `--filter chrome` / `firefox` / `chromium` / `edge` / `safari`.
    List(CatalogListArgs),
    /// Show the full fingerprint for a single profile by curl-impersonate
    /// name (e.g. `chrome_116.0.5845.180_win10`) or by `<browser>-<major>-<os>`
    /// (e.g. `chrome-149-linux`).
    Show(CatalogShowArgs),
}

#[derive(Args, Debug)]
pub struct CatalogListArgs {
    /// Restrict to one browser family (`chrome`, `chromium`, `firefox`,
    /// `edge`, `safari`). Omit to list all.
    #[arg(long)]
    pub filter: Option<String>,
    /// Output as JSON (compact one-line per profile) instead of the
    /// default human-readable table.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct CatalogShowArgs {
    /// Profile identifier — either the catalog name
    /// (`chrome_116.0.5845.180_win10`) or a `<browser>-<major>-<os>`
    /// spec that resolves via era fallback (`chrome-149-linux`).
    pub profile: String,
    /// Output as JSON instead of the default human-readable layout.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct TelemetryShowArgs {
    #[arg(long)]
    pub db: String,
    #[arg(long, default_value_t = 20)]
    pub top: usize,
}

#[derive(Args, Debug)]
pub struct QueueStatsArgs {
    #[arg(long)]
    pub queue_path: String,
}

#[derive(Args, Debug)]
pub struct QueuePurgeArgs {
    #[arg(long)]
    pub queue_path: String,
}

#[derive(Args, Debug)]
pub struct QueueExportArgs {
    #[arg(long)]
    pub queue_path: String,
    #[arg(long)]
    pub out: String,
}

#[derive(Args, Debug)]
pub struct SessionsListArgs {
    #[arg(long)]
    pub storage_path: String,
    #[arg(long)]
    pub state: Option<String>,
}

#[derive(Args, Debug)]
pub struct SessionDropArgs {
    #[arg(long)]
    pub storage_path: String,
    #[arg(long)]
    pub id: String,
}

#[derive(Args, Debug, Clone)]
pub struct IntelExportArgs {
    /// Registrable domain whose intel was previously collected.
    pub target: String,
    #[arg(long, default_value = "./crawlex.db")]
    pub db: String,
    /// Write JSON to this file. Empty/omitted ⇒ stdout.
    #[arg(long)]
    pub out: Option<String>,
    /// Write a single-file HTML dashboard to this path instead of JSON.
    /// Takes precedence over `--out` when both are supplied.
    #[arg(long)]
    pub html: Option<String>,
    /// Pretty-print (2-space indent). Off ⇒ compact one-line JSON.
    #[arg(long, default_value_t = false)]
    pub pretty: bool,
}

#[derive(Args, Debug, Clone)]
pub struct IntelShowArgs {
    /// Registrable domain whose intel was previously collected.
    pub target: String,
    /// SQLite database path.
    #[arg(long, default_value = "./crawlex.db")]
    pub db: String,
    /// Cap on how many rows each list-section prints. Zero disables
    /// truncation for that section.
    #[arg(long, default_value_t = 30)]
    pub limit: usize,
}

#[derive(Args, Debug, Clone)]
pub struct IntelArgs {
    /// Registrable domain to investigate (e.g. `stone.com.br`).
    pub target: String,
    /// SQLite database path — re-uses the same schema the crawl
    /// subcommand writes into, so an intel run + crawl run populate
    /// one coherent store.
    #[arg(long, default_value = "./crawlex.db")]
    pub db: String,
    /// Skip the subdomain enumeration step.
    #[arg(long)]
    pub no_subdomains: bool,
    /// Skip DNS record collection.
    #[arg(long)]
    pub no_dns: bool,
    /// Skip WHOIS/RDAP.
    #[arg(long)]
    pub no_whois: bool,
    /// Skip the TLS handshake / certificate grab.
    #[arg(long)]
    pub no_cert: bool,
    /// Opt-in to active TCP-connect port probes (top ~20 ports) +
    /// reverse DNS + cloud/CDN IP-range tagging per unique IP. Even
    /// though it runs without CAP_NET_RAW, the 3-way handshake still
    /// shows up in the target's access logs — default OFF.
    #[arg(long)]
    pub network_probe: bool,
}

#[derive(Args, Debug, Clone)]
pub struct CrawlArgs {
    #[arg(long, action = clap::ArgAction::Append)]
    pub seed: Vec<String>,
    #[arg(long)]
    pub seeds_file: Option<String>,

    /// Default fetch method: "spoof" (HTTP), "render" (Chrome), "auto"
    #[arg(long, default_value = "spoof")]
    pub method: String,

    /// Operator-level render-path switch:
    /// `auto` (default) keeps today's behaviour — impersonate first,
    /// escalate to render via the policy engine when needed.
    /// `always` skips impersonation and forces every seeded job onto
    /// the render path. `never` pins every job to the impersonate path
    /// and refuses any render escalation, so the render pool is never
    /// instantiated. Wins over `--method` when both are set.
    #[arg(long = "render-mode")]
    pub render_mode: Option<String>,

    #[arg(long)]
    pub max_concurrent_render: Option<usize>,
    #[arg(long)]
    pub max_concurrent_http: Option<usize>,
    #[arg(long)]
    pub max_depth: Option<u32>,
    #[arg(long, default_value_t = false)]
    pub same_host_only: bool,
    #[arg(long, default_value_t = true)]
    pub include_subdomains: bool,
    #[arg(long)]
    pub respect_robots_txt: Option<bool>,

    #[arg(long)]
    pub wait_strategy: Option<String>,
    #[arg(long)]
    pub wait_idle_ms: Option<u64>,

    /// Per-CDP-command timeout in milliseconds (default 30000). Bumps the
    /// deadline applied to every CDP request, including `Page.navigate` —
    /// heavy real-world targets (Cloudflare-fronted SPAs with WordPress +
    /// ad scripts) regularly exceed 30s before lifecycle settles. Mirrors
    /// the `CRAWLEX_REQUEST_TIMEOUT_MS` env var; flag wins when both set.
    #[arg(long = "render-request-timeout-ms")]
    pub render_request_timeout_ms: Option<u64>,

    /// Lifecycle event the navigation watcher waits for. `load` (default)
    /// blocks until window onload fires; `domcontentloaded` returns as
    /// soon as the parser is done — much faster on heavy pages whose
    /// `load` never settles within the request timeout. Mirrors the
    /// `CRAWLEX_NAVIGATION_LIFECYCLE` env var; flag wins when both set.
    #[arg(long = "navigation-lifecycle")]
    pub navigation_lifecycle: Option<String>,

    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long)]
    pub chrome_path: Option<String>,
    #[arg(long = "chrome-flag", action = clap::ArgAction::Append)]
    pub chrome_flag: Vec<String>,
    /// Connect to an existing Chrome/Chromium CDP endpoint instead of
    /// launching a local browser, e.g. `http://127.0.0.1:9222`.
    #[arg(long)]
    pub external_cdp_url: Option<String>,
    /// GPU posture for managed Chrome: `compat` keeps `--disable-gpu`,
    /// `stealth` keeps GPU surfaces enabled where Chrome can support them.
    #[arg(long)]
    pub gpu_policy: Option<String>,
    /// Flatten open shadow roots into serialized HTML before extraction.
    #[arg(long, default_value_t = false)]
    pub flatten_shadow_dom: bool,
    /// Remove fixed/sticky modal overlays before serializing rendered HTML.
    #[arg(long, default_value_t = false)]
    pub remove_overlays: bool,
    /// Remove common consent/cookie banners before serializing rendered HTML.
    #[arg(long, default_value_t = false)]
    pub remove_consent_popups: bool,
    /// Enable a last-resort fetch adapter command. The command receives JSON
    /// on stdin and returns JSON on stdout.
    #[arg(long)]
    pub fallback_fetch_command: Option<String>,
    /// Additional argument for `--fallback-fetch-command`. Repeatable.
    #[arg(long = "fallback-fetch-arg", action = clap::ArgAction::Append)]
    pub fallback_fetch_arg: Vec<String>,
    #[arg(long)]
    pub fallback_fetch_timeout_ms: Option<u64>,
    #[arg(long)]
    pub fallback_fetch_max_bytes: Option<u64>,
    /// Validate existing cached pages and skip full processing when fresh.
    #[arg(long, default_value_t = false)]
    pub cache_validate: bool,
    /// Accept cache rows younger than this many seconds without a network
    /// validation probe.
    #[arg(long)]
    pub cache_max_age_secs: Option<u64>,
    /// Skip pages whose stored `Last-Modified` is at-or-before this Unix
    /// timestamp (seconds since epoch). Pre-network freshness check.
    #[arg(long)]
    pub modified_since: Option<u64>,
    /// Discovery-only mode: extract/enqueue links while skipping heavy page
    /// persistence and analysis.
    #[arg(long, default_value_t = false)]
    pub prefetch: bool,
    /// Score newly discovered URLs and process higher-value links first.
    #[arg(long, default_value_t = false)]
    pub best_first: bool,
    /// Keyword bonus for `--best-first`. Repeatable.
    #[arg(long = "score-keyword", action = clap::ArgAction::Append)]
    pub score_keyword: Vec<String>,
    #[arg(long)]
    pub block_resource: Option<String>,
    /// Typed CDP-level reject category. Repeatable. Accepted values:
    /// `image`, `media`, `font`, `stylesheet`. Auto-disabled when
    /// `--screenshot` is set so visual fidelity is preserved.
    #[arg(long = "reject-resource-type", action = clap::ArgAction::Append)]
    pub reject_resource_type: Vec<String>,

    /// Declared crawl purpose, honored against the `Content-Signal:`
    /// directive in each host's robots.txt. Repeatable. Accepted values:
    /// `search`, `ai-input`, `ai-train`. Comma-separated shorthand also
    /// accepted. Empty (default) means all three.
    #[arg(long = "crawl-purpose", action = clap::ArgAction::Append)]
    pub crawl_purpose: Vec<String>,

    #[arg(long)]
    pub queue: Option<String>,
    #[arg(long)]
    pub queue_path: Option<String>,
    #[arg(long)]
    pub queue_redis_url: Option<String>,

    #[arg(long)]
    pub storage: Option<String>,
    #[arg(long)]
    pub storage_path: Option<String>,

    #[arg(long)]
    pub output_html_dir: Option<String>,
    #[arg(long)]
    pub output_graph: Option<String>,
    #[arg(long)]
    pub output_metadata: Option<String>,
    #[arg(long, default_value_t = false)]
    pub screenshot: bool,
    #[arg(long)]
    pub screenshot_dir: Option<String>,
    /// Screenshot capture mode: `viewport`, `fullpage` (default), or
    /// `element:<css>`. The capture runs *after* wait-strategy + actions +
    /// Lua hooks have mutated the DOM, so SPA post-click views are the
    /// surface being grabbed. Element mode falls back to None (no file) if
    /// the selector can't be resolved.
    #[arg(long)]
    pub screenshot_mode: Option<String>,

    /// DNS-over-HTTPS provider. One of `off` (default; use system
    /// resolver), `cloudflare`, `google`, `quad9`, or a custom
    /// `https://…/dns-query` URL. Default OFF so existing behaviour is
    /// preserved; operators opt in when they want the crawl's DNS
    /// queries off the ISP resolver. See `src/impersonate/doh.rs` for
    /// the current wiring status (config-only today).
    #[arg(long, default_value = "off")]
    pub doh: String,

    #[arg(long = "proxy", action = clap::ArgAction::Append)]
    pub proxy: Vec<String>,
    #[arg(long)]
    pub proxy_file: Option<String>,
    #[arg(long)]
    pub proxy_strategy: Option<String>,
    #[arg(long, default_value_t = false)]
    pub proxy_sticky_per_host: bool,
    #[arg(long)]
    pub proxy_health_check_interval_secs: Option<u64>,
    /// Launch a local explicit HTTP proxy backed by Raffel and use it as the
    /// crawler's sole proxy.
    #[arg(long, default_value_t = false)]
    pub raffel_proxy: bool,
    /// Path to the local Raffel checkout/build. Defaults to the workspace used
    /// during development on this machine.
    #[arg(long, default_value = "/home/cyber/Work/tetis/libs/raffel")]
    pub raffel_proxy_path: String,
    /// Host/interface for the local Raffel proxy listener.
    #[arg(long, default_value = "127.0.0.1")]
    pub raffel_proxy_host: String,
    /// Listen port for the local Raffel proxy.
    #[arg(long, default_value_t = 8899)]
    pub raffel_proxy_port: u16,

    #[arg(long = "hook-script", action = clap::ArgAction::Append)]
    pub hook_script: Vec<String>,

    /// Drive lifecycle hooks via the JS bridge protocol. Format:
    /// `stdio` (multiplex on stdin+stdout — bridge envelopes
    /// disambiguated from NDJSON events by their `kind` field) or
    /// `fd:N` for an explicit file-descriptor pair (`N` reads, `N+1`
    /// writes). Spawn convention is owned by the SDK — see
    /// `sdk/crawlex-sdk.js::crawl({hooks})`. Off by default.
    #[arg(long = "hook-bridge")]
    pub hook_bridge: Option<String>,

    #[arg(long)]
    pub on_discovery_filter_regex: Option<String>,

    /// Pick a persona from `identity::profiles::catalog()` (0-indexed).
    /// When set, overrides the historical Linux/Intel default and wires
    /// `IdentityBundle::from_persona(catalog()[N], …)` into the render
    /// pool. Prefer `--persona <name>` for legibility — this remains for
    /// existing scripts that pass numeric indices.
    #[arg(long)]
    pub identity_preset: Option<u8>,

    /// Pick a persona by codename (`tux`, `office`, `gamer`, `atlas`,
    /// `pixel`). Resolves to the same row as `--identity-preset N` but
    /// tracks the row even if catalog ordering shifts.
    /// `tux` = Linux Intel desktop, `office` = Win10 Intel laptop,
    /// `gamer` = Win10 NVIDIA desktop, `atlas` = macOS Apple M1,
    /// `pixel` = Android mobile (Adreno). Run `crawlex stealth catalog list`
    /// to see all rows. Mutually exclusive with `--identity-preset`.
    #[arg(long, conflicts_with = "identity_preset")]
    pub persona: Option<String>,

    /// Only follow URLs classified as page/document/api; other assets are
    /// stored but not enqueued. Set --follow-all-assets to disable.
    #[arg(long, default_value_t = false)]
    pub follow_all_assets: bool,

    /// Enable certificate-transparency subdomain seeding (crt.sh).
    #[arg(long, default_value_t = false)]
    pub crtsh: bool,

    /// Skip robots.txt Disallow/Allow path expansion (enabled by default).
    #[arg(long, default_value_t = false)]
    pub no_robots_paths: bool,

    /// Skip .well-known/* probes (enabled by default).
    #[arg(long, default_value_t = false)]
    pub no_well_known: bool,

    /// Skip PWA manifest / service worker probes (enabled by default).
    #[arg(long, default_value_t = false)]
    pub no_pwa: bool,

    /// Skip favicon mmh3 fingerprinting (enabled by default).
    #[arg(long, default_value_t = false)]
    pub no_favicon: bool,

    /// Enable Wayback Machine (CDX) URL seeding.
    #[arg(long, default_value_t = false)]
    pub wayback: bool,

    /// Enable DNS record enumeration and seed related hosts.
    #[arg(long, default_value_t = false)]
    pub dns: bool,

    /// Opt-in: collect both network timings and Web Vitals (overrides the
    /// granular flags below). OFF by default — speed first.
    #[arg(long, default_value_t = false)]
    pub metrics: bool,

    /// Opt-in: measure DNS/TCP/TLS/TTFB/download on HTTP path and store.
    #[arg(long, default_value_t = false)]
    pub metrics_net: bool,

    /// Opt-in: execute Web Vitals JS after render and store.
    #[arg(long, default_value_t = false)]
    pub metrics_vitals: bool,

    /// Opt-in: extract peer TLS cert (CN, SANs, fingerprint) and seed SANs.
    #[arg(long, default_value_t = false)]
    pub peer_cert: bool,

    /// Opt-in: RDAP lookup per registrable domain (registrar, expires, NS).
    #[arg(long, default_value_t = false)]
    pub rdap: bool,

    /// Disable cookie persistence across requests (default: enabled).
    #[arg(long, default_value_t = false)]
    pub no_cookies: bool,

    /// Browser session reuse boundary for render jobs:
    /// registrable_domain|host|origin|url.
    #[arg(long)]
    pub render_session_scope: Option<String>,

    /// Disable 3xx redirect following (default: enabled).
    #[arg(long, default_value_t = false)]
    pub no_follow_redirects: bool,

    /// Disable auto-download of a pinned Chromium-for-Testing when no system
    /// Chrome is found (default: enabled). Only meaningful with the
    /// `chromium-fetcher` feature compiled in.
    #[arg(long, default_value_t = false)]
    pub no_fetch_chromium: bool,

    /// Max redirects to follow (default 10).
    #[arg(long)]
    pub max_redirects: Option<u8>,

    /// Path to a JSON file with an Actions script executed on every rendered
    /// page (see src/render/actions.rs for schema). Enables form fill, click,
    /// scroll, type with human-like timing.
    #[arg(long)]
    pub actions_file: Option<String>,

    /// Path to a ScriptSpec v1 JSON file (see `crate::script::spec`).
    /// When set, each rendered page runs the declarative script instead
    /// of the legacy `--actions-file` recipe — mutually exclusive with
    /// `--actions-file`. ScriptSpec is the recommended replacement for
    /// multi-step interactive crawls (click, type, wait_for, screenshot,
    /// snapshot, extract, assert, export).
    #[arg(long, value_name = "PATH", conflicts_with = "actions_file")]
    pub script_spec: Option<String>,

    #[arg(long)]
    pub rate_per_host_rps: Option<f64>,
    #[arg(long)]
    pub retry_max: Option<u32>,
    #[arg(long)]
    pub retry_backoff_ms: Option<u64>,

    #[arg(long)]
    pub user_agent_override: Option<String>,
    #[arg(long)]
    pub timezone: Option<String>,
    #[arg(long)]
    pub locale: Option<String>,

    #[arg(long)]
    pub metrics_prometheus_port: Option<u16>,
    #[arg(long, default_value = "info")]
    pub log_level: String,
    #[arg(long, default_value = "text")]
    pub log_format: String,

    // ----- v0.2 contract flags ---------------------------------------
    /// Emit lifecycle events on stdout. `ndjson` writes one JSON object per
    /// line, `none` keeps stdout silent (default).
    #[arg(long, default_value = "none")]
    pub emit: String,

    /// Policy preset that shapes every decide-here-or-there call.
    /// `fast` minimises render escalation; `balanced` is the default;
    /// `deep` prefers render when uncertain; `forensics` collects full
    /// artifacts on every job.
    #[arg(long = "policy", default_value = "balanced")]
    pub policy: String,

    /// Load a `Config` JSON from `<path>` or stdin (`-`). When set,
    /// individual CLI flags still override fields the config sets.
    /// Schema mirrors `crawlex::config::Config`.
    #[arg(long)]
    pub config: Option<String>,

    /// Mirror every `decision.made` event to stderr in human-readable
    /// form. The NDJSON stream on stdout is unaffected.
    #[arg(long, default_value_t = false)]
    pub explain: bool,

    /// Disable the SPA JS observer (History API + fetch + XHR wrappers).
    /// Default: observer is active and runtime_routes/network_endpoints
    /// artifacts are emitted + pushed to the frontier.
    #[arg(long, default_value_t = false)]
    pub no_spa_observer: bool,

    /// Enable the IndexedDB inventory collector (opt-in — heavy on
    /// data-rich apps). Emits `snapshot.indexeddb` artifacts.
    #[arg(long, default_value_t = false)]
    pub collect_indexeddb: bool,

    /// Enable the Cache Storage inventory collector (opt-in — heavy
    /// on SW-backed apps). Emits `snapshot.cache_storage` artifacts.
    #[arg(long, default_value_t = false)]
    pub collect_cache_storage: bool,

    /// Turn on ALL SPA/PWA state collectors at once: runtime routes,
    /// network endpoints, IndexedDB, Cache Storage, manifest JSON and
    /// service workers. Convenient umbrella for `--policy forensics`
    /// style crawls. Individual `--collect-*` / `--no-spa-observer`
    /// flags still apply when set explicitly.
    #[arg(long, default_value_t = false)]
    pub collect_spa_state: bool,

    /// Per-verb policy applied to every action step (click/type/eval/...).
    /// Accepts `permissive` (default, all allowed), `strict` (deny all),
    /// `default` (conservative: eval=deny, download=confirm, rest=allow),
    /// or a path to a JSON policy file. Use when running a ScriptSpec
    /// from an untrusted source (LLM-generated, shared fixture).
    #[arg(long)]
    pub action_policy: Option<String>,

    /// Challenge handling mode: `avoidance` keeps captcha handling strictly
    /// prevention-only; `solver-ready` records extra widget metadata so a
    /// solver can be integrated later without changing the capture contract.
    #[arg(long)]
    pub challenge_mode: Option<String>,

    /// Vendor-specific bypass tier. `none` (default) disables every
    /// trick; `replay` enables conservative cookie pinning for cookies
    /// the crawler's own sessions earned (Akamai `_abck`, DataDome,
    /// PerimeterX `_px*`); `aggressive` additionally allows best-effort
    /// Turnstile invisible-widget dummy attempts. Opt-in only.
    #[arg(long, value_name = "LEVEL")]
    pub antibot_bypass: Option<String>,

    // ----- Phase 5: throughput / budgets ---------------------------
    /// Max Chrome instances kept alive simultaneously. Each proxy key
    /// gets its own Chrome; LRU eviction fires when the cap is hit.
    #[arg(long)]
    pub max_browsers: Option<usize>,

    /// Max idle + in-flight pages per BrowserContext. Higher = more
    /// parallel tabs per session; lower = better memory reuse.
    #[arg(long)]
    pub max_pages_per_context: Option<usize>,

    /// Slice 7 — overall wall-clock budget for this crawl, in seconds.
    /// When the watchdog fires the run is cancelled with terminal
    /// reason `cancelled_due_to_timeout`. Default unset = no watchdog.
    #[arg(long)]
    pub job_max_runtime_secs: Option<u64>,

    /// Slice 7 — TTL (seconds) applied to written pages + the run's
    /// `crawl_stats` row. Rows past the deadline are GC'd by the
    /// reaper. Default unset = no retention/reaper.
    #[arg(long)]
    pub result_retention_secs: Option<u64>,

    /// Slice 7 — hard cap on successfully crawled pages per run.
    /// Hitting the cap terminates the run with reason
    /// `cancelled_due_to_limits`. Default unset = unbounded.
    #[arg(long)]
    pub max_pages: Option<u64>,

    /// Max concurrent render jobs targeting a single host.
    #[arg(long)]
    pub max_per_host_inflight: Option<usize>,

    /// Max concurrent render jobs targeting a single origin.
    #[arg(long)]
    pub max_per_origin_inflight: Option<usize>,

    /// Max concurrent render jobs routed through a single proxy.
    #[arg(long)]
    pub max_per_proxy_inflight: Option<usize>,

    /// Max concurrent render jobs per stateful session. Default 1 so
    /// per-session cookies / SPA state don't interleave across tabs.
    #[arg(long)]
    pub max_per_session_inflight: Option<usize>,

    // ----- Phase 6: session isolation -------------------------------
    /// Render session time-to-live (seconds). Sessions not touched for
    /// this long are torn down (BrowserContext disposed, cookies
    /// dropped). Default 3600 (1h).
    #[arg(long)]
    pub session_ttl_secs: Option<u64>,

    /// When set, policy may automatically demote the
    /// `--render-session-scope` on login pages and hard antibot
    /// walls. Default off (scope stays what the operator declared).
    #[arg(long, default_value_t = false)]
    pub session_scope_auto: bool,

    /// Keep contaminated/blocked sessions around even when policy would
    /// otherwise drop them on first hit. Default: drop on block.
    #[arg(long, default_value_t = false)]
    pub keep_blocked_sessions: bool,

    /// Human motion engine preset:
    /// `fast` — linear path + minimal delay (throughput first);
    /// `balanced` (default) — WindMouse + Fitts + OU jitter;
    /// `human` — realistic cadence, overshoots, ~2–4s/click;
    /// `paranoid` — aggressive realism, 5–10s/click.
    #[arg(long)]
    pub motion_profile: Option<String>,

    /// Enable the post-settle "reading" dwell: after the wait strategy
    /// fires, sleep proportional to the rendered body's word count
    /// before we serialise the DOM. Trades throughput for stealth —
    /// reCAPTCHA v3 / DataDome score instant extraction as bot-like.
    #[arg(long, default_value_t = false)]
    pub reading_dwell: bool,

    /// Words-per-minute the "reader" simulates. 250 ≈ typical adult
    /// prose speed. Only consulted when `--reading-dwell` is set.
    #[arg(long, default_value_t = 250)]
    pub reading_dwell_wpm: u32,

    /// Gaussian jitter σ (ms) applied to the computed dwell, so
    /// successive requests aren't exactly identical. Only consulted
    /// when `--reading-dwell` is set.
    #[arg(long, default_value_t = 40)]
    pub reading_dwell_jitter_ms: u64,

    // ----- Wave 2 infra-scaffold wire-ups --------------------------
    /// Residential-proxy provider adapter (stub). One of
    /// `none` (default), `brightdata`, `oxylabs`, `iproyal`. All
    /// adapters are scaffold-only in this build — they return
    /// `AdapterNotConfigured` until real API credentials are wired.
    /// Provided here so operator config files + shell scripts can
    /// settle on the final flag name ahead of adapter rollout.
    #[arg(long, default_value = "none")]
    pub residential_provider: String,

    /// Captcha-solver adapter (stub). One of `none` (default),
    /// `2captcha`, `anticaptcha`, `vlm`. Crawlex policy stays
    /// prevention-first: every adapter refuses to answer unless the
    /// operator wires an API key via env vars documented in
    /// `docs/infra-tier-operator.md`.
    #[arg(long, default_value = "none")]
    pub captcha_solver: String,

    /// Mobile device profile for the Chromium backend. Accepts the
    /// aliases documented in `src/render/android_profile.rs` (e.g.
    /// `pixel-7-pro`, `pixel8`, `s23`, `android`). Default: desktop
    /// profile (no mobile emulation).
    #[arg(long)]
    pub mobile_profile: Option<String>,
}

#[derive(Args, Debug)]
pub struct ResumeArgs {
    #[arg(long)]
    pub queue_path: String,
}

#[derive(Args, Debug)]
pub struct InspectArgs {
    pub url: String,
    #[arg(long)]
    pub profile: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum QueueCmd {
    Stats {
        #[arg(long)]
        queue_path: String,
    },
    Purge {
        #[arg(long)]
        queue_path: String,
    },
    Export {
        #[arg(long)]
        queue_path: String,
        #[arg(long)]
        out: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionsCmd {
    /// List sessions persisted in the `sessions_archive` table.
    List {
        #[arg(long)]
        storage_path: String,
        /// Optional state filter: clean|warm|contaminated|blocked.
        #[arg(long)]
        state: Option<String>,
    },
    /// Archive (evict) a session by id. Requires the SQLite storage
    /// backend — the registry itself is in-process and can only be
    /// mutated by the running crawler.
    Drop {
        #[arg(long)]
        storage_path: String,
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TelemetryCmd {
    /// Print aggregated challenge-rate dashboards (by vendor / proxy /
    /// session). Reads the `v_challenge_rate_*` views created by the
    /// storage layer on startup. Verb under the `challenge` resource
    /// so the full invocation stays `telemetry challenge show`.
    Show {
        /// Path to the crawlex SQLite storage (same as `--storage-path`).
        #[arg(long)]
        db: String,
        /// Cap rows for the session view (operator-first: keep terminals
        /// readable). Defaults to 20.
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
}

#[derive(Args, Debug)]
pub struct ExportGraphArgs {
    #[arg(long)]
    pub storage_path: String,
    #[arg(long)]
    pub out: String,
}
