// Type surface for the crawlex Node SDK.
//
// Wire format mirrors `src/events/envelope.rs::EventEnvelope` — every
// line emitted by `crawlex crawl --emit ndjson` parses into one of the
// `CrawlEvent` variants below. The union is keyed on the `event` field
// (NOT a synthetic `kind` discriminator) so destructuring a stream is a
// single `switch (ev.event) { ... }` away.
//
// `data` is typed where the rust emitter has a stable shape and left as
// `Record<string, unknown>` where the field is reserved for a future
// emit site (FetchCompleted / ExtractCompleted / ProxyScored / etc).

// ─── Envelope ──────────────────────────────────────────────────────────

/** Stable string identifiers for every event kind crawlex can emit. */
export type EventKind =
  | 'run.started'
  | 'run.completed'
  | 'session.created'
  | 'session.state_changed'
  | 'session.evicted'
  | 'job.started'
  | 'job.failed'
  | 'decision.made'
  | 'fetch.completed'
  | 'render.completed'
  | 'extract.completed'
  | 'artifact.saved'
  | 'proxy.scored'
  | 'robots.decision'
  | 'challenge.detected'
  | 'step.started'
  | 'step.completed'
  | 'vendor.telemetry_observed'
  | 'tech.fingerprint_detected';

/**
 * Canonical per-URL lifecycle status (slice 1). Mirrors
 * `crawlex::Status` on the Rust side. Written to the SQLite
 * `pages.crawl_status` column and shipped on `BaseEnvelope.status`.
 */
export type UrlStatus =
  | 'queued'
  | 'completed'
  | 'disallowed'
  | 'skipped'
  | 'errored'
  | 'cancelled';

/**
 * Canonical per-job terminal label (slice 1). Mirrors
 * `crawlex::TerminalReason`. Written to the SQLite
 * `crawl_stats.terminal_reason` column.
 */
export type TerminalReason =
  | 'completed'
  | 'errored'
  | 'cancelled_due_to_timeout'
  | 'cancelled_due_to_limits'
  | 'cancelled_by_user';

/** Outer envelope — every NDJSON line decodes into this shape. */
export interface BaseEnvelope<E extends EventKind = EventKind, D = unknown> {
  /**
   * Wire schema version. Currently `2` — bumped from `1` in slice 1
   * when the canonical `status` field was added. Older consumers that
   * only read `event`/`why`/`data` stay compatible.
   */
  v: 2;
  /** ISO-8601 UTC timestamp with millisecond precision. */
  ts: string;
  /** Discriminator. */
  event: E;
  /** Stable run id; present once `run.started` has been emitted. */
  run_id?: number;
  /** Active browser/identity session id, when applicable. */
  session_id?: string;
  /** Job target URL, when applicable. */
  url?: string;
  /**
   * Short structured reason (e.g. `proxy:bad-score`,
   * `render:js-challenge`, `retry:5xx`, `budget:exceeded`). Required on
   * `decision.made` / `job.failed`; optional elsewhere.
   */
  why?: string;
  /**
   * Canonical per-URL status (slice 1). Present on events that carry a
   * per-URL lifecycle transition; optional everywhere else.
   */
  status?: UrlStatus;
  /** Event-specific payload. Shape varies per `event`. */
  data: D;
}

// ─── pages list (SDK results endpoint) ─────────────────────────────────

/**
 * Row returned by `crawlex pages list --json` (slice 1). The SDK
 * results endpoint — call via [`runJson`] with `['pages', 'list',
 * '--storage-path', path, '--status', 'completed']` to pull persisted
 * rows filtered by canonical status.
 *
 * `crawl_status` is `null` on legacy rows written before the column
 * existed; new writes populate it.
 */
export interface PageStatusRow {
  url: string;
  final_url: string;
  /** Upstream HTTP status code (or `0` if the fetch never produced one). */
  http_status: number;
  crawl_status: UrlStatus | null;
}

/**
 * Opaque pagination token from the SDK results read path (slice 8).
 * Treated as an unstructured string by consumers — the encoding is a
 * URL-safe base64 of a versioned struct and may change across crawlex
 * releases. Pass it back verbatim on the next `pages list` call.
 */
