# NDJSON Events

## Envelope

Every event emitted by `--emit ndjson` uses the same outer shape:

```json
{
  "v": 1,
  "ts": "2026-04-22T17:53:19Z",
  "event": "decision.made",
  "run_id": 14538211,
  "session_id": "sess-001",
  "url": "https://example.com/login",
  "why": "render:js-challenge",
  "data": {
    "policy_profile": "deep",
    "selected_method": "render"
  }
}
```

## Field semantics

| Field | Meaning |
| --- | --- |
| `v` | Event envelope version |
| `ts` | ISO-8601 UTC timestamp |
| `event` | Stable event kind name |
| `run_id` | Run-scoped identifier shared across the crawl |
| `session_id` | Optional browser or logical session identifier |
| `url` | Event-associated URL when available |
| `why` | Short structured reason, mostly for decisions and failures |
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

## High-signal payloads

`fetch.completed` includes final URL, status, byte count, truncation flag and optional DNS/TCP/TLS/TTFB/download timings.

`render.completed` includes final URL, status, SPA/PWA flags and a compact Web Vitals summary when collection is enabled.

`artifact.saved` includes kind, MIME, size, sha256 and a backend path/handle when available.

`crawl.attempted` is emitted for each HTTP spoof, render or fallback-fetch attempt. It carries attempt index, engine, status, latency, proxy, block classification and error fields.

`crawl.resolved` summarizes the whole crawl id: attempt count, whether fallback fetch was used, final engine and success boolean.

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
