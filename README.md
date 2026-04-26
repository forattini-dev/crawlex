<div align="center">

# 🕸️ crawlex

### **The stealth crawler that actually looks like Chrome.**

TLS, HTTP/2, JS fingerprint — every byte indistinguishable from real Chrome 149.<br>
Rust core • Node SDK • Lua hooks • cross-platform binaries.

[![CI](https://github.com/forattini-dev/crawlex/actions/workflows/ci.yml/badge.svg)](https://github.com/forattini-dev/crawlex/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/crawlex.svg?logo=rust)](https://crates.io/crates/crawlex)
[![npm](https://img.shields.io/npm/v/crawlex.svg?logo=npm)](https://www.npmjs.com/package/crawlex)
[![docs](https://img.shields.io/badge/docs-docsify-success.svg)](https://forattini-dev.github.io/crawlex/)
[![downloads](https://img.shields.io/crates/d/crawlex.svg)](https://crates.io/crates/crawlex)
[![license](https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-blue.svg)](#license)

```bash
pnpm add -g crawlex && crawlex pages run --seed https://example.com --method render
```

[**Quickstart**](#-quickstart) · [**Features**](#-features) · [**Examples**](#-examples) · [**Docs**](https://forattini-dev.github.io/crawlex/) · [**Why crawlex**](#-why-crawlex)

</div>

---

## ⚡ Why crawlex

Standard crawlers fail on the first Cloudflare wall. `crawlex` arrives the way **real Chrome** arrives — every fingerprint surface is identical, not approximated.

<table>
<tr><th>Layer</th><th>What we match — exactly, not approximately</th></tr>
<tr><td>🔐 <strong>TLS ClientHello</strong></td><td>Extension order, ALPS, GREASE values, <code>permute_extensions</code>, X25519MLKEM768, signature algorithms — verified against <a href="https://tls.peet.ws">tls.peet.ws</a> and <a href="https://ja4db.com">ja4db.com</a> oracles</td></tr>
<tr><td>🚦 <strong>HTTP/2 frame</strong></td><td>Pseudo-header order <code>:method :authority :scheme :path</code>, SETTINGS frame parameters, WINDOW_UPDATE pattern — passes Akamai BMP signature checks</td></tr>
<tr><td>🎭 <strong>JS fingerprint</strong></td><td>29-section stealth shim: <code>navigator</code>, <code>chrome.*</code>, permissions, plugins, screen, timezone, battery, WebGL (vendor / params / extensions), canvas (zero-preserving noise), AudioContext (FFT + offline render), <code>Function.prototype.toString</code> proxy, WebGPU, <code>performance.memory</code>, sensors, iframe, requestAnimationFrame throttle, <code>performance.now()</code> 100µs grain, mediaDevices, fonts, WebRTC SDP/ICE/getStats scrub</td></tr>
<tr><td>🤖 <strong>Behavior</strong></td><td>Mouse jitter, scroll cadence, dwell time, idle drift — coherent <code>motion::</code> profiles per persona</td></tr>
<tr><td>📦 <strong>Catalog</strong></td><td>30 Chrome stable × 30 Chromium × 20 Firefox × Edge × Safari fingerprints. Era-fallback resolution: ask for <code>chrome-149-linux</code>, get the closest captured profile</td></tr>
<tr><td>🛠️ <strong>Worker scope</strong></td><td>Same shim auto-attached to dedicated / shared / service workers via CDP <code>Target.setAutoAttach</code> — Camoufox port</td></tr>
</table>

→ Validated against [BrowserScan](https://browserscan.net), [CreepJS](https://abrahamjuliot.github.io/creepjs/), [Sannysoft](https://bot.sannysoft.com/), [tls.peet.ws](https://tls.peet.ws), [ja4db.com](https://ja4db.com).

---

## 🚀 Install

```bash
# npm — bundled binary download via postinstall
pnpm add -g crawlex

# Rust — from source
cargo install crawlex

# Direct binary (linux x86_64/arm64, macOS x86_64/arm64, windows x86_64)
# https://github.com/forattini-dev/crawlex/releases/latest
```

> ⚠️ **Production crawls run locally**, never in CI. Datacenter IPs (GitHub Actions, AWS, Azure) are flagged instantly by every modern WAF.

---

## 🏃 Quickstart

```bash
# Stealth render with persona, sitemap discovery, NDJSON event stream
crawlex pages run \
  --seed https://target.com \
  --method render \
  --persona atlas \
  --max-depth 3 \
  --screenshot \
  --emit ndjson > events.ndjson

# Live tail what just happened
jq -c 'select(.event == "fetch.completed" or .event == "render.completed")' events.ndjson
```

Three integration paths, your pick:

<table>
<tr><th>CLI</th><th>Node SDK</th><th>Embedded Rust</th></tr>
<tr><td>

```bash
crawlex pages run \
  --seed https://...\
  --method render \
  --persona pixel \
  --emit ndjson
```

One-shot crawls, scripted pipelines.

</td><td>

```ts
import { crawl, defineHooks } from 'crawlex';

for await (const ev of crawl({
  seeds: ['https://...'],
  args: { method: 'render' },
})) { ... }
```

Production services with hook logic.

</td><td>

```rust
use crawlex::{Crawler, Config};
let crawler = Crawler::new(
    Config::builder().build()?
)?;
crawler.run().await?;
```

In-process embedding, zero IPC.

</td></tr>
</table>

---

## 🎨 Examples

### 1. Hunt a SaaS product page with vitals + screenshot

```ts
import { crawl } from 'crawlex';

for await (const ev of crawl({
  seeds: ['https://stripe.com/pricing'],
  args: {
    method: 'render',
    persona: 'atlas',                 // macOS Apple M1, Retina, en-US
    screenshot: true,
    screenshotMode: 'fullpage',
    storage: 'filesystem',
    storagePath: './out',
    waitStrategy: '{"NetworkIdle":{"idle_ms":1500}}',
  },
})) {
  if (!('event' in ev)) continue;
  switch (ev.event) {
    case 'render.completed':
      console.log(`✅ ${ev.url} | LCP=${ev.data.vitals.largest_contentful_paint_ms}ms | CLS=${ev.data.vitals.cumulative_layout_shift}`);
      break;
    case 'artifact.saved':
      if (ev.data.kind === 'screenshot.full_page')
        console.log(`📸 → out/${ev.data.path}  (${(ev.data.size/1024).toFixed(0)}kB)`);
      break;
    case 'challenge.detected':
      console.log(`🚧 ${ev.data.vendor} (${ev.data.level}) on ${ev.url}`);
      break;
  }
}
```

### 2. Crawl an entire domain with proxy rotation + retry policy

```ts
import { crawl, defineHooks } from 'crawlex';

const hooks = defineHooks({
  // Rate-limit retry: 429/503 → re-enqueue (up to retry_max)
  async onAfterFirstByte(ctx) {
    if (ctx.response_status === 429 || ctx.response_status === 503) return 'retry';
    return 'continue';
  },
  // Inject the canonical sitemap.xml for every host we touch
  async onDiscovery(ctx) {
    const host = new URL(ctx.url).host;
    return {
      decision: 'continue',
      patch: { capturedUrls: [...ctx.captured_urls, `https://${host}/sitemap.xml`] },
    };
  },
  // Tag the crawl with custom metadata that lands in user_data
  async onJobStart(ctx) {
    return {
      decision: 'continue',
      patch: { userData: { ...ctx.user_data, run_owner: 'qa-bot' } },
    };
  },
});

for await (const ev of crawl({
  seeds: ['https://target.com'],
  args: {
    method: 'auto',                   // policy engine picks http vs render
    maxConcurrentHttp: 8,
    maxConcurrentRender: 2,
    maxDepth: 5,
    crtsh: true,                      // certificate-transparency seeding
    storage: 'sqlite',
    storagePath: './crawl.db',
    queue: 'sqlite',
    queuePath: './crawl.db',
    proxies: ['http://user:pass@proxy1:8080', 'http://user:pass@proxy2:8080'],
    proxyStrategy: 'health-weighted',
    proxyStickyPerHost: true,
  },
  hooks,
  signal: AbortSignal.timeout(30 * 60_000),
})) {
  if (!('event' in ev)) continue;
  if (ev.event === 'job.failed') console.error(`✗ ${ev.url} — ${ev.data.error}`);
  if (ev.event === 'run.completed') console.log('done.');
}
```

### 3. Embedded library with custom Rust hooks

```rust
use crawlex::{Config, Crawler, queue::FetchMethod};
use crawlex::hooks::{HookDecision, HookRegistry};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::main]
async fn main() -> crawlex::Result<()> {
    let hooks = HookRegistry::new();
    let pages_seen = Arc::new(AtomicUsize::new(0));

    // Closure-captured counter — observe without intervening
    let counter = pages_seen.clone();
    hooks.on_response_body(move |_ctx| {
        let c = counter.clone();
        Box::pin(async move {
            c.fetch_add(1, Ordering::Relaxed);
            Ok(HookDecision::Continue)
        })
    });

    // Domain-level deny list — short-circuit before fetch
    hooks.on_before_each_request(|ctx| {
        let url = ctx.url.clone();
        Box::pin(async move {
            if url.path().starts_with("/admin/") { return Ok(HookDecision::Skip); }
            Ok(HookDecision::Continue)
        })
    });

    let config = Config::builder()
        .max_concurrent_http(16)
        .build()?;

    let crawler = Crawler::new(config)?.with_hooks(hooks);
    crawler.seed_with(
        vec!["https://target.com".parse().unwrap()],
        FetchMethod::HttpSpoof,
    ).await?;
    crawler.run().await?;

    println!("Crawled {} pages", pages_seen.load(Ordering::Relaxed));
    Ok(())
}
```

→ Full runnable example: [`examples/embedded_with_hooks.rs`](examples/embedded_with_hooks.rs)

### 4. Pin a specific browser fingerprint from the catalog

```bash
# Browse 80+ ready-to-use fingerprints
crawlex stealth catalog list
crawlex stealth catalog list --filter chrome
crawlex stealth catalog show chrome-149-linux

# Pin a precise version + OS
crawlex pages run --seed https://target.com \
  --profile chrome-149-linux

# Era fallback: chromium-122 not captured? falls back to closest era + warns
crawlex pages run --seed https://target.com \
  --profile chromium-122-linux

# Mobile persona (touch viewport, sec-ch-ua-mobile: ?1)
crawlex pages run --seed https://target.com \
  --method render --persona pixel
```

### 5. Inspect what your stealth stack actually emits

```bash
# Print active IdentityBundle + TLS profile summary
crawlex stealth inspect --profile chrome-149-linux

# Verify ALPN/cipher/JA4 against built-in expectations
crawlex stealth test

# Compare against tls.peet.ws / ja4db.com via the live oracle
crawlex stealth catalog show chrome-149-linux --json
```

---

## 🎯 Features

<table>
<tr>
<td width="50%" valign="top">

### 🥷 Stealth core
- 🔐 Chrome 149 TLS via BoringSSL fork
- 🚦 H2 pseudo-header order patch
- 🎭 29-section JS shim — full leak inventory covered
- 🤖 Worker scope shim (dedicated / shared / SW)
- 📦 80+ browser fingerprints from curl-impersonate + ja4db + tls.peet
- 🌍 5 personas: `tux`, `office`, `gamer`, `atlas`, `pixel`
- 🎬 Coherent `motion::` profiles (mouse / scroll / dwell)
- 🕸️ WebRTC scrub (SDP, ICE, getStats — public-interface only)

### 🔍 Discovery
- 🗺️ Sitemap recursion + robots.txt parsing
- 🔎 Certificate transparency (crt.sh)
- 🌐 DNS records + RDAP + Wayback CDX
- 📜 PWA manifest + service worker probes
- 📂 `.well-known/*` enumeration
- 🔬 Tech fingerprinting (Wappalyzer-class)
- 🔌 JS endpoint extraction from runtime
- 🛡️ security.txt parser
- 🧬 Asset-ref classification (JS / CSS / image / API / nav)
- 🔓 TCP port scan (opt-in, network-active)

### 🛡️ Antibot policy engine
- 🚧 Detect: Cloudflare, DataDome, PerimeterX, Akamai BMP, Imperva, hCaptcha, reCAPTCHA, Turnstile
- 📊 Vendor telemetry observer (passive — sees outbound calls to known endpoints)
- 🔄 Policy decisions: keep / drop / retry / scope-demote / proxy-rotate / give-up
- 🎯 4 captcha solver adapters: in-house reCAPTCHA v3, 2captcha, anticaptcha, VLM

</td>
<td width="50%" valign="top">

### ⚙️ Pipeline
- 🎯 Render pool — Chromium auto-fetch + isolated user-data dirs
- 🔁 Persistent queue: in-memory / SQLite / Redis backends
- 💾 Storage: filesystem / SQLite / memory — opt-in per concern (artifact, state, challenge, telemetry, intel)
- 🔄 Proxy rotator — health checks + sticky sessions + per-host affinity
- 📊 Web Vitals + per-fetch network breakdown (DNS / TCP / TLS / TTFB / download)
- 🎬 ScriptSpec runner — declarative `Plan` execution with assertions
- 🔧 Frontier with dedupe + rate-limit + retry policies
- 📐 Wait strategies: `Load`, `DOMContentLoaded`, `NetworkIdle`, `Selector`, `Fixed`

### 📡 Observability
- 📜 NDJSON event stream — versioned envelope (`v: 1`)
- 🎬 19 event kinds covering full lifecycle
- 🔬 Embedded `WebVitals` summary on `render.completed`
- ⏱️ Per-request timings on `fetch.completed` (ALPN, cipher, TLS version)
- 📸 Artifact descriptors with on-disk path on the wire
- 🪝 Hooks: 12 lifecycle points × 3 languages (Rust / JS / Lua)
- 📊 Prometheus metrics endpoint

### 🔌 Integrations
- 📦 npm + crates.io + GitHub Releases
- 🦀 Rust library — embed `Crawler` directly
- 📘 TypeScript types — strict, full envelope coverage
- 🔌 SDK `crawl()` async iterator
- 📚 docsify docs site (GitHub Pages)
- 🧪 386+ lib tests, 27 fpjs compliance, TLS catalog roundtrip suite
- 🔐 Optional Lua hooks (`mlua`)
- 🪶 Two binaries: `crawlex` (full) + `crawlex-mini` (HTTP-only, no Chromium)

</td>
</tr>
</table>

---

## 📡 NDJSON event stream

Every run emits one JSON envelope per line on stdout. Versioned, stable, 19 kinds:

```jsonl
{"v":1,"event":"run.started","ts":"2026-04-26T19:42:00.000Z","run_id":42,"data":{"policy_profile":"strict","max_concurrent_http":8,"max_concurrent_render":2}}
{"v":1,"event":"job.started","run_id":42,"url":"https://target.com/","data":{"job_id":"j_001","method":"render","depth":0,"priority":0,"attempts":0}}
{"v":1,"event":"fetch.completed","run_id":42,"url":"https://target.com/","data":{"final_url":"https://target.com/","status":200,"bytes":98234,"body_truncated":false,"dns_ms":12,"tcp_connect_ms":18,"tls_handshake_ms":24,"ttfb_ms":142,"download_ms":83,"total_ms":280,"alpn":"h2","tls_version":"TLSv1.3","cipher":"TLS_AES_128_GCM_SHA256"}}
{"v":1,"event":"render.completed","run_id":42,"session_id":"sess_abc","url":"https://target.com/","data":{"final_url":"https://target.com/","status":200,"manifest":true,"service_workers":1,"is_spa":true,"vitals":{"ttfb_ms":142,"first_contentful_paint_ms":380.5,"largest_contentful_paint_ms":920.1,"cumulative_layout_shift":0.03,"total_blocking_time_ms":50.0,"dom_nodes":1842,"js_heap_used_bytes":12345678,"resource_count":45,"total_transfer_bytes":982341}}}
{"v":1,"event":"artifact.saved","run_id":42,"url":"https://target.com/","data":{"kind":"screenshot.full_page","mime":"image/png","size":1234567,"sha256":"a1b2c3...","path":"artifacts/sess_abc/1714123456_screenshot_full_page_a1b2c3d4.png"}}
{"v":1,"event":"challenge.detected","run_id":42,"url":"https://protected.com/","data":{"vendor":"cloudflare_turnstile","level":"widget_present"}}
{"v":1,"event":"decision.made","run_id":42,"url":"https://protected.com/","why":"render:js-challenge","data":{"decision":"retry","reason":{"code":"render:js-challenge"}}}
{"v":1,"event":"run.completed","run_id":42}
```

**Discriminator key:** `event` (snake_case) — TypeScript narrows via `switch (ev.event) { … }`. Fallback for malformed lines: `{ kind: 'raw', line }` so consumers can log/recover.

---

## 🪝 Hooks — 12 lifecycle points × 3 languages

```
before_each_request → after_dns → after_tls → after_first_byte → on_response_body
   → after_load → after_idle → on_discovery → on_job_start → on_job_end
   → on_error → on_robots_decision
```

| Language | API | Best for |
|---|---|---|
| **Rust** | `hooks.on_after_first_byte(closure)` — full `&mut HookContext` access | Embedded library, latency-critical paths |
| **JS / TS** | `defineHooks({...})` via SDK — IPC bridge, async closures | Production crawls, business logic |
| **Lua** | `--hook-script foo.lua` — page-driving helpers (`page_click`, `page_eval`) | Ad-hoc scripts, no build step |

**All three modes return the same decision:** `continue` / `skip` / `retry` / `abort`. Hooks can mutate `ctx.captured_urls`, inject extra URLs, write to `user_data` to communicate with downstream hooks, or override `robots_allowed`.

---

## 🎭 Personas — coherent identity bundles

Each persona is a complete bundle — UA + Sec-CH-UA + screen + viewport + DPR + GPU + fonts + media-device counts + TLS profile + motion timings — so every signal **matches**. No mismatched UA + WebGL combo gives you away.

| Codename | OS | GPU | Locale | Form factor |
|---|---|---|---|---|
| 🐧 `tux` | Linux | Intel UHD 630 | en-US | desktop 1920×1080 |
| 🏢 `office` | Windows 10 | Intel UHD 620 | en-US | laptop 1920×1080 (DPR 1.25) |
| 🎮 `gamer` | Windows 10 | NVIDIA GTX 1060 | pt-BR | desktop 1920×1080 |
| 🍎 `atlas` | macOS | Apple M1 | en-US | retina 1440×900 (DPR 2.0) |
| 📱 `pixel` | Android 14 | Adreno 640 | pt-BR | **mobile** 412×823 (DPR 2.625) |

```bash
crawlex pages run --seed https://target.com --persona atlas    # macOS
crawlex pages run --seed https://target.com --persona pixel    # mobile
```

---

## 🏗️ Architecture

```mermaid
flowchart LR
  S[Seeds] --> Q[Frontier<br/>+ dedupe + rate-limit]
  Q --> P[Policy Engine]
  P -->|http| F[ImpersonateClient<br/>BoringSSL + h2 patched]
  P -->|render| R[RenderPool<br/>Chromium + stealth shim]
  F --> X[Extractor<br/>+ Asset Refs]
  R --> X
  X --> D[Discovery<br/>Pipeline]
  X --> ST[Storage<br/>5 traits]
  D --> Q
  P --> EV[NDJSON Events<br/>19 kinds]
  R --> H1[Rust Hooks]
  R --> H2[JS Bridge]
  R --> H3[Lua Scripts]
```

**Module map:**
- `impersonate/` — TLS catalog + BoringSSL connector + ALPS + GREASE
- `render/` — Chromium pool + 29-section stealth shim + motion engine + ScriptSpec runner
- `discovery/` — 17-stage pipeline (DNS, RDAP, sitemap, robots, crtsh, wayback, well-known, …)
- `policy/` — pure engine: `decide_pre_fetch`, `decide_post_fetch`, `decide_post_error`, `decide_post_challenge`
- `antibot/` — vendor classifier + 4 captcha solver adapters
- `storage/` — 5 concern-oriented traits (artifact / state / challenge / telemetry / intel)
- `events/` — NDJSON envelope + sink (stdout / null / memory)
- `hooks/` — registry + JS bridge + Lua host

---

## 🛠️ Tech stack

| Layer | Implementation |
|---|---|
| TLS | `boring-sys` — BoringSSL fork with ALPS / permute_extensions / X25519MLKEM768 |
| HTTP/2 | Vendored `h2` crate with pseudo-header order patch (`vendor/h2`) |
| CDP | chromiumoxide-derived, embedded behind `cdp-backend` feature |
| Async | tokio multi-thread |
| Storage | rusqlite (SQLite WAL), DashMap (memory), filesystem layout |
| Discovery | hickory-resolver (DNS), reqwest (RDAP), texting_robots (robots.txt) |
| Lua | mlua 0.10 (optional, `lua-hooks` feature) |
| SDK | Node 20+, CommonJS, zero runtime deps |

**Two binaries** ship from one source tree:
- `crawlex` — **full** build with HTTP impersonation + Chromium rendering + stealth shim + persistent queue
- `crawlex-mini` — **HTTP-only** worker, no Chromium dependency, same CLI surface (browser-only flags return `Error::RenderDisabled`)

---

## 📊 Versus the alternatives

| | crawlex | Playwright stealth | Puppeteer + plugins | curl-impersonate |
|---|:-:|:-:|:-:|:-:|
| TLS-perfect ClientHello | ✅ BoringSSL | ⚠️ relies on Chromium | ⚠️ relies on Chromium | ✅ |
| H2 pseudo-header order | ✅ patched h2 | ⚠️ Chromium default | ⚠️ Chromium default | ❌ |
| 29-section JS leak coverage | ✅ | ⚠️ partial | ⚠️ via plugins | ❌ no JS |
| Worker-scope stealth | ✅ auto-attach | ⚠️ manual | ⚠️ manual | ❌ |
| HTTP-only path (no browser) | ✅ `crawlex-mini` | ❌ | ❌ | ✅ |
| Persistent queue + resume | ✅ SQLite/Redis | ❌ external | ❌ external | ❌ |
| Discovery pipeline | ✅ 17 stages | ❌ | ❌ | ❌ |
| Streaming NDJSON events | ✅ versioned | ❌ | ❌ | ❌ |
| Rust embedding | ✅ | ❌ | ❌ | ⚠️ libcurl |
| Single binary | ✅ | ❌ | ❌ | ✅ |

---

## 📚 Documentation

- 🌐 **[forattini-dev.github.io/crawlex](https://forattini-dev.github.io/crawlex/)** — full docsify hub
- 🏗️ [Architecture overview](https://forattini-dev.github.io/crawlex/#/architecture/00-overview)
- 📖 [CLI reference](https://forattini-dev.github.io/crawlex/#/reference/cli)
- ⚙️ [Config JSON schema](https://forattini-dev.github.io/crawlex/#/reference/config)
- 📡 [NDJSON event envelope](https://forattini-dev.github.io/crawlex/#/reference/events)
- 🎯 [Guides](https://forattini-dev.github.io/crawlex/#/guides/) — HTTP-only, rendered sessions, persistent runs
- 🥷 [Stealth & proxies](https://forattini-dev.github.io/crawlex/#/features/proxy-stealth)

---

## 🤝 Contributing

```bash
git clone https://github.com/forattini-dev/crawlex
cd crawlex

# Unit tests + offline shim compliance
cargo test --lib                    # 386+ tests
cargo test --test fpjs_compliance   # 27 cases
cargo test --test tls_catalog_coverage --test tls_catalog_roundtrip

# SDK tests
pnpm test                           # 21 node:test cases

# Quality gates
cargo fmt --check
cargo clippy --all-features -- -D warnings
cargo publish --dry-run --locked

# Live integration tests (require system Chromium)
cargo test --all-features --test stealth_runtime_live -- --ignored
cargo test --all-features --test worker_shim_live -- --ignored
```

CI runs all of the above on every PR. Contributions welcome — issues, feature requests, and PRs all reviewed.

---

## 📄 License

Dual-licensed under **MIT OR Apache-2.0** at your option. SPDX: `MIT OR Apache-2.0`.

Third-party attribution: see [`NOTICE`](NOTICE).

---

<div align="center">

<sub>**Built for crawlers who refuse to be detected.**</sub>

[Docs](https://forattini-dev.github.io/crawlex/) · [Releases](https://github.com/forattini-dev/crawlex/releases) · [Issues](https://github.com/forattini-dev/crawlex/issues) · [Discussions](https://github.com/forattini-dev/crawlex/discussions)

</div>