export type PageCursor = string;

/**
 * Wire shape of `crawlex pages list --json` (slice 8). When `limit > 0`,
 * `next_cursor` is present iff additional rows match the filter. With
 * `limit = 0` the entire result set ships in `rows` and `next_cursor`
 * is absent.
 */
export interface PageListResponse {
  rows: PageStatusRow[];
  next_cursor?: PageCursor;
}

/** Options for [`paginatePages`]. */
export interface PaginatePagesOptions {
  storagePath: string;
  /** Optional canonical status filter (mirrors `--status`). */
  status?: UrlStatus;
  /** Rows per page. Defaults to `100`. Must be `> 0`. */
  pageSize?: number;
  /** Override the resolved binary path. */
  bin?: string;
  /** Extra env vars for the child process. */
  env?: Record<string, string>;
}

/**
 * Stream every persisted `pages` row matching `status`. Wraps
 * `crawlex pages list` with cursor pagination so callers iterate
 * without seeing the cursor token. Yields `PageStatusRow` values.
 */
export function paginatePages(
  opts: PaginatePagesOptions
): AsyncIterableIterator<PageStatusRow>;

// ─── Typed payloads ────────────────────────────────────────────────────

export interface RunStartedData {
  policy_profile: string;
  max_concurrent_http: number;
  max_concurrent_render: number;
}

export type RunCompletedData = Record<string, unknown>;

export interface SessionCreatedData {
  /** `"render"` for the CDP backend, `"http"` for the impersonate path. */
  engine: 'render' | 'http' | string;
  /** Session-scope discriminator, e.g. `"per-host:example.com"`. */
  scope: string;
}

export interface SessionStateChangedData {
  /** Previous session state (`"clean"`, `"contaminated"`, ...). */
  from: string;
  to: string;
  /** e.g. `"challenge:cloudflare_turnstile"`. */
  reason?: string;
}

export interface SessionEvictedData {
  /** `"ttl" | "block" | "manual" | "run_ended"` and friends. */
  reason: string;
  state: string;
  urls_visited: number;
  challenges_seen: number;
}

export interface JobStartedData {
  job_id: string;
  /** `"http" | "render"`. */
  method: string;
  depth: number;
  priority: number;
  attempts: number;
}

export interface JobFailedData {
  job_id: string;
  /** Coarse failure bucket (`"network"`, `"timeout"`, `"5xx"`, ...). */
  kind: string;
  error: string;
  attempts: number;
}

export interface DecisionMadeData {
  /** `"keep" | "drop" | "retry" | "skip"` and friends. */
  decision: string;
  reason?: string;
  error_kind?: string;
  job_id?: string;
  [k: string]: unknown;
}

/**
 * Mirrors `src/events/envelope.rs::FetchCompletedData`. Carries the
 * per-fetch network breakdown so a stream consumer can act on timings
 * without round-tripping through the SQLite `page_metrics` table.
 */
export interface FetchCompletedData {
  final_url: string;
  status: number;
  /** Which path served this URL: `"impersonate"` (HTTP spoof client) or
   * `"fallback"` (external fallback_fetch command). The render path
   * emits a separate `render.completed` event with `path: "render"`. */
  path?: 'impersonate' | 'fallback';
  bytes?: number;
  body_truncated: boolean;
  dns_ms?: number;
  tcp_connect_ms?: number;
  tls_handshake_ms?: number;
  ttfb_ms?: number;
  download_ms?: number;
  total_ms?: number;
  alpn?: string;
  tls_version?: string;
  cipher?: string;
}

/**
 * Compact subset of `metrics::WebVitals` shipped on `render.completed`.
 * All fields optional — bot-blocked or pre-load renders may have nothing
 * populated.
 */
export interface VitalsSummary {
  ttfb_ms?: number;
  dom_content_loaded_ms?: number;
  load_event_ms?: number;
  first_contentful_paint_ms?: number;
  largest_contentful_paint_ms?: number;
  cumulative_layout_shift?: number;
  total_blocking_time_ms?: number;
  dom_nodes?: number;
  js_heap_used_bytes?: number;
  resource_count?: number;
  total_transfer_bytes?: number;
}

