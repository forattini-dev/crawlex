#!/usr/bin/env node
'use strict';

// crawlex — Node SDK + CLI wrapper.
//
// Two roles:
//   1. Library: import { crawl, run, ensureInstalled } from 'crawlex'
//   2. Bin:     `crawlex <args>` — transparently delegates to native binary
//
// Binary resolution order:
//   1. CRAWLEX_FORCE_BINARY env
//   2. <pkgRoot>/.crawlex/bin/crawlex[.exe]        (postinstall target)
//   3. PATH lookup                                  (if user installed via `cargo install`)
//
// Stream contract:
//   * The native binary writes one JSON envelope per line to stdout
//     (`{ v, ts, event, run_id?, session_id?, url?, why?, data }`).
//   * `crawl()` parses each line and yields it through an async iterator.
//   * Lines that fail to parse as JSON yield `{ kind: 'raw', line }` so
//     consumers can log/recover instead of dropping bytes silently.
//   * TypeScript consumers should narrow with `'event' in ev` (real
//     event) vs `'kind' in ev` (`raw` fallback). See `index.d.ts` for
//     the discriminated union over the 19 event kinds.

const fs = require('node:fs');
const fsp = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawn, spawnSync, execFileSync } = require('node:child_process');
const readline = require('node:readline');
const { Readable } = require('node:stream');
const https = require('node:https');
const { pipeline } = require('node:stream/promises');
const zlib = require('node:zlib');

const PKG_ROOT = path.resolve(__dirname, '..');
// Static version baked at publish time by `scripts/sync-version.js`.
// Avoids `require('../package.json')` so library consumers bundling
// the SDK (webpack/rollup/esbuild) don't need to mark package.json as
// an external resource or pin a filesystem read.
const SDK_VERSION = require('./version.js');
const BIN_DIR = path.join(PKG_ROOT, '.crawlex', 'bin');
const BIN_NAME = process.platform === 'win32' ? 'crawlex.exe' : 'crawlex';
const DEFAULT_REPO = process.env.CRAWLEX_REPO || 'forattini-dev/crawlex';

// ---------- asset naming ----------

function assetBaseName() {
  const plat = process.platform;
  const arch = process.arch;
  if (plat === 'linux' && arch === 'x64') return 'crawlex-linux-x86_64';
  if (plat === 'linux' && arch === 'arm64') return 'crawlex-linux-aarch64';
  if (plat === 'darwin' && arch === 'x64') return 'crawlex-macos-x86_64';
  if (plat === 'darwin' && arch === 'arm64') return 'crawlex-macos-aarch64';
  if (plat === 'win32' && arch === 'x64') return 'crawlex-windows-x86_64.exe';
  throw new Error(`unsupported platform: ${plat}/${arch}`);
}

// ---------- binary resolution ----------

function binaryPath() {
  if (process.env.CRAWLEX_FORCE_BINARY) return process.env.CRAWLEX_FORCE_BINARY;
  const local = path.join(BIN_DIR, BIN_NAME);
  if (fs.existsSync(local)) return local;
  // PATH fallback.
  const which = spawnSync(process.platform === 'win32' ? 'where' : 'which', ['crawlex'], {
    encoding: 'utf8',
  });
  if (which.status === 0) {
    const first = which.stdout.split('\n').map((s) => s.trim()).filter(Boolean)[0];
    if (first) return first;
  }
  return local; // Will ENOENT at spawn time; caller handles.
}

// ---------- download / install ----------

function get(url, { headers = {}, followRedirects = 5 } = {}) {
  return new Promise((resolve, reject) => {
    const req = https.get(url, { headers: { 'user-agent': `crawlex-sdk/${SDK_VERSION}`, ...headers } }, (res) => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode) && res.headers.location && followRedirects > 0) {
        res.resume();
        resolve(get(res.headers.location, { headers, followRedirects: followRedirects - 1 }));
        return;
      }
      if (res.statusCode !== 200) {
        res.resume();
        reject(new Error(`GET ${url} -> ${res.statusCode}`));
        return;
      }
      resolve(res);
    });
    req.on('error', reject);
  });
}

