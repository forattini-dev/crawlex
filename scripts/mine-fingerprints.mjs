#!/usr/bin/env node
// Mine TLS fingerprint hashes (JA3 / JA4) from public databases and emit
// validation oracles for our catalog.
//
// Sources:
//   - tls.peet.ws / api / all  → returns the JA3/JA4 of YOUR connection,
//     so we don't actually mine here directly. Instead we pull the
//     curated dataset at https://tls.peet.ws/api/clean (community-shared
//     captures keyed by UA).
//   - ja4db.com / api / v1     → searchable JA4 database with
//     `?software=Chrome&version=149` queries.
//
// Usage:
//   node scripts/mine-fingerprints.mjs            # full sync
//   node scripts/mine-fingerprints.mjs --dry-run  # report only
//
// Output:
//   references/mined-fingerprints/peet-ws-snapshot-<ISO>.json    (raw)
//   references/mined-fingerprints/ja4db-snapshot-<ISO>.json      (raw)
//   src/impersonate/catalog/mined/<browser>_<major>_<os>.json    (oracles)
//
// The oracle JSON shape is intentionally tiny — we want JA3/JA4 hashes
// only, not raw ClientHello bytes (mining APIs don't return those):
//   {
//     "name": "chrome_149_linux",
//     "browser": "chrome", "major": 149, "os": "linux",
//     "ja3": "771,4865-...", "ja3_hash": "...md5...",
//     "ja4": "t13d1516h2_...",
//     "source": "ja4db.com",
//     "captured_at": "2026-04-25T18:30:00Z"
//   }
//
// build.rs picks these up and emits `pub static MINED_HASHES` consts so
// roundtrip tests can compare our generated JA3/JA4 against community
// observations. Treats mismatches as warn (mining is anonymized so
// outliers happen) but emits a report.

import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..');

const TARGET_BROWSERS = {
  chrome: { range: [120, 149] },
  chromium: { range: [120, 149] },
  firefox: { range: [111, 130] },
};

const TARGET_OSES = ['linux', 'windows', 'macos', 'android'];

const PEET_WS_URL = 'https://tls.peet.ws/api/clean';
const JA4DB_URL = 'https://ja4db.com/api/v1/lookup';

function nowIso() {
  return new Date().toISOString().replace(/\.\d+/, '');
}

async function fetchJson(url, options = {}) {
  const userAgent = options.userAgent || 'crawlex-mine-fingerprints/1.0';
  const res = await fetch(url, {
    headers: { 'user-agent': userAgent, accept: 'application/json' },
    signal: AbortSignal.timeout(15_000),
  });
  if (!res.ok) {
    throw new Error(`GET ${url} -> HTTP ${res.status}`);
  }
  return await res.json();
}

function parseUaForTargets(ua) {
  // Chrome / Chromium UAs both contain `Chrome/<major>.<minor>...`
  const chromeMatch = ua.match(/Chrome\/(\d+)\./);
  const firefoxMatch = ua.match(/Firefox\/(\d+)\./);
  const isChromium = /Chromium/.test(ua) && !/Edg\//.test(ua) && !/Brave/.test(ua);
  const isEdge = /Edg\//.test(ua);
  const isFirefox = !!firefoxMatch && !/Chrome/.test(ua);

  // OS detection.
  let os = 'other';
  if (/Windows NT/.test(ua)) os = 'windows';
  else if (/Macintosh|Mac OS X/.test(ua)) os = 'macos';
  else if (/Android/.test(ua)) os = 'android';
  else if (/Linux/.test(ua)) os = 'linux';

  if (isFirefox) {
    return { browser: 'firefox', major: Number(firefoxMatch[1]), os };
  }
  if (isChromium) {
    return { browser: 'chromium', major: Number(chromeMatch[1]), os };
  }
  if (chromeMatch && !isEdge) {
    return { browser: 'chrome', major: Number(chromeMatch[1]), os };
  }
  return null;
}

function isTargeted(target) {
  if (!target) return false;
  const conf = TARGET_BROWSERS[target.browser];
  if (!conf) return false;
  return target.major >= conf.range[0] && target.major <= conf.range[1];
}

async function minePeetWs() {
  console.log(`[peet.ws] fetching ${PEET_WS_URL}`);
  let payload;
  try {
    payload = await fetchJson(PEET_WS_URL);
  } catch (err) {
    console.warn(`[peet.ws] fetch failed: ${err.message}`);
    return [];
  }

  const out = [];
  // Schema may be an array or {entries: [...]}; tolerate both.
  const entries = Array.isArray(payload) ? payload : payload?.entries || [];

  for (const entry of entries) {
    const ua = entry?.user_agent || entry?.ua;
    if (!ua) continue;
    const target = parseUaForTargets(ua);
    if (!isTargeted(target)) continue;
    const ja3 = entry?.tls?.ja3 || entry?.ja3 || null;
    const ja3Hash = entry?.tls?.ja3_hash || entry?.ja3_hash || null;
    const ja4 = entry?.tls?.ja4 || entry?.ja4 || null;
    if (!ja3 && !ja4) continue;
    out.push({
      name: `${target.browser}_${target.major}_${target.os}`,
      browser: target.browser,
      major: target.major,
      os: target.os,
      ja3,
      ja3_hash: ja3Hash,
      ja4,
      source: 'tls.peet.ws',
      user_agent: ua,
      captured_at: nowIso(),
    });
  }

  console.log(`[peet.ws] ${out.length} entries match target browsers`);
  return out;
}