export interface RenderCompletedData {
  final_url: string;
  status: number;
  /** Always `"render"` — the literal lets a stream consumer
   * disambiguate against `fetch.completed.data.path` without
   * inspecting the envelope's `event` field. */
  path?: 'render';
  manifest: boolean;
  service_workers: number;
  screenshot: boolean;
  resources: number;
  runtime_routes: number;
  network_endpoints: number;
  is_spa: boolean;
  artifacts: number;
  /** Core Web Vitals snapshot — present when the renderer collected them. */
  vitals: VitalsSummary;
}

/** Reserved — not yet emitted as of v1.0.0. */
export type ExtractCompletedData = Record<string, unknown>;

/**
 * Mirrors `src/events/envelope.rs::ArtifactSavedData`. Carries the
 * complete descriptor a consumer needs to locate / reuse a persisted
 * artifact.
 */
export interface ArtifactSavedData {
  /** e.g. `"screenshot.full_page"`, `"snapshot.html"`. */
  kind: string;
  mime: string;
  size: number;
  /** Hex-encoded SHA-256 of the artifact bytes. */
  sha256: string;
  name?: string;
  step_id?: string;
  step_kind?: string;
  selector?: string;
  final_url?: string;
  /**
   * Where the artifact landed:
   * - Filesystem backend: path relative to the storage root
   *   (e.g. `artifacts/<session>/<stem>.png`).
   * - SQLite backend: `cas:<sha256>` URI pointing at the
   *   content-addressed blob store (`<dbfile>.blobs/<shard>/<sha256>`).
   * - Memory backend / non-persisting sinks: omitted.
   */
  path?: string;
}

/** Reserved — not yet emitted as of v1.0.0. */
export type ProxyScoredData = Record<string, unknown>;

/** Reserved — not yet emitted as of v1.0.0. */
export type RobotsDecisionData = Record<string, unknown>;

export interface ChallengeDetectedData {
  /** `"cloudflare_turnstile"`, `"recaptcha"`, `"datadome"`, ... */
  vendor: string;
  /** `"suspected" | "challenge_page" | "widget_present" | "hard_block"`. */
  level: string;
  origin?: string;
  proxy?: string;
  [k: string]: unknown;
}

export interface StepStartedData {
  step_id: string;
  step_kind: string;
}

export interface StepCompletedData {
  step_id: string;
  step_kind: string;
  success: boolean;
  duration_ms: number;
  error?: string | null;
}

/** Reserved — not yet emitted as of v1.0.0. */
export type VendorTelemetryObservedData = Record<string, unknown>;

export interface TechFingerprintDetectedData {
  host: string;
  url: string;
  technologies: Array<{
    name: string;
    confidence?: number;
    [k: string]: unknown;
  }>;
  confidence_max?: number;
  [k: string]: unknown;
}

// ─── Discriminated union ───────────────────────────────────────────────

/**
 * Every NDJSON line emitted by `crawlex crawl --emit ndjson` parses
 * into one of these variants. `switch (ev.event)` narrows `ev.data` to
 * the matching payload type.
 */
export type CrawlEvent =
  | BaseEnvelope<'run.started', RunStartedData>
  | BaseEnvelope<'run.completed', RunCompletedData>
  | BaseEnvelope<'session.created', SessionCreatedData>
  | BaseEnvelope<'session.state_changed', SessionStateChangedData>
  | BaseEnvelope<'session.evicted', SessionEvictedData>
  | BaseEnvelope<'job.started', JobStartedData>
  | BaseEnvelope<'job.failed', JobFailedData>
  | BaseEnvelope<'decision.made', DecisionMadeData>
  | BaseEnvelope<'fetch.completed', FetchCompletedData>
  | BaseEnvelope<'render.completed', RenderCompletedData>
  | BaseEnvelope<'extract.completed', ExtractCompletedData>
  | BaseEnvelope<'artifact.saved', ArtifactSavedData>
  | BaseEnvelope<'proxy.scored', ProxyScoredData>
  | BaseEnvelope<'robots.decision', RobotsDecisionData>
  | BaseEnvelope<'challenge.detected', ChallengeDetectedData>
  | BaseEnvelope<'step.started', StepStartedData>
  | BaseEnvelope<'step.completed', StepCompletedData>
  | BaseEnvelope<'vendor.telemetry_observed', VendorTelemetryObservedData>
  | BaseEnvelope<'tech.fingerprint_detected', TechFingerprintDetectedData>;

