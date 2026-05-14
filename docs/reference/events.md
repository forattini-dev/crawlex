# NDJSON Events

## Envelope

Every event emitted by `--emit ndjson` uses the same outer shape:

```json
{
  "v": 3,
  "ts": "2026-04-22T17:53:19Z",
  "event": "decision.made",
  "run_id": 14538211,
  "session_id": "sess-001",
  "url": "https://example.com/login",
  "why": "render:js-challenge",
  "status": "queued",
  "data": {
    "policy_profile": "deep",
    "selected_method": "render"
  }
}
```

## Field semantics

| Field | Meaning |
| --- | --- |
| `v` | Event envelope version (currently `3`; was `2` before slice 18 added `item.scraped`) |
| `ts` | ISO-8601 UTC timestamp |
| `event` | Stable event kind name |
| `run_id` | Run-scoped identifier shared across the crawl |
| `session_id` | Optional browser or logical session identifier |
| `url` | Event-associated URL when available |
| `why` | Short structured reason, mostly for decisions and failures |
| `status` | Optional canonical per-URL status: `queued`, `completed`, `disallowed`, `skipped`, `errored`, `cancelled` |
| `data` | Event-specific JSON payload |

## Event kinds

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
- `item.scraped`

## High-signal payloads

`fetch.completed` includes final URL, status, byte count, truncation flag and optional DNS/TCP/TLS/TTFB/download timings. The `path` field is `"impersonate"` for the in-process HTTP spoof client and `"fallback"` for the external `fallback_fetch` command.

`render.completed` includes final URL, status, SPA/PWA flags and a compact Web Vitals summary when collection is enabled. The `path` field is the literal `"render"` so a stream consumer can disambiguate against `fetch.completed.data.path` without inspecting the envelope's `event` field.

`artifact.saved` includes kind, MIME, size, sha256 and a backend path/handle when available.

`crawl.attempted` is emitted for each HTTP spoof, render or fallback-fetch attempt. It carries attempt index, engine, status, latency, proxy, block classification and error fields.

`crawl.resolved` summarizes the whole crawl id: attempt count, whether fallback fetch was used, final engine and success boolean.

`item.scraped` (slice 18) carries `{ spider_id, identifier?, payload }`. Emitted once per item yielded by `Spider::parse`. The `payload` is the raw JSON value the recipe returned; `identifier` is an optional stable key (defaults to the item's `id` or `url` field when present). Consumers that just want the items can subscribe via `SpiderRunner::stream()` (Rust) / `runSpider(...).stream()` (Node) — slow consumers lag silently rather than blocking the producer.

`decision.made` also reports non-policy gates such as cache validation:

```json
{
  "event": "decision.made",
  "why": "cache:fresh",
  "data": {
    "phase": "http_response",
    "decision": "use_cache",
    "cache_status": "fresh",
    "reason": "etag matched",
    "http_status": 200
  }
}
```

## Consumer rules

- Ignore unknown event kinds.
- Do not assume every event has `url`, `why` or `session_id`.
- Treat `data` as schema-evolving.
- If you need operator-readable traces too, combine `--emit ndjson` with `--explain`.

## Practical integrations

- pipe stdout into a queue, log shipper or Node stream consumer
- correlate `decision.made` with `job.failed`
- trigger alerts from repeated `proxy.scored` degradation
- build external progress UIs keyed by `run_id`
