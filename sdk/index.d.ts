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

/** Outer envelope — every NDJSON line decodes into this shape. */
export interface BaseEnvelope<E extends EventKind = EventKind, D = unknown> {
  /** Wire schema version. Currently `1`. */
  v: 1;
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
  /** Event-specific payload. Shape varies per `event`. */
  data: D;
}

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
  /** Full crawlex config — serialized to JSON and piped on stdin. */
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

/** Download (and verify) the native binary into the package cache. */
export function ensureInstalled(opts?: EnsureInstalledOptions): Promise<EnsureInstalledResult>;

/** Resolved path where the SDK would invoke the binary. */
export function binaryPath(): string;

/** Asset filename expected on the GitHub release for the current platform. */
export function assetBaseName(): string;

/** npm package version (matches the native binary version). */
export const version: string;