/**
 * Fallback variant emitted by the SDK when a line fails to parse as
 * JSON — preserves the raw string so consumers can log/recover instead
 * of dropping it silently.
 */
export interface RawLine {
  kind: 'raw';
  line: string;
}

/** Union of every value yielded by the iterator. */
export type StreamEvent = CrawlEvent | RawLine;

// ─── crawl() options + handle ──────────────────────────────────────────

/**
 * Structured CLI args for `crawlex crawl`. camelCase keys map to the
 * kebab-case flags the binary parses (`maxDepth` → `--max-depth`,
 * `screenshotMode` → `--screenshot-mode`). Multi-value fields
 * (`seeds`, `proxies`, `hookScripts`, `chromeFlags`) repeat the flag
 * once per array element.
 *
 * Coverage is curated — flags not listed here can still be passed via
 * `CrawlOptions.rawArgs`. Boolean flags whose Rust default is `true`
 * (e.g. `--include-subdomains`) cannot be turned off from this object;
 * use `rawArgs: ['--include-subdomains=false']` if needed.
 */
export interface CrawlArgs {
  /** Seed URLs (repeated `--seed`). */
  seeds?: string[];
  /** Path to a newline-delimited file of seed URLs. */
  seedsFile?: string;

  /** Default fetch method. */
  method?: 'spoof' | 'render' | 'auto' | string;
  maxConcurrentHttp?: number;
  maxConcurrentRender?: number;
  maxDepth?: number;
  sameHostOnly?: boolean;
  /** Default `true`; cannot be unset from this object — use `rawArgs`. */
  includeSubdomains?: boolean;
  respectRobotsTxt?: boolean;

  // ─── Render / browser ────────────────────────────────────────────
  waitStrategy?: string;
  waitIdleMs?: number;
  renderRequestTimeoutMs?: number;
  navigationLifecycle?: 'load' | 'domcontentloaded' | string;
  /** Stealth profile name. See `crawlex stealth catalog list`. */
  profile?: string;
  /** Persona codename (`tux`, `office`, `gamer`, `atlas`, `pixel`). */
  persona?: 'tux' | 'office' | 'gamer' | 'atlas' | 'pixel' | string;
  /** Numeric persona index — mutually exclusive with `persona`. */
  identityPreset?: number;
  chromePath?: string;
  /** Extra `--chrome-flag X` repeated per element. */
  chromeFlags?: string[];
  blockResource?: string;

  // ─── Storage / queue ─────────────────────────────────────────────
  storage?: 'memory' | 'filesystem' | 'sqlite' | string;
  storagePath?: string;
  queue?: 'memory' | 'sqlite' | 'redis' | string;
  queuePath?: string;
  queueRedisUrl?: string;

  // ─── Output ──────────────────────────────────────────────────────
  outputHtmlDir?: string;
  outputGraph?: string;
  outputMetadata?: string;
  /** Toggle screenshot capture. */
  screenshot?: boolean;
  screenshotDir?: string;
  /** `viewport` (default), `fullpage`, or `element:<css>`. */
  screenshotMode?: 'viewport' | 'fullpage' | string;

  // ─── Network ─────────────────────────────────────────────────────
  doh?: 'off' | 'cloudflare' | 'google' | 'quad9' | string;
  /** Proxy URLs (repeated `--proxy`). */
  proxies?: string[];
  proxyFile?: string;
  proxyStrategy?: string;
  proxyStickyPerHost?: boolean;
  proxyHealthCheckIntervalSecs?: number;
  raffelProxy?: boolean;
  raffelProxyHost?: string;
  raffelProxyPort?: number;