async function mineJa4Db() {
  // ja4db.com lookup API: GET /api/v1/lookup?software=Chrome&version=149
  // Returns JA4 records keyed by software+version.
  const out = [];
  for (const [browser, { range }] of Object.entries(TARGET_BROWSERS)) {
    for (let major = range[0]; major <= range[1]; major++) {
      const software = browser === 'firefox' ? 'Firefox' : 'Chrome';
      const url = `${JA4DB_URL}?software=${software}&version=${major}`;
      try {
        const payload = await fetchJson(url);
        const entries = Array.isArray(payload) ? payload : payload?.results || [];
        for (const entry of entries) {
          const os = (entry?.os || entry?.platform || 'unknown').toLowerCase();
          if (!entry?.ja4) continue;
          out.push({
            name: `${browser}_${major}_${os}`,
            browser,
            major,
            os: ['windows', 'macos', 'linux', 'android'].includes(os) ? os : 'other',
            ja3: entry?.ja3 || null,
            ja3_hash: entry?.ja3_hash || null,
            ja4: entry.ja4,
            source: 'ja4db.com',
            captured_at: nowIso(),
          });
        }
      } catch (err) {
        // ja4db is opt-in; don't fail the whole script if a single
        // (browser, version) pair has no record.
        // console.warn(`[ja4db] ${software}/${major} skipped: ${err.message}`);
      }
    }
  }
  console.log(`[ja4db] ${out.length} entries`);
  return out;
}

function mergeOracles(peet, ja4db) {
  // Index by name. Prefer ja4db (curated) when both have a hit; peet.ws
  // entries supplement when ja4db is empty. Carry both ja3 and ja4 if
  // available.
  const byName = new Map();
  for (const e of peet) {
    byName.set(e.name, e);
  }
  for (const e of ja4db) {
    const existing = byName.get(e.name);
    if (!existing) {
      byName.set(e.name, e);
    } else {
      byName.set(e.name, {
        ...existing,
        ja4: e.ja4 || existing.ja4,
        ja3: e.ja3 || existing.ja3,
        ja3_hash: e.ja3_hash || existing.ja3_hash,
        source: existing.source === e.source ? existing.source : `${existing.source}+${e.source}`,
      });
    }
  }
  return [...byName.values()];
}

async function main() {
  const dryRun = process.argv.includes('--dry-run');

  const peetDir = path.join(REPO_ROOT, 'references/mined-fingerprints');
  const minedDir = path.join(REPO_ROOT, 'src/impersonate/catalog/mined');
  await fs.mkdir(peetDir, { recursive: true });
  await fs.mkdir(minedDir, { recursive: true });

  const peet = await minePeetWs();
  const ja4db = await mineJa4Db();

  const ts = nowIso().replace(/[:]/g, '-');
  if (!dryRun) {
    await fs.writeFile(
      path.join(peetDir, `peet-ws-snapshot-${ts}.json`),
      JSON.stringify(peet, null, 2),
    );
    await fs.writeFile(
      path.join(peetDir, `ja4db-snapshot-${ts}.json`),
      JSON.stringify(ja4db, null, 2),
    );
  }

  const oracles = mergeOracles(peet, ja4db);
  console.log(`merged ${oracles.length} unique oracle entries`);

  if (!dryRun) {
    for (const oracle of oracles) {
      const fname = `${oracle.name}.json`;
      await fs.writeFile(
        path.join(minedDir, fname),
        JSON.stringify(oracle, null, 2),
      );
    }
  }

  // Print coverage report.
  const covered = new Set(oracles.map((o) => `${o.browser}_${o.major}_${o.os}`));
  const expected = [];
  for (const [browser, { range }] of Object.entries(TARGET_BROWSERS)) {
    for (let major = range[0]; major <= range[1]; major++) {
      for (const os of TARGET_OSES) {
        if (browser === 'chromium' && os !== 'linux') continue;
        expected.push(`${browser}_${major}_${os}`);
      }
    }
  }

  const missing = expected.filter((k) => !covered.has(k));
  console.log(`\ncoverage: ${covered.size} / ${expected.length}`);
  if (missing.length) {
    console.log(`missing ${missing.length} tuples (top 10):`);
    for (const k of missing.slice(0, 10)) console.log(`  - ${k}`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
