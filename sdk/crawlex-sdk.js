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
const PKG_JSON = require(path.join(PKG_ROOT, 'package.json'));
const BIN_DIR = path.join(PKG_ROOT, '.crawlex', 'bin');
const BIN_NAME = process.platform === 'win32' ? 'crawlex.exe' : 'crawlex';
const DEFAULT_REPO =
  process.env.CRAWLEX_REPO ||
  (PKG_JSON.repository && PKG_JSON.repository.url
    ? PKG_JSON.repository.url.replace(/^git\+/, '').replace(/\.git$/, '').replace(/^https?:\/\/github\.com\//, '')
    : 'forattini-dev/crawlex');

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
    const req = https.get(url, { headers: { 'user-agent': `crawlex-sdk/${PKG_JSON.version}`, ...headers } }, (res) => {
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
    version = process.env.CRAWLEX_POSTINSTALL_VERSION || PKG_JSON.version,
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
  if (opts.config) args.push('--config', '-');
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

  if (opts.config) {
    child.stdin.write(JSON.stringify(opts.config));
  }
  child.stdin.end();

  const rl = readline.createInterface({ input: child.stdout, crlfDelay: Infinity });

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
        yield parseLine(line);
      }
      await exited;
    } finally {
      rl.close();
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

module.exports = {
  crawl,
  crawlAll,
  runJson,
  ensureInstalled,
  binaryPath,
  assetBaseName,
  serializeArgs,
  version: PKG_JSON.version,
};

if (require.main === module) {
  runCli(process.argv.slice(2));
}