  // ─── Hooks / discovery ───────────────────────────────────────────
  hookScripts?: string[];
  onDiscoveryFilterRegex?: string;
  followAllAssets?: boolean;
  crtsh?: boolean;
  noRobotsPaths?: boolean;

  // Allow forward-compat fields without breaking strict mode. Anything
  // that doesn't match the typed surface above will still serialize as
  // `--<kebab-case-key> <value>` (or the appropriate flag form).
  [key: string]: string | number | boolean | string[] | number[] | undefined;
}

export interface CrawlOptions {
  /**
   * Seed URLs to enqueue. Shorthand for `args.seeds`; forwarded as
   * repeated `--seed` flags.
   */
  seeds?: string[];
  /**
   * Full crawlex config — serialized to JSON and piped on stdin.
   * **Mutually exclusive with `hooks`** (the bridge protocol uses
   * stdin as the reply channel). Pass config flags via `args` instead
   * when wiring hooks.
   */
  config?: Record<string, unknown>;
  /**
   * Structured CLI args. Auto-converted to flags via camelCase →
   * kebab-case mapping. See [`CrawlArgs`].
   */
  args?: CrawlArgs;
  /**
   * Raw CLI flag passthrough for advanced or future flags not yet
   * typed in `CrawlArgs`. Appended verbatim after the structured
   * args.
   */
  rawArgs?: string[];
  /**
   * JS hook spec built via [`defineHooks`]. When set, the SDK adds
   * `--hook-bridge stdio` to the child invocation and routes inbound
   * bridge envelopes through the dispatcher. The crawler yields
   * NDJSON events for the consumer; hook traffic is consumed
   * internally and never surfaces on the iterator.
   */
  hooks?: HookSpec;
  /** Override the resolved binary path. */
  bin?: string;
  /** Cancel the run. SIGTERM is sent to the child. */
  signal?: AbortSignal;
  /** Extra env vars for the child process. */
  env?: Record<string, string>;
}

export interface CrawlHandle extends AsyncIterable<StreamEvent> {
  /** Underlying child process. */
  child: import('node:child_process').ChildProcess;
  /** Resolves when the process exits cleanly. */
  done: Promise<{ code: number | null; signal: NodeJS.Signals | null }>;
}

// ─── ensureInstalled() ─────────────────────────────────────────────────

export interface EnsureInstalledOptions {
  targetDir?: string;
  version?: string;
  channel?: 'stable' | 'next' | string;
  verify?: boolean;
  skipIfFresh?: boolean;
  repo?: string;
  githubToken?: string;
}

export interface EnsureInstalledResult {
  binaryPath: string;
  changed: boolean;
  source: string;
  channel?: string;
}

// ─── runJson() ─────────────────────────────────────────────────────────

export interface RunJsonOptions {
  bin?: string;
  env?: Record<string, string>;
  input?: string | Buffer;
}

// ─── exports ───────────────────────────────────────────────────────────

/** Stream crawl events as NDJSON. */
export function crawl(opts?: CrawlOptions): CrawlHandle;

/** Convenience: run a crawl to completion and collect all events. */
export function crawlAll(opts?: CrawlOptions): Promise<StreamEvent[]>;

/** Run any subcommand with `--json` and return the parsed result. */
export function runJson<T = unknown>(args: string[], opts?: RunJsonOptions): T;

// ─── Hooks (JS bridge) ─────────────────────────────────────────────────

/**
 * Wire-shape `HookContext` shipped on each `hook_invoke` envelope. The
 * subset is intentionally narrower than the rust `HookContext` —
 * request/response bodies and binary headers are dropped. Hooks that
 * need raw bytes should run as a Rust-native or Lua hook in-process.
 */
export interface HookCtx {
  url: string;
  depth: number;
  response_status?: number;
  response_headers?: Record<string, string>;
  html_present: boolean;
  body_size?: number;
  captured_urls: string[];
  proxy?: string;
  retry_count: number;
  allow_retry: boolean;
  robots_allowed?: boolean;
  user_data: Record<string, unknown>;
  error?: string;
}

