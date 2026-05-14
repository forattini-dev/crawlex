# Storage and Outputs

## Backends

| Backend | Best for | Persistence |
| --- | --- | --- |
| `memory` | tests, embedded runs | none |
| `sqlite` | resumable crawls, graph export, metrics | durable |
| `filesystem` | artifact-heavy runs, simple file inspection | durable |

## What gets stored

Depending on the path taken through the run, storage can contain:

- raw HTTP bodies
- rendered HTML after JS execution
- content-addressed body/html blobs
- cache validators: `etag`, `last_modified` and `head_fingerprint`
- graph edges between pages
- host facts such as favicon hashes and cert metadata
- page metrics
- screenshots
- crawl attempt and resolution telemetry

## Cache-aware storage

SQLite stores enough page metadata for `--cache-validate` to skip repeated work:

- `etag`
- `last_modified`
- `head_fingerprint`
- `saved_at`

Freshness can be accepted by age (`--cache-max-age-secs`) or by comparing the current response validators/fingerprint. A fresh cache emits `decision.made` with `why="cache:fresh"` and completes the job without running the heavier extraction/render path.

```bash
crawlex crawl \
  --seed https://example.com \
  --queue sqlite --queue-path state/queue.db \
  --storage sqlite --storage-path state/crawl.db \
  --cache-validate \
  --cache-max-age-secs 86400
```

## Prefetch and scoring outputs

`--prefetch` is meant for discovery passes. It fetches or renders enough HTML to extract links, then skips expensive page analysis and rendered-page persistence. Combine it with `--best-first` and `--score-keyword` to shape the queue for a later full crawl.

```bash
crawlex crawl \
  --seed https://docs.example.com \
  --prefetch \
  --best-first \
  --score-keyword api
```

## Exports that work today

Use the explicit commands:

```bash
cargo run --release -- queue export --queue-path state/queue.db --out state/queue.ndjson
cargo run --release -- export-graph --storage-path state/crawl.db --out state/edges.ndjson
```

## Metrics endpoint

`--metrics-prometheus-port <port>` starts a minimal scrape endpoint that exposes counters such as:

- `crawlex_requests_http_total`
- `crawlex_requests_render_total`
- `crawlex_pages_saved_total`
- `crawlex_errors_total`
- `crawlex_discovered_urls_total`
- `crawlex_retries_total`
- `crawlex_robots_blocked_total`

## Important caveat

The `output.*` config fields and matching CLI flags exist, but the strongest supported artifact path today is still the selected storage backend plus the explicit export commands above.
