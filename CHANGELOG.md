# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — In-house reCAPTCHA v3 Invisible Solver

- **`src/antibot/recaptcha/`** (~1.4k LOC, 7 modules) — server-side
  port of `h4ckf0r0day/reCaptchaV3-Invisible-Solver` adapted to
  crawlex's `IdentityBundle` stack. UA, UA-CH brands, screen,
  timezone, canvas, WebGL flow from the active persona instead of
  hardcoded Chrome 136 Windows — fixes the cross-check failure mode
  of the upstream reference.
- **No external API needed** — `--captcha-solver recaptcha-invisible`
  routes via `RecaptchaInvisibleAdapter` (3-hop pipeline:
  `api.js` → `anchor` → `reload`, regex out the token, return as
  `g-recaptcha-response`).
- **Limited to vanilla reCAPTCHA v3** — Enterprise (anchor-with-action
  verification), hCaptcha, Turnstile, DataDome, PerimeterX fall back
  to external adapters (`2captcha`, `anticaptcha`, `vlm`).
- **40 inline unit tests** across the 7 module files +
  `tests/recaptcha_solver_smoke.rs` for module-boundary coverage:
  alias parsing, adapter dispatch, vendor refusal, sitekey requirement.
- **External adapters remain prevention-first** — `2captcha`,
  `anticaptcha`, `vlm` refuse to run until their respective API key
  env var is wired (`CRAWLEX_SOLVER_2CAPTCHA_KEY`, etc.). No silent
  paid-API calls.

### Added — TLS Fingerprint Catalog (Phase A)

- **Data-driven `Profile` enum** — replaces the previous closed
  `Chrome131Stable`/`Chrome132Stable`/`Chrome149Stable` (all of which
  emitted the same ClientHello) with `Profile::Chrome { major, os }`,
  `Profile::Chromium { major, os }`, `Profile::Firefox { major, os }`,
  `Profile::Edge { major, os }`, `Profile::Safari { major, os }`.
  Coverage: 30 last Chrome stable + 30 last Chromium + 20 last Firefox
  × OSes = **210 SKUs** mapped via era fallback.
- **Builder API** — `Profile::for_chrome(149).os(BrowserOs::Linux).build()`,
  `Profile::for_firefox(130).os(BrowserOs::MacOs).build()`, etc.
- **`Profile::FromStr` + `Display`** — round-trip via
  `chrome-149-linux` form. CLI `--profile` flag accepts any spec.
- **TLS catalog at `src/impersonate/catalog/`** — `TlsFingerprint`
  struct + Browser/BrowserOs/Channel enums + IANA→OpenSSL/BoringSSL
  name translation helpers (`render_cipher_list`,
  `render_curves_list`, `render_sigalgs_list`, `encode_alpn_wire`).
  21 curl-impersonate vendored YAMLs compiled in at build time;
  capture pipeline ready to add more.
- **Era fallback logic** — `era_for(browser, major, os)` maps any
  major to its closest captured representative, with `tracing::warn`
  per era so operators know which approximation is active.
  Chrome E1-E7 + Firefox ESR/F-A through FF-C.
- **Firefox NSS-style connector** — separate
  `src/impersonate/tls_firefox.rs` build path: no ALPS, no
  cert_compression by default, NSS-style cipher ordering.
- **CLI catalog browser** — `crawlex stealth catalog list [--filter
  chrome] [--json]` and `crawlex stealth catalog show <profile>
  [--json]` for inspecting and diffing fingerprints.
- **Capture pipeline scripts**:
  - `scripts/tls-canary.rs` — local TCP server captures raw ClientHello
  - `scripts/yaml-from-capture.mjs` — `.bin` → curl-impersonate YAML
  - `scripts/sync-tls-catalog.sh` — bulk download/capture
    Chrome/Chromium/Firefox 80 versions
  - `scripts/mine-fingerprints.mjs` — pull tls.peet.ws + ja4db.com
    for cross-validation oracles, with conflict detection
- **`build.rs`** — compile-time codegen ingests both vendored YAMLs
  and locally-captured/mined data.

### Changed

- `src/impersonate/tls.rs::build_connector` reads ciphers, curves,
  sigalgs, ALPN, ALPS, cert_compression from `profile.tls()` instead
  of three hardcoded `chrome_*` static functions (now removed).
  Firefox profiles dispatch to the dedicated NSS-style builder.

### Tests

- `tests/tls_catalog_coverage.rs` — 210 SKUs all resolve (no panic)
- `tests/tls_catalog_roundtrip.rs` — JA3 canonical, IANA→BoringSSL
  lookup tables, ALPS presence per browser family
- `tests/tls_live_match.rs` (`#[ignore]`) — handshake against
  tls.peet.ws and assert returned JA4 matches catalog

### Migration

The legacy `Profile::Chrome131Stable` / `Chrome132Stable` /
`Chrome149Stable` constants are kept as `#[doc(hidden)]` aliases for
backward compatibility. New code should use the typed builder API or
the `chrome-149-linux` string form.

CLI strings like `--profile chrome-131-stable` still work; new strings
like `--profile chrome-149-linux` are preferred.

## [1.0.0] - 2026-04-25

First stable release. The CLI surface, configuration schema, queue backend
selection, and event names are now covered by SemVer; breaking changes will
require a major bump.

### Added