/** Patch the SDK can apply back onto the live HookContext. */
export interface HookPatch {
  capturedUrls?: string[];
  userData?: Record<string, unknown>;
  robotsAllowed?: boolean;
  allowRetry?: boolean;
}

export type HookDecision = 'continue' | 'skip' | 'retry' | 'abort';

/** Either a bare decision string or a structured result with patch. */
export type HookResult = HookDecision | { decision: HookDecision; patch?: HookPatch };

/** Async or sync handler signature used by `defineHooks`. */
export type HookHandler = (ctx: HookCtx) => HookResult | Promise<HookResult>;

/** Per-event handler map accepted by `defineHooks`. */
export interface HookHandlers {
  onBeforeEachRequest?: HookHandler;
  onAfterDnsResolve?: HookHandler;
  onAfterTlsHandshake?: HookHandler;
  onAfterFirstByte?: HookHandler;
  onResponseBody?: HookHandler;
  onAfterLoad?: HookHandler;
  onAfterIdle?: HookHandler;
  onDiscovery?: HookHandler;
  onJobStart?: HookHandler;
  onJobEnd?: HookHandler;
  onError?: HookHandler;
  onRobotsDecision?: HookHandler;
}

/**
 * Output of `defineHooks`. The bridge driver inside `crawl()` (when
 * wired in a future SDK release) reads `subscribed` to send the
 * `subscribe` envelope and routes every inbound `hook_invoke` through
 * `dispatch`. Exposed here so library consumers can plug into custom
 * IPC fabrics (`fork()`, worker threads, websockets) the same way.
 */
export interface HookSpec {
  subscribed: string[];
  dispatch(envelope: { kind: string; [k: string]: unknown }): Promise<{
    kind: 'hook_result';
    id: number;
    decision: HookDecision;
    patch?: Record<string, unknown>;
  } | null>;
}

/** Build a hook spec from a typed handler map. */
export function defineHooks(handlers: HookHandlers): HookSpec;

/** Stable wire names of every supported event (snake_case). */
export const HOOK_EVENTS: readonly string[];

/** Download (and verify) the native binary into the package cache. */
export function ensureInstalled(opts?: EnsureInstalledOptions): Promise<EnsureInstalledResult>;

/** Resolved path where the SDK would invoke the binary. */
export function binaryPath(): string;

/** Asset filename expected on the GitHub release for the current platform. */
export function assetBaseName(): string;

/** npm package version (matches the native binary version). */
export const version: string;

// ─── Selector engine (slices 9 & 11) ──────────────────────────────────
//
// Forward-declared types for the parser/selector surface. The runtime
// binding lands in a future SDK release (same deferral pattern as the
// streaming `paginate` helper above). Until then the type names are
// exported so application code can be written against the planned API.

/** CSS / XPath flavour for [`ElementHandle.generateSelector`]. */
export type SelectorKind = 'css' | 'xpath';

/**
 * Handle to an element inside a parsed tree. Mirrors the rust
 * `ElementHandle` surface — `css` / `xpath` queries scoped to this
 * subtree, attribute / text access, navigation, and auto-selector
 * generation (slice 11).
 */
export interface ElementHandle {
  readonly tag: string;
  attr(name: string): string | undefined;
  text(): string;
  html(): string;
  innerHtml(): string;
  parent(): ElementHandle | undefined;
  children(): ElementHandle[];
  siblings(): ElementHandle[];
  css(selector: string): ElementHandle[];
  xpath(expr: string): ElementHandle[];
  /**
   * Produce a selector string that uniquely identifies this element in
   * its source tree. Prefers stable anchors (`id`, `data-testid`,
   * ARIA attributes, semantic tags) over positional fallbacks
   * (`:nth-of-type` for CSS, `[N]` for XPath).
   */
  generateSelector(opts: { kind: SelectorKind }): string;
  /**
   * Find other elements in the same tree whose similarity score against
   * this element meets `threshold` (default 0.2). The anchor itself is
   * excluded. Results are sorted by descending score, so callers can
   * `.slice(0, n)` for top-N matches. Pure in-tree scan — does not
   * touch the adaptive store.
   */
  findSimilar(opts?: { threshold?: number }): ElementHandle[];
}

