# crawlex

[![CI](https://github.com/forattini-dev/crawlex/actions/workflows/ci.yml/badge.svg)](https://github.com/forattini-dev/crawlex/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/crawlex.svg)](https://crates.io/crates/crawlex)
[![npm](https://img.shields.io/npm/v/crawlex.svg)](https://www.npmjs.com/package/crawlex)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Stealth crawler with **Chrome-perfect TLS / HTTP-2 fingerprint**, render pool,
hooks, and persistent queue. Written in Rust.

`crawlex` ships two binaries:

- **`crawlex`** — full build with HTTP impersonation, headless Chromium
  rendering, stealth shim, and persistent queue.
- **`crawlex-mini`** — HTTP-only worker (no Chromium dependency, same CLI).

It's designed to crawl real production targets that block typical headless
crawlers — Cloudflare, DataDome, PerimeterX, Akamai BMP — by matching the
TLS extension order, HTTP/2 SETTINGS frame, pseudo-header order, and JS
fingerprint surface of stock desktop Chrome.

---

## Install

### Cargo

```bash
cargo install crawlex
```

> ⚠️ The `[patch.crates-io]` H2 fork is **not** carried by `cargo install` —
> the binary you get from crates.io has TLS-perfect fingerprint but uses
> upstream `h2 = 0.4` pseudo-header order. For full Chrome-149 H2 match,
> use the GitHub Release binaries below or build from source.

### npm (Node wrapper)

```bash
pnpm add -g crawlex
# or: npm i -g crawlex
```

The npm package includes a `postinstall` step that downloads the prebuilt
binary for your platform from the GitHub Release.

### Pre-built binary

Linux x86_64, Linux aarch64, macOS x86_64, macOS aarch64, Windows x86_64
binaries (with checksums) are attached to each
[GitHub Release](https://github.com/forattini-dev/crawlex/releases).

### Build from source

```bash
git clone --recurse-submodules https://github.com/forattini-dev/crawlex
cd crawlex
cargo build --release --features cli,sqlite,cdp-backend
./target/release/crawlex --help
```

Requires Rust 1.91 or newer.

---

## Quickstart

```bash
# Crawl a single page (HTTP only, no browser).
crawlex pages https://example.com

# Crawl with browser rendering, stealth shim, persistent SQLite queue.
crawlex crawl https://example.com \
  --max-depth 3 \
  --concurrency 4 \
  --queue sqlite:///tmp/crawlex.db \
  --render

# Inspect the active TLS / HTTP-2 fingerprint that crawlex sends.
crawlex fingerprint

# Re-render an existing crawl and show the fingerprint stack used.
crawlex stealth probe https://nowsecure.nl
```

See [`docs/`](https://forattini-dev.github.io/crawlex/) for the full reference
(CLI flags, configuration, hooks, events, queue backends, proxy rotation,
identity bundles, render pool tuning).

---

## What's in the box

| Concern | What we cover |
|---|---|
| **TLS** | Chrome 149 ClientHello — extensions ordered like Chrome, BoringSSL ALPS, `permute_extensions` for ChaCha vs AES, GREASE values |
| **HTTP/2** | Pseudo-header order `(method, authority, scheme, path)` — Akamai BMP signature match. SETTINGS frame parameters in Chrome's order |
| **JS fingerprint** | 29-section stealth shim: navigator, chrome.\*, permissions, plugins, screen, timezone, battery, connection, matchMedia, WebGL (vendor / params / extensions / `isEnabled`), canvas (zero-preserving noise), AudioContext (FFT + offline render), `Function.prototype.toString`, WebGPU, `performance.memory`, sensors, iframe, window geometry, requestAnimationFrame throttle, `performance.now()` 100µs grain + jitter, sample-rate pinning, mediaDevices coherent counts, speechSynthesis voices per OS, font list, worker concurrency cap, TextMetrics (HarfBuzz analogue), WebRTC SDP / ICE / getStats scrub |
| **Worker scope** | Same shim auto-attached to dedicated / shared / service workers via CDP `Target.setAutoAttach` + `Runtime.evaluate` (Camoufox port) |
| **Render pool** | Headless Chromium pool with isolated user-data dirs, persona pinning, optional Lua hooks, request interception |
| **Queue** | In-memory, SQLite, or Redis persistent backends with DEDUPE + RATE-LIMIT + RETRY policies |
| **Proxy** | Rotator with health-checks, sticky sessions, public-interface-only WebRTC policy |
| **Discovery** | Sitemap, robots, certificate transparency, DNS, Wayback, security.txt, well-known, JS endpoint extraction, network probe |

---

## Real-world coverage

Last validated: see [`production-validation/summary.md`](production-validation/summary.md).

### Browser fingerprint catalog

`crawlex` ships a TLS fingerprint catalog covering the **last 30 Chrome
stable, 30 Chromium, and 20 Firefox** majors plus Edge and Safari.
Choose the persona via `--profile <browser>-<major>-<os>`:

```bash
crawlex pages run --seed https://example.com \
  --method spoof \
  --profile chrome-149-linux

crawlex pages run --seed https://example.com \
  --profile firefox-130-macos

crawlex pages run --seed https://example.com \
  --profile chromium-122-linux
```

The catalog is data-driven: curl-impersonate's MPL-2.0 vendored
signatures live at `references/curl-impersonate/tests/signatures/*.yaml`,
our own captures land at `src/impersonate/catalog/captured/*.yaml`, and
public mining oracles (tls.peet.ws + ja4db.com) populate
`src/impersonate/catalog/mined/*.json`. All are compiled into a static
registry by `build.rs` at compile time.

**Discover what's available:**

```bash
# List every fingerprint registered in the catalog
crawlex stealth catalog list

# Filter by browser family
crawlex stealth catalog list --filter chrome

# Show full ClientHello breakdown for a profile
crawlex stealth catalog show chrome-149-linux
crawlex stealth catalog show firefox-130-macos --json
```

**Era fallback:** when an exact `(browser, major, os)` tuple isn't yet
captured, the catalog falls back to the closest era's representative
profile and emits a `tracing::warn` so operators know an approximation
is in play. Chrome eras:

| Era | Majors    | Marker change                          |
|-----|-----------|----------------------------------------|
| E1  | 98-99     | Pre-permute_extensions                 |
| E2  | 100-103   | permute_extensions enabled             |
| E3a | 104-110   | post-quantum experimentation start     |
| E3b | 111-116   | curl-impersonate frontier              |
| E4  | 117-123   | X25519Kyber768Draft00                  |
| E5  | 124-131   | ALPS payload reformat                  |
| E6  | 132-141   | MLKEM768 (Kyber removed)               |
| E7  | 142+      | ECH wider deployment                   |

Firefox eras (NSS-based, separate connector path):

| Era  | Majors  | Marker change                           |
|------|---------|-----------------------------------------|
| ESR  | 91      | NSS 3.68 ESR baseline                   |
| F-A  | 92-95   | TLS 1.3 default, Kyber off              |
| F-B  | 96-100  | encrypted_client_hello staged           |
| F-C  | 101-108 | NSS 3.79 base                           |
| F-D  | 109-116 | session_ticket reordered                |
| FF-A | 117-119 | NSS 3.79+ stabilised                    |
| FF-B | 120-126 | KyberSlash mitigation                   |
| FF-C | 127-130 | TLS Encrypted ClientHello experimental  |

**Refreshing the catalog with new captures:**

```bash
# Pull JA3/JA4 hashes from public databases (validation oracles)
node scripts/mine-fingerprints.mjs

# Bulk-capture every browser version (downloads + headless launch + emit YAML)
bash scripts/sync-tls-catalog.sh

# Subset capture
CHROME_MAJORS="148 149" bash scripts/sync-tls-catalog.sh
```

### Captcha solving

`crawlex` ships **prevention-first** captcha handling — the policy
engine prefers avoiding challenges over solving them. When a challenge
does fire, four solver adapters are available:

| Adapter | Vendors | Configuration |
|---|---|---|
| `recaptcha-invisible` | reCAPTCHA v3 (vanilla) | none — server-side, in-house |
| `2captcha` | reCAPTCHA, hCaptcha, image | `CRAWLEX_SOLVER_2CAPTCHA_KEY` |
| `anticaptcha` | reCAPTCHA, hCaptcha, FunCaptcha | `CRAWLEX_SOLVER_ANTICAPTCHA_KEY` |
| `vlm` | hCaptcha, image puzzles | `CRAWLEX_SOLVER_VLM_PROVIDER` + key |

Pick via CLI:

```bash
crawlex pages run --seed https://example.com \
  --method render \
  --captcha-solver recaptcha-invisible

# Externals refuse to run until their API key env var is set —
# this is by design, no silent paid-API calls.
CRAWLEX_SOLVER_2CAPTCHA_KEY=abc123 crawlex pages run \
  --seed https://example.com --captcha-solver 2captcha
```

#### In-house reCAPTCHA v3 invisible solver

`recaptcha-invisible` is a port of
[`h4ckf0r0day/reCaptchaV3-Invisible-Solver`](https://github.com/h4ckf0r0day/reCaptchaV3-Invisible-Solver)
adapted to crawlex's identity stack. Key differences from the reference:

- **Identity coherence** — UA, UA-CH brands, screen, timezone, canvas
  hash, WebGL strings flow from the active `IdentityBundle` instead of
  hardcoded Chrome 136 / Windows defaults. Eliminates the cross-check
  failure mode the reference suffers when the persona disagrees with
  the synthesised `oz` payload.
- **Server-side only** — no headless browser launched per challenge.
  Pipeline: `api.js` → `anchor` → `reload` → regex out the token →
  return as `g-recaptcha-response`.
- **Empirical scoring 0.3-0.9** — same range the reference reports.
  Use as a fallback for sites where a real browser path isn't viable
  (rate limit, bandwidth budget). Production use should still prefer a
  real Chrome render via `--method render` for tougher detectors.

The solver lives at `src/antibot/recaptcha/`:

```
recaptcha/
├── proto.rs       # Minimal protobuf encoder (no prost dep)
├── utils.rs       # base36, cb / co / scramble_oz primitives
├── oz.rs          # Build the `oz` JSON payload
├── telemetry.rs   # Synthesise field-74 client blob (mouse, scroll, perf)
├── solver.rs      # 3-hop pipeline + token extraction
└── adapter.rs     # CaptchaSolver trait impl
```

Limited to `ChallengeVendor::Recaptcha`. **Not** supported:
reCAPTCHA Enterprise (anchor-with-action verification), hCaptcha,
Cloudflare Turnstile, DataDome, PerimeterX — those are different
protocols entirely and fall back to external adapters if configured.

### Tuning render timeouts

Heavy real-world targets (Cloudflare-fronted SPAs, ad-laden landing pages)
can exceed the default 30 s `Page.navigate` deadline. Two knobs:

```bash
crawlex pages run --seed https://www.example.com \
  --method render \
  --render-request-timeout-ms 60000 \
  --navigation-lifecycle domcontentloaded \
  --wait-strategy '{"NetworkIdle":{"idle_ms":1500}}'
```

- `--render-request-timeout-ms` (or `CRAWLEX_REQUEST_TIMEOUT_MS` env) — CDP
  command + navigation watcher deadline. Default 30000.
- `--navigation-lifecycle` (or `CRAWLEX_NAVIGATION_LIFECYCLE` env) — `load`
  (default) or `domcontentloaded`. The latter returns as soon as the parser
  finishes; useful when third-party trackers keep the `load` event
  perpetually pending.

### Known limitations

- **Cloudflare Turnstile**: Some challenge variants gate on solving an
  in-page CAPTCHA — crawlex does not solve CAPTCHAs.
- **`robots.txt`**: Parsed (RFC-compliant via `texting_robots`) but not yet
  auto-enforced inside the crawler loop. Set `--respect-robots` to opt in,
  or check manually via `crawlex robots https://...`.
- **`cargo install crawlex`**: Pseudo-header order patch is not carried
  (see Install note above). Use GitHub Release binaries or `cargo build`
  from source for the full H2 fingerprint.
- **HTTP/3 + QUIC**: Not yet supported. HTTP/2 over TLS only.

---

## Development

```bash
# Clone with submodules (vendored h2 fork lives at vendor/h2).
git clone --recurse-submodules https://github.com/forattini-dev/crawlex
cd crawlex

# Run unit tests + the offline shim compliance suite.
cargo test --lib
cargo test --test fpjs_compliance

# Live tests (require system Chromium):
cargo test --all-features --test stealth_runtime_live -- --ignored
cargo test --all-features --test worker_shim_live -- --ignored

# Format / lint:
cargo fmt
cargo clippy --all-features -- -D warnings
```

---

## License

Dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option. SPDX: `MIT OR Apache-2.0`.

Third-party attribution: see [`NOTICE`](NOTICE).
