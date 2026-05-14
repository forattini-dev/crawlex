# Events, Hooks and SDK

## NDJSON event bus

`crawlex crawl --emit ndjson` writes one JSON envelope per line. Each envelope carries:

- protocol version (currently `2` — bumped from `1` in slice 1 when the canonical `status` field was added)
- UTC timestamp
- event kind
- optional `run_id`
- optional `session_id`
- optional `url`
- optional short `why`
- optional canonical per-URL `status` (one of `queued`, `completed`, `disallowed`, `skipped`, `errored`, `cancelled`)
- event-specific `data`

Consumers should ignore unknown event kinds and treat the stream as forward-compatible.

### Canonical status taxonomy (slice 1)

The `status` field on the envelope and the `crawl_status` column on the SQLite `pages` table share one canonical, snake_case taxonomy:

- per-URL: `queued`, `completed`, `disallowed`, `skipped`, `errored`, `cancelled`
- per-job terminal (`crawl_stats.terminal_reason`): `completed`, `errored`, `cancelled_due_to_timeout`, `cancelled_due_to_limits`, `cancelled_by_user`

Legacy rows (written before the column existed) carry `crawl_status = NULL` and stay readable. New writes populate the column. The SDK results endpoint (`crawlex pages list --status <value>`) filters on this column.

### Cursor pagination (slice 8)

`crawlex pages list` accepts `--limit <N>` and `--cursor <token>`. The response shape is:

```json
{ "rows": [ /* PageStatusRow[] */ ], "next_cursor": "<opaque base64 token>" }
```

`next_cursor` is omitted on the final batch. Tokens are URL-safe base64 of a small versioned struct — opaque to consumers, never carrying the raw `rowid`. A decoder that doesn't understand the encoded `v` returns a focused `cursor version N not supported` error rather than silently returning wrong rows. Tokens survive process restarts: the read path opens a fresh read-only SQLite connection each call and never depends on in-memory server state.

Filter composition: a cursor is bound to the `--status` filter it was minted under. Replaying the same token under a different filter is rejected — the rowid ordering would still match, but the consumer's mental model would silently drop or duplicate rows. Iterate to completion under one filter at a time.

TS SDK:

```ts
import { paginatePages } from 'crawlex';
for await (const row of paginatePages({ storagePath: 'crawlex.db', status: 'errored', pageSize: 100 })) {
  // row: PageStatusRow
}
```

The iterator hides the cursor token from the caller and re-invokes the binary once per page until `next_cursor` is absent.

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
