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
- `job.started`
- `job.failed`
- `decision.made`
- `fetch.completed`
- `render.completed`
- `extract.completed`
- `artifact.saved`
- `proxy.scored`
- `robots.decision`

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