/** Engine backend a session is bound to. */
export type BackendKind = 'http' | 'render' | 'stealth';

/** Options accepted by the [`Request`] constructor. */
export interface RequestOptions {
  /** HTTP method. Defaults to `GET`. */
  method?: string;
  /**
   * Optional session id. When supplied, the request runs against the
   * backend + cookie jar registered for that id via `SessionManager`.
   * Unknown ids log a warning and fall back to the default backend.
   */
  sessionId?: string;
}

/**
 * Recipe-facing request descriptor. Slice 16 plumbs `sessionId` so a
 * recipe can pin successive fetches to an isolated engine state.
 */
export declare class Request {
  constructor(url: string, opts?: RequestOptions);
  readonly url: string;
  readonly method: string;
  readonly sessionId?: string;
}

// ─── Spider DSL (slice 17) ────────────────────────────────────────────

/** Response shape passed into `parse` by the JS spider runner. */
export interface SpiderResponse {
  request: { url: string; method: string; sessionId?: string };
  finalUrl: string;
  status: number;
  headers: Record<string, string | string[] | undefined>;
  body: Buffer;
}

/** What `parse` yields. Items are arbitrary plain objects; new requests
 *  must be `Request` instances so the runner can dedupe by method+URL. */
export type ParseYield = Request | Record<string, unknown> | null | undefined;

export interface SpiderSpec {
  /** Seed URLs. Required, must be non-empty. */
  startUrls: string[];
  /** Sync/async generator (or any iterable) that yields items and `Request`s. */
  parse: (resp: SpiderResponse) =>
    | Iterable<ParseYield>
    | AsyncIterable<ParseYield>
    | ParseYield;
  /** Per-host minimum gap between fetches, in milliseconds. */
  downloadDelayMs?: number;
  /** Honour `Disallow` + `Crawl-delay` against `opts.robotsCache`. */
  robotsTxtObey?: boolean;
  /** UA string used in robots evaluation and the default fetcher. */
  userAgent?: string;
  /** Stop after N items emitted. */
  maxItems?: number;
}

export interface SpiderDef extends Required<Omit<SpiderSpec, 'maxItems' | 'userAgent' | 'downloadDelayMs' | 'robotsTxtObey'>> {
  startUrls: string[];
  parse: SpiderSpec['parse'];
  downloadDelayMs: number;
  robotsTxtObey: boolean;
  userAgent: string;
  maxItems: number | null;
}

/** Persistable runner state. Field naming mirrors the Rust `Checkpoint`
 *  struct (snake_case) so a JS-paused run can be resumed in Rust. */
export interface SpiderCheckpoint {
  pending: Array<{ url: string; method: string; session_id?: string }>;
  seen: string[];
  items_emitted: number;
}

export interface RunSpiderOptions {
  /** Override the default node:http(s) fetcher. */
  fetcher?: (req: { url: string; method: string; sessionId?: string; userAgent?: string }) =>
    Promise<SpiderResponse>;
  /** Map<host, robots.txt body>. Required when `robotsTxtObey` is true. */
  robotsCache?: Map<string, string>;
  /** Abort the run (Ctrl-C). Returned handle's `checkpoint()` captures
   *  the frontier at the pause point. */
  signal?: AbortSignal;
  /** Resume from a previously captured [`SpiderCheckpoint`]. */
  resume?: SpiderCheckpoint;
}

/** Async iterable of items yielded by `parse`. Call `checkpoint()` after
 *  the loop terminates to snapshot the frontier for a resumable pause. */
export interface SpiderHandle extends AsyncIterableIterator<Record<string, unknown>> {
  checkpoint(): SpiderCheckpoint;
  isPaused(): boolean;
}

/** Validate + freeze a spider spec. */
export function defineSpider(spec: SpiderSpec): SpiderDef;

/** Drive a defined spider. */
export function runSpider(spider: SpiderDef, opts?: RunSpiderOptions): SpiderHandle;