async function fetchText(url, opts) {
  const res = await get(url, opts);
  const chunks = [];
  for await (const c of res) chunks.push(c);
  return Buffer.concat(chunks).toString('utf8');
}

async function downloadTo(url, dest, opts) {
  await fsp.mkdir(path.dirname(dest), { recursive: true });
  const res = await get(url, opts);
  await pipeline(res, fs.createWriteStream(dest, { mode: 0o755 }));
}

async function sha256File(file) {
  const h = crypto.createHash('sha256');
  await pipeline(fs.createReadStream(file), h);
  return h.digest('hex');
}

async function ensureInstalled(options = {}) {
  const {
    targetDir = BIN_DIR,
    version = process.env.CRAWLEX_POSTINSTALL_VERSION || SDK_VERSION,
    channel = process.env.CRAWLEX_POSTINSTALL_CHANNEL || 'stable',
    verify = true,
    skipIfFresh = true,
    repo = DEFAULT_REPO,
    githubToken = process.env.GITHUB_TOKEN,
  } = options;

  if (process.env.CRAWLEX_FORCE_BINARY) {
    return { binaryPath: process.env.CRAWLEX_FORCE_BINARY, changed: false, source: 'env' };
  }

  const dest = path.join(targetDir, BIN_NAME);
  if (skipIfFresh && fs.existsSync(dest)) {
    return { binaryPath: dest, changed: false, source: 'cache' };
  }

  const tag = `v${version}`;
  const asset = assetBaseName();
  const base = `https://github.com/${repo}/releases/download/${tag}`;
  const headers = githubToken ? { authorization: `Bearer ${githubToken}` } : {};

  await fsp.mkdir(targetDir, { recursive: true });
  await downloadTo(`${base}/${asset}`, dest, { headers });
  fs.chmodSync(dest, 0o755);

  if (verify) {
    const shaUrl = `${base}/${asset}.sha256`;
    const shaText = await fetchText(shaUrl, { headers });
    const expected = shaText.trim().split(/\s+/)[0];
    const actual = await sha256File(dest);
    if (expected.toLowerCase() !== actual.toLowerCase()) {
      await fsp.rm(dest, { force: true });
      throw new Error(`sha256 mismatch for ${asset}: expected ${expected}, got ${actual}`);
    }
  }

  return { binaryPath: dest, changed: true, source: `release:${tag}`, channel };
}

// ---------- NDJSON streaming ----------

function parseLine(line) {
  try {
    return JSON.parse(line);
  } catch (_) {
    return { kind: 'raw', line };
  }
}

// camelCase → kebab-case. Stable mapping so consumers don't have to
// remember which Rust flag name corresponds to which JS field.
function kebab(name) {
  return name.replace(/[A-Z0-9]/g, (m, idx) => (idx === 0 ? m.toLowerCase() : `-${m.toLowerCase()}`));
}

// Multi-value flags emit `--flag VALUE` once per array element
// (clap `ArgAction::Append`). The Rust side names these in singular —
// `--seed`, `--proxy`, `--hook-script`, `--chrome-flag` — even though
// the SDK exposes them as plural arrays.
const MULTI_VALUE_FLAGS = {
  seeds: 'seed',
  proxies: 'proxy',
  hookScripts: 'hook-script',
  chromeFlags: 'chrome-flag',
};

// Top-level keys that are SDK-level concerns, not CLI flags. Keep the
// serializer from accidentally turning them into `--bin` or `--signal`.
const RESERVED_KEYS = new Set(['bin', 'signal', 'env', 'config', 'rawArgs']);

/**
 * Convert a structured `CrawlArgs` object into the array of CLI flags
 * the binary expects. Booleans → flag presence; strings / numbers →
 * `--flag VALUE`; arrays → repeated `--flag VALUE`. `null`/`undefined`
 * are dropped silently. Anything in `rawArgs` is appended verbatim.
 *
 * Boolean flags with a Rust `default_value_t = true` (e.g.
 * `--include-subdomains`) cannot be turned off from this serializer —
 * pass the raw form via `rawArgs: ['--include-subdomains=false']` if
 * you need to override.
 */
