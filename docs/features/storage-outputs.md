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
- graph edges between pages
- host facts such as favicon hashes and cert metadata
- page metrics
- screenshots

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
