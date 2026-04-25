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