function serializeArgs(args = {}) {
  const out = [];
  for (const [key, value] of Object.entries(args)) {
    if (value === undefined || value === null) continue;
    if (RESERVED_KEYS.has(key)) continue;

    if (key in MULTI_VALUE_FLAGS) {
      if (!Array.isArray(value)) {
        throw new TypeError(`crawlex: '${key}' must be an array`);
      }
      const flag = `--${MULTI_VALUE_FLAGS[key]}`;
      for (const v of value) out.push(flag, String(v));
      continue;
    }

    const flag = `--${kebab(key)}`;
    if (typeof value === 'boolean') {
      if (value) out.push(flag);
      continue;
    }
    if (Array.isArray(value)) {
      // Default behaviour for un-mapped arrays: repeat the flag. Caller
      // can always reach for `rawArgs` if they need a comma-joined form.
      for (const v of value) out.push(flag, String(v));
      continue;
    }
    out.push(flag, String(value));
  }
  return out;
}

/**
 * Spawn `crawlex crawl ...` with `--emit ndjson` and yield events.
 *
 * @param {object} opts
 * @param {string[]} [opts.seeds]         URLs to seed the frontier (also accepted under `args.seeds`).
 * @param {object}   [opts.config]        Full config object; passed on stdin as JSON.
 * @param {object}   [opts.args]          Structured CLI args; see `CrawlArgs` in `index.d.ts`.
 *                                        Auto-converted to flags (camelCase → kebab-case).
 * @param {string[]} [opts.rawArgs]       Raw CLI flag passthrough for advanced/un-typed flags.
 * @param {string}   [opts.bin]           Override binary path.
 * @param {AbortSignal} [opts.signal]     Abort/cancel the run.
 * @param {object}   [opts.env]           Extra env vars for the child.
 * @returns {AsyncIterable<object>}
 */
function crawl(opts = {}) {
  const bin = opts.bin || binaryPath();
  const args = ['crawl', '--emit', 'ndjson'];
  // `--config -` reads from stdin. Mutually exclusive with `hooks`,
  // which uses stdin as the bridge reply channel.
  if (opts.config && !opts.hooks) args.push('--config', '-');
  if (opts.hooks) args.push('--hook-bridge', 'stdio');
  // top-level `seeds` is shorthand for `args.seeds`. Both spellings
  // forward into the same `--seed` repetition.
  const merged = { ...(opts.args || {}) };
  if (opts.seeds && !merged.seeds) merged.seeds = opts.seeds;
  args.push(...serializeArgs(merged));
  if (opts.rawArgs) args.push(...opts.rawArgs);

  const child = spawn(bin, args, {
    stdio: ['pipe', 'pipe', 'inherit'],
    env: { ...process.env, ...(opts.env || {}) },
  });

  if (opts.signal) {
    opts.signal.addEventListener('abort', () => child.kill('SIGTERM'), { once: true });
  }

  if (opts.config && !opts.hooks) {
    child.stdin.write(JSON.stringify(opts.config));
    child.stdin.end();
  } else if (!opts.hooks) {
    child.stdin.end();
  }
  // When hooks are wired we keep stdin open for the lifetime of the
  // crawl so the bridge can write `subscribe` + `hook_result` lines.

  const rl = readline.createInterface({ input: child.stdout, crlfDelay: Infinity });

  // Subscribe announcement is sent as soon as we see the rust `hello`
  // envelope. Done lazily so the test pump in `define-hooks.test.js`
  // continues to work without a real binary.
  let helloAcked = false;
  const writeBridge = (msg) => {
    try {
      child.stdin.write(JSON.stringify(msg) + '\n');
    } catch (e) {
      // Stdin already closed (child exited) — swallow; the iterator
      // will surface the exit code on `done`.
    }
  };

  const exited = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => {
      if (code === 0 || signal === 'SIGTERM') resolve({ code, signal });
      else reject(new Error(`crawlex exited with code ${code}${signal ? ` (signal ${signal})` : ''}`));
    });
  });

  async function* iterate() {
    try {
      for await (const line of rl) {
        if (!line) continue;
        const msg = parseLine(line);
        // Bridge envelopes carry `kind`; NDJSON event envelopes carry
        // `event`. Disambiguate to either route through the dispatcher
        // or yield to the consumer.
        if (opts.hooks && msg && msg.kind && msg.kind !== 'raw') {
          if (!helloAcked && msg.kind === 'hello') {
            helloAcked = true;
            writeBridge({ kind: 'subscribe', subscribed: opts.hooks.subscribed });
            continue;
          }
          if (msg.kind === 'hook_invoke') {
            // Run the dispatcher in the background so a slow handler
            // doesn't block reading subsequent events. Replies are
            // strictly ordered by `id`, not by stdout arrival order.
            opts.hooks
              .dispatch(msg)
              .then((reply) => {
                if (reply) writeBridge(reply);
              })
              .catch((e) => {
                writeBridge({
                  kind: 'hook_result',
                  id: msg.id,
                  decision: 'abort',
                  patch: { user_data: { hook_dispatch_error: String(e) } },
                });
              });
            continue;
          }
        }
        yield msg;
      }
      await exited;
    } finally {
      rl.close();
      if (opts.hooks) {
        try { child.stdin.end(); } catch (_) {}
      }
      if (!child.killed) child.kill('SIGTERM');
    }
  }

  return { [Symbol.asyncIterator]: iterate, child, done: exited };
}

