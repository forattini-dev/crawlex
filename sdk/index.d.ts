// Type surface for the crawlex Node SDK.

export interface CrawlOptions {
  /** Seed URLs to enqueue. */
  seeds?: string[];
  /** Full crawlex config — serialized to JSON and piped on stdin. */
  config?: Record<string, unknown>;
  /** Additional raw CLI arguments appended after `crawl --emit ndjson`. */
  args?: string[];
  /** Override the resolved binary path. */
  bin?: string;
  /** Cancel the run. Sends SIGTERM to the child. */
  signal?: AbortSignal;
  /** Extra env vars for the child process. */
  env?: Record<string, string>;
}

/** Discriminated union of NDJSON events emitted by `crawlex crawl --emit ndjson`. */
export type CrawlEvent =
  | { kind: 'page'; url: string; status: number; depth: number; bytes: number; content_type?: string; [k: string]: unknown }
  | { kind: 'link'; from: string; to: string; rel?: string; [k: string]: unknown }
  | { kind: 'discovered'; source: string; url: string; [k: string]: unknown }
  | { kind: 'error'; url?: string; message: string; [k: string]: unknown }
  | { kind: 'metric'; name: string; value: number; [k: string]: unknown }
  | { kind: 'robots'; url: string; allowed: boolean; [k: string]: unknown }
  | { kind: 'done'; total_pages: number; duration_ms: number; [k: string]: unknown }
  | { kind: 'raw'; line: string }
  | { kind: string; [k: string]: unknown };

export interface CrawlHandle extends AsyncIterable<CrawlEvent> {
  /** Underlying child process. */
  child: import('node:child_process').ChildProcess;
  /** Resolves when the process exits cleanly. */
  done: Promise<{ code: number | null; signal: NodeJS.Signals | null }>;
}

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

export interface RunJsonOptions {
  bin?: string;
  env?: Record<string, string>;
  input?: string | Buffer;
}

/** Stream crawl events as NDJSON. */
export function crawl(opts?: CrawlOptions): CrawlHandle;

/** Convenience: run a crawl to completion and collect all events. */
export function crawlAll(opts?: CrawlOptions): Promise<CrawlEvent[]>;

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