- **Stealth shim — 29 sections of fingerprint coherence**
  - Navigator identity (UA, brands, full-version list, platform)
  - `window.chrome.{app,runtime,loadTimes,csi}` runtime stub
  - `navigator.permissions.query` aligned with `Notification.permission`
  - Plugins / mimeTypes (Chrome 114+ PDF cluster)
  - Screen geometry (1920×1080 desktop default, persona-overridable)
  - Timezone — `Intl.DateTimeFormat` ↔ `Date.prototype.getTimezoneOffset`
  - Battery / connection / `matchMedia` query coercion
  - WebGL — vendor, renderer, parameters, extensions list,
    `getShaderPrecisionFormat`, `getContextAttributes`, `isEnabled` defaults
  - Canvas 2D — deterministic per-channel **zero-preserving** noise
    (Camoufox port; passes CreepJS clear-canvas invariant)
  - AudioContext / OfflineAudioContext — `Analyser`, `startRendering`,
    **`AudioBuffer.getChannelData` + `copyFromChannel`** with seeded LCG
    + non-linear polynomial perturbation (Camoufox port)
  - `Function.prototype.toString` proxy keeps overrides looking native
  - `Notification.requestPermission` denied → default coercion
  - WebGPU adapter coherence
  - `performance.memory` desktop heap pinning
  - Sensors / Battery absence consistent with desktop
  - `HTMLIFrameElement.contentWindow` defensive ownership (no Proxy)
  - Window inner / outer geometry — scrollbar shape per persona
  - `requestAnimationFrame` 1 Hz throttle when document hidden
  - **`performance.now()` clamp at 100 µs (Chrome non-COI grain) + xorshift32
    sub-grain jitter**, replacing the previous 5 µs tell (Camoufox port)
  - `AudioContext.sampleRate` pinned per persona
  - **`navigator.mediaDevices.enumerateDevices` + `getUserMedia` with
    persona-driven mic / cam / speaker counts** (Camoufox port)
  - `speechSynthesis.getVoices` per-OS list
  - Font-list coherence
  - `chrome.runtime.id` / `sendMessage` extension-hint shape
  - Web Worker concurrency ceiling matched to `hardwareConcurrency`
  - **`CanvasRenderingContext2D.measureText` / `TextMetrics` 0.1 % seeded
    multiplicative jitter across 12 metric fields** with FNV-1a 32-bit
    hash for `(string, font)` determinism (Camoufox HarfBuzz analogue)
  - **WebRTC SDP / ICE / `getStats()` scrub** — strips `a=candidate` lines
    with private IPv4 (10/127/169.254/192.168/172.16-31) or IPv6 link-local
    (fc/fd/fe80/::1), filters `onicecandidate`, sanitizes `local-candidate`
    rows in `getStats()` (Camoufox port)

- **Worker auto-attach** (Camoufox port S3.1) — same persona shim runs in
  every dedicated / shared / service worker spawned by the page, via CDP
  `Target.setAutoAttach { flatten: true, waitForDebuggerOnStart: true }` +
  per-session `Runtime.evaluate` before `Runtime.runIfWaitingForDebugger`.
  DOM-only sections are stripped from the worker variant via marker
  comments in the shim source.

- **HTTP-layer impersonation**
  - TLS — Chrome 149 ClientHello: extensions in Chrome's order, BoringSSL
    ALPS, `permute_extensions` for ChaCha vs AES, GREASE values
  - HTTP/2 — pseudo-header order `(method, authority, scheme, path)` for
    Akamai BMP signature match (vendored `h2 = 0.4.13` fork)
  - Header sets per persona, JA3 / JA4 / Akamai H2 hashes shipped with the
    `crawlex fingerprint` subcommand

- **Render pool** — headless Chromium pool with isolated user-data dirs,
  per-browser persona pinning, optional Lua hooks, request interception,
  Chromium binary auto-fetch and validation.

- **Queue backends** — in-memory, SQLite (file-backed), Redis (with sticky
  sessions). DEDUPE + RATE-LIMIT + RETRY policies.

- **Discovery** — sitemap, robots, certificate transparency, DNS,
  Wayback, security.txt, well-known, PWA manifest, favicon, JS endpoint
  extraction, network probe, tech fingerprinting.

- **CLI** — `pages`, `crawl`, `fingerprint`, `graph`, `queue`, `sessions`,
  `session`, `telemetry`, `stealth` subcommands. Two binaries: `crawlex`
  (full) and `crawlex-mini` (HTTP-only, no Chromium dependency).

- **CI / release pipeline** — multi-platform binary builds (Linux x86_64
  + aarch64, macOS x86_64 + aarch64, Windows x86_64), checksums, GitHub
  Release + crates.io + npm publish from a `v*.*.*` tag.

### Notes

- `cargo install crawlex` produces a binary with TLS-perfect fingerprint
  but **not** the Chrome H2 pseudo-header order — the `[patch.crates-io]`
  redirect to `vendor/h2` does not propagate to crates.io consumers. Use
  the GitHub Release binaries or build from source for full H2 match.
  This is tracked for resolution in v1.1 (publishing the H2 fork as
  `h2-chromepatch` separately).
- `robots.txt` is parsed but not auto-enforced inside the crawler loop.
  Opt in via `--respect-robots` or check manually with `crawlex robots`.

[1.0.0]: https://github.com/forattini-dev/crawlex/releases/tag/v1.0.0