/**
 * Collect all events into an array. Convenience for small crawls/tests.
 */
async function crawlAll(opts) {
  const stream = crawl(opts);
  const events = [];
  for await (const e of stream) events.push(e);
  return events;
}

/**
 * One-shot JSON command (buffered). For subcommands like
 * `crawlex discover sitemap <url> --json`.
 */
function runJson(args, { bin, env, input } = {}) {
  const exe = bin || binaryPath();
  const res = spawnSync(exe, [...args, '--json'], {
    input,
    encoding: 'utf8',
    env: { ...process.env, ...(env || {}) },
  });
  if (res.status !== 0) {
    const err = new Error(`crawlex ${args.join(' ')} failed (${res.status})`);
    err.stderr = res.stderr;
    err.stdout = res.stdout;
    throw err;
  }
  return JSON.parse(res.stdout);
}

// ---------- CLI passthrough ----------

function runCli(argv) {
  const bin = binaryPath();
  if (!fs.existsSync(bin) && !process.env.CRAWLEX_FORCE_BINARY) {
    process.stderr.write(
      `crawlex: binary not found at ${bin}\n` +
      `  run \`pnpm install --force crawlex\` or set CRAWLEX_FORCE_BINARY.\n`
    );
    process.exit(1);
  }
  const child = spawn(bin, argv, { stdio: 'inherit' });
  child.on('exit', (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    else process.exit(code ?? 0);
  });
}

// ---------- JS hook bridge ----------
//
// `defineHooks({...})` returns an object the SDK feeds into the
// `--hook-bridge` IPC channel exposed by the rust binary. The wire
// protocol mirrors `src/hooks/bridge.rs`:
//
//   rust → js (stdout):  {kind:"hello", v, protocol}
//                        {kind:"hook_invoke", id, event, ctx}
//   js   → rust (fd):    {kind:"subscribe", subscribed:[...]}
//                        {kind:"hook_result", id, decision, patch?}
//
// The handler signature: `(ctx, helpers) => Decision | Promise<Decision>`
// where `Decision` is one of `'continue' | 'skip' | 'retry' | 'abort'`
// or an object `{ decision, patch?: { capturedUrls?, userData?,
// robotsAllowed?, allowRetry? } }`.

const HOOK_EVENTS = Object.freeze([
  'before_each_request',
  'after_dns_resolve',
  'after_tls_handshake',
  'after_first_byte',
  'on_response_body',
  'after_load',
  'after_idle',
  'on_discovery',
  'on_job_start',
  'on_job_end',
  'on_error',
  'on_robots_decision',
]);

// camelCase JS handler name → wire event name. Handler keys also
// accept the snake_case wire name directly so callers can copy-paste
// from the rust enum if they prefer.
const HANDLER_TO_EVENT = {
  onBeforeEachRequest: 'before_each_request',
  onAfterDnsResolve: 'after_dns_resolve',
  onAfterTlsHandshake: 'after_tls_handshake',
  onAfterFirstByte: 'after_first_byte',
  onResponseBody: 'on_response_body',
  onAfterLoad: 'after_load',
  onAfterIdle: 'after_idle',
  onDiscovery: 'on_discovery',
  onJobStart: 'on_job_start',
  onJobEnd: 'on_job_end',
  onError: 'on_error',
  onRobotsDecision: 'on_robots_decision',
};

