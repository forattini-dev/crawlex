<div align="center">

# 🕸️ crawlex

**Stealth crawler que parece Chrome real. TLS, HTTP/2, JS — tudo.**

Rust core • Node SDK • binários cross-platform

[![CI](https://github.com/forattini-dev/crawlex/actions/workflows/ci.yml/badge.svg)](https://github.com/forattini-dev/crawlex/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/crawlex.svg)](https://crates.io/crates/crawlex)
[![npm](https://img.shields.io/npm/v/crawlex.svg)](https://www.npmjs.com/package/crawlex)
[![docs](https://img.shields.io/badge/docs-docsify-success.svg)](https://forattini-dev.github.io/crawlex/)
[![license](https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-blue.svg)](#license)

```bash
pnpm add -g crawlex && crawlex pages run --seed https://example.com
```

</div>

---

## ⚡ Por que crawlex

Crawler convencional cai na primeira parede da Cloudflare. `crawlex` chega como **Chrome 149** chega:

| Camada | O que crawlex iguala ao Chrome real |
|---|---|
| 🔐 **TLS** | ClientHello idêntico — extension order, ALPS, GREASE, permute_extensions, X25519MLKEM768 |
| 🚦 **HTTP/2** | Pseudo-header order, SETTINGS frame, WINDOW_UPDATE pattern |
| 🎭 **JS fingerprint** | 29 seções de stealth shim — navigator, webgl, canvas, audio, perf.now, mediaDevices, fonts, WebRTC scrub |
| 🤖 **Comportamento** | Mouse jitter, scroll cadence, dwell time, idle drift — `motion::` profiles por persona |
| 📦 **Catálogo** | 30 Chrome × 30 Chromium × 20 Firefox × Edge × Safari fingerprints, era fallback automático |

→ Resultado: passa em [BrowserScan](https://browserscan.net), [CreepJS](https://abrahamjuliot.github.io/creepjs/), [Sannysoft Bot Detection](https://bot.sannysoft.com/), [tls.peet.ws](https://tls.peet.ws), [ja4db.com](https://ja4db.com).

---

## 🚀 Instalação

```bash
# npm — bundled binary download via postinstall
pnpm add -g crawlex

# Rust — full source build
cargo install crawlex

# Direct binary (Linux x86_64/arm64, macOS, Windows)
# https://github.com/forattini-dev/crawlex/releases/latest
```

> ⚠️ Crawls reais devem rodar **localmente**, não em CI. Datacenter IPs (GitHub Actions, etc) caem instantaneamente.

---

## 💻 Três jeitos de usar

### 1️⃣ CLI direto

```bash
# Stealth render com persona, sitemap discovery, NDJSON event stream
crawlex pages run \
  --seed https://target.com \
  --method render \
  --persona atlas \
  --max-depth 3 \
  --screenshot \
  --emit ndjson > events.ndjson
```

### 2️⃣ SDK Node/TypeScript

```ts
import { crawl, defineHooks } from 'crawlex';

const hooks = defineHooks({
  async onAfterFirstByte(ctx) {
    if (ctx.response_status === 429) return 'retry';
    return 'continue';
  },
  async onDiscovery(ctx) {
    return {
      decision: 'continue',
      patch: { capturedUrls: [...ctx.captured_urls, `${ctx.url}/sitemap.xml`] },
    };
  },
});

for await (const ev of crawl({
  seeds: ['https://target.com'],
  args: { method: 'render', persona: 'atlas', screenshot: true },
  hooks,
})) {
  if (!('event' in ev)) continue;
  if (ev.event === 'render.completed') console.log(`LCP=${ev.data.vitals.largest_contentful_paint_ms}`);
  if (ev.event === 'artifact.saved' && ev.data.kind === 'screenshot.full_page') {
    console.log('screenshot →', ev.data.path);
  }
}
```

### 3️⃣ Embedded Rust library

```rust
use crawlex::{Config, Crawler};
use crawlex::hooks::{HookDecision, HookRegistry};
use crawlex::queue::FetchMethod;

let hooks = HookRegistry::new();
hooks.on_after_first_byte(|ctx| Box::pin(async move {
    match ctx.response_status {
        Some(429) | Some(503) => Ok(HookDecision::Retry),
        _ => Ok(HookDecision::Continue),
    }
}));

let config = Config::builder().max_concurrent_http(8).build()?;
let crawler = Crawler::new(config)?.with_hooks(hooks);
crawler.seed_with(seeds, FetchMethod::HttpSpoof).await?;
crawler.run().await?;
```

→ Rodável: [`examples/embedded_with_hooks.rs`](examples/embedded_with_hooks.rs)

---

## 🎯 Features

<table>
<tr>
<td width="50%" valign="top">

### Stealth core
- 🔐 Chrome 149 TLS via BoringSSL fork
- 🚦 H2 pseudo-header order patch
- 🎭 29-section JS shim (canvas/webgl/audio/...)
- 🤖 Worker scope shim (dedicated/shared/SW)
- 📦 80+ browser fingerprints catalog
- 🌍 5 personas (tux/office/gamer/atlas/pixel)

### Discovery
- 🗺️ Sitemap, robots, /.well-known
- 🔎 crt.sh certificate transparency
- 🌐 DNS, RDAP, Wayback
- 📜 PWA manifest, service workers
- 🔬 Tech fingerprint (Wappalyzer-like)
- 🔌 JS endpoint extraction

</td>
<td width="50%" valign="top">

### Pipeline
- 🎯 Render pool (Chromium auto-fetch)
- 🔁 Persistent queue (SQLite/Redis)
- 🔄 Proxy rotator (health checks)
- 🛡️ Antibot policy engine
- 🔧 Captcha solvers (4 adapters)
- 📊 Web Vitals + per-fetch timings

### Integrations
- 📡 NDJSON stream (19 event kinds)
- 🪝 Hooks: Rust / JS / Lua
- 🗃️ Storage: filesystem / SQLite / memory
- 🔌 SDK TypeScript types
- 📦 npm + crates.io + GH binaries
- 📚 docsify docs at /docs

</td>
</tr>
</table>

---

## 📡 Stream de eventos

Toda corrida emite NDJSON estável no stdout (versionado, `v: 1`):

```jsonl
{"v":1,"event":"run.started","ts":"2026-04-26T...","run_id":42}
{"v":1,"event":"fetch.completed","url":"...","data":{"status":200,"ttfb_ms":142,"total_ms":280,"alpn":"h2","cipher":"TLS_AES_128_GCM_SHA256"}}
{"v":1,"event":"render.completed","data":{"final_url":"...","vitals":{"largest_contentful_paint_ms":920.1,"cumulative_layout_shift":0.03}}}
{"v":1,"event":"artifact.saved","data":{"kind":"screenshot.full_page","sha256":"...","path":"artifacts/sess_abc/...png"}}
{"v":1,"event":"challenge.detected","data":{"vendor":"cloudflare_turnstile","level":"widget_present"}}
```

19 event kinds cobrem o ciclo completo. Schema TypeScript completa em `index.d.ts`.

---

## 🪝 Hooks — 12 lifecycle points, 3 linguagens

```
before_each_request → after_dns → after_tls → after_first_byte → on_response_body
   → after_load → after_idle → on_discovery → on_job_start → on_job_end
   → on_error → on_robots_decision
```

| Linguagem | Como | Quando usar |
|---|---|---|
| **Rust** | `hooks.on_after_first_byte(\|ctx\| ...)` | Embedded library, latency crítica |
| **JS/TS** | `defineHooks({...})` via SDK | Production crawls, lógica de negócio |
| **Lua** | `--hook-script foo.lua` | Scripts ad-hoc, sem build step |

Decisões: `continue` / `skip` / `retry` / `abort`. Hooks podem mutar `ctx.captured_urls`, injetar URLs extras, escrever em `user_data` para downstream.

---

## 📚 Docs completas

- 🌐 **[forattini-dev.github.io/crawlex](https://forattini-dev.github.io/crawlex/)** — docsify hub
- 🏗️ [Architecture](https://forattini-dev.github.io/crawlex/#/architecture/00-overview) — pipeline, identidade, queue, eventos
- 📖 [CLI reference](https://forattini-dev.github.io/crawlex/#/reference/cli) — todas as flags
- ⚙️ [Config JSON](https://forattini-dev.github.io/crawlex/#/reference/config)
- 📡 [Event envelope](https://forattini-dev.github.io/crawlex/#/reference/events)
- 🎯 [Guias](https://forattini-dev.github.io/crawlex/#/guides/) — HTTP-only, rendered sessions, runs persistentes

---

## 🛠️ Stack

- **TLS**: BoringSSL fork via `boring-sys`
- **H2**: vendored `h2` com pseudo-header order patch (`vendor/h2`)
- **CDP**: chromiumoxide-derived, embedded com `cdp-backend` feature
- **Async**: tokio multi-thread
- **Storage**: rusqlite (sqlite), DashMap (memory)
- **Lua**: mlua 0.10 (opcional, feature `lua-hooks`)

Build full (`cargo install crawlex`) traz tudo. Build mini (`crawlex-mini`) é HTTP-only sem Chromium — perfeito pra workers leves.

---

## 🤝 Contribuindo

```bash
git clone https://github.com/forattini-dev/crawlex
cd crawlex
cargo test --lib                  # 386+ tests
cargo test --test fpjs_compliance # offline shim compliance
pnpm test                         # SDK node:test (21 cases)
```

Live tests (precisam Chromium):
```bash
cargo test --all-features --test stealth_runtime_live -- --ignored
```

CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo publish --dry-run`. PR-friendly.

---

## 📄 License

Dual: **MIT OR Apache-2.0**. SPDX `MIT OR Apache-2.0`. Você escolhe.

Third-party attribution em [`NOTICE`](NOTICE).

---

<div align="center">

**Built for crawlers who refuse to be detected.**

[Docs](https://forattini-dev.github.io/crawlex/) · [Issues](https://github.com/forattini-dev/crawlex/issues) · [Releases](https://github.com/forattini-dev/crawlex/releases)

</div>
