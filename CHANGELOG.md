# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
