# Events, Hooks and SDK

## NDJSON event bus

`crawlex crawl --emit ndjson` writes one JSON envelope per line. Each envelope carries:

- protocol version
- UTC timestamp
- event kind
- optional `run_id`
- optional `session_id`
- optional `url`
- optional short `why`
- event-specific `data`

Consumers should ignore unknown event kinds and treat the stream as forward-compatible.

## Event kinds

Stable names exposed today:

- `run.started`
- `run.completed`
- `session.created`
- `session.state_changed`
- `session.evicted`
- `job.started`
- `job.failed`
- `decision.made`
- `fetch.completed`
- `crawl.attempted`
- `crawl.resolved`
- `render.completed`
- `extract.completed`
- `artifact.saved`
- `proxy.scored`
- `robots.decision`
- `challenge.detected`
- `step.started`
- `step.completed`
- `vendor.telemetry_observed`
- `tech.fingerprint_detected`

Important payloads:

- `fetch.completed` carries final URL, status, body size, truncation flag and optional network timings.
- `render.completed` carries render status plus Web Vitals and SPA/PWA summary fields.
- `crawl.attempted` records one HTTP/render/fallback attempt.
- `crawl.resolved` records the final attempt ladder summary for a crawl id.
- `artifact.saved` carries the storage handle/path, MIME, size and sha256.

## Hooks

The in-process hook registry exposes interception points such as:

- before each request
- after DNS resolution
- after TLS handshake
- after first byte
- on response body
- after page load or idle
- on discovery
- on job start or end
- on robots decisions
- on errors

Lua hook scripts are optional and require a build with `--features lua-hooks`.

## JavaScript wrapper

The Node wrapper exports:

- `crawl(opts)` for NDJSON streaming
- `crawlAll(opts)` for small runs
- `defineHooks({...})` for typed JS/TS lifecycle hooks over the bridge protocol
- `ensureInstalled(opts)` for release asset download
- `binaryPath()` and `assetBaseName()`
- `runJson(args)` for JSON-returning subcommands

When hooks are provided to the SDK, the wrapper owns the `--hook-bridge` wiring and exchanges hook envelopes with the Rust process. Hook decisions match the Rust/Lua contract: `continue`, `skip`, `retry` or `abort`.