const DECISION_SET = new Set(['continue', 'skip', 'retry', 'abort']);

/**
 * Build a hook spec from an `{ onAfterFirstByte: async (ctx) => ... }`
 * map. Returns `{ subscribed, dispatch }`:
 *
 *   subscribed: string[]      // wire event names the SDK listens to
 *   dispatch(envelope) → reply  // takes an inbound `hook_invoke`,
 *                                  returns the matching `hook_result`
 *
 * The `crawl()` integration uses both: subscribed → emitted in the
 * `subscribe` envelope; dispatch → bound to the bridge read loop.
 */
function defineHooks(handlerMap = {}) {
  const handlers = {};
  for (const [key, fn] of Object.entries(handlerMap)) {
    if (typeof fn !== 'function') continue;
    let event;
    if (HANDLER_TO_EVENT[key]) {
      event = HANDLER_TO_EVENT[key];
    } else if (HOOK_EVENTS.includes(key)) {
      event = key;
    } else {
      throw new Error(
        `defineHooks: unknown handler '${key}'. Expected one of: ` +
        Object.keys(HANDLER_TO_EVENT).concat(HOOK_EVENTS).join(', '),
      );
    }
    handlers[event] = fn;
  }

  const subscribed = Object.keys(handlers);

  async function dispatch(envelope) {
    if (!envelope || envelope.kind !== 'hook_invoke') return null;
    const { id, event, ctx } = envelope;
    const handler = handlers[event];
    if (!handler) {
      return { kind: 'hook_result', id, decision: 'continue' };
    }
    let raw;
    try {
      raw = await handler(ctx);
    } catch (err) {
      // A throwing JS hook becomes an `abort` so the rust side surfaces
      // it in the NDJSON event stream as `decision.made why=hook:throw`.
      // The error message lands in `user_data.hook_error` so consumers
      // can correlate.
      return {
        kind: 'hook_result',
        id,
        decision: 'abort',
        patch: {
          user_data: {
            ...(ctx?.user_data || {}),
            hook_error: String(err && err.message ? err.message : err),
          },
        },
      };
    }
    return normalizeResult(id, raw);
  }

  return { subscribed, dispatch };
}

function normalizeResult(id, raw) {
  // Allow shorthand: handler returns 'skip' instead of `{ decision: 'skip' }`.
  if (typeof raw === 'string') {
    if (!DECISION_SET.has(raw)) {
      throw new Error(`hook returned unknown decision: '${raw}'`);
    }
    return { kind: 'hook_result', id, decision: raw };
  }
  if (raw == null) {
    return { kind: 'hook_result', id, decision: 'continue' };
  }
  const decision = raw.decision || 'continue';
  if (!DECISION_SET.has(decision)) {
    throw new Error(`hook returned unknown decision: '${decision}'`);
  }
  // Translate the camelCase JS patch fields onto the snake_case wire
  // names so the rust `ContextPatch` deserialiser sees them as-is.
  const patch = raw.patch ? camelToSnakePatch(raw.patch) : undefined;
  return { kind: 'hook_result', id, decision, ...(patch ? { patch } : {}) };
}

function camelToSnakePatch(p) {
  const out = {};
  if (Array.isArray(p.capturedUrls)) out.captured_urls = p.capturedUrls.map(String);
  if (p.userData && typeof p.userData === 'object') out.user_data = p.userData;
  if (typeof p.robotsAllowed === 'boolean') out.robots_allowed = p.robotsAllowed;
  if (typeof p.allowRetry === 'boolean') out.allow_retry = p.allowRetry;
  return out;
}

module.exports = {
  crawl,
  crawlAll,
  runJson,
  ensureInstalled,
  binaryPath,
  assetBaseName,
  serializeArgs,
  defineHooks,
  HOOK_EVENTS,
  version: SDK_VERSION,
};

if (require.main === module) {
  runCli(process.argv.slice(2));
}
