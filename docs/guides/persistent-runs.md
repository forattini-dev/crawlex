# Persistent Runs

This is the safest pattern for long-running collection jobs.

## Start with durable queue and storage

```bash
cargo run --release -- crawl \
  --seed https://example.com \
  --method auto \
  --queue sqlite --queue-path state/queue.db \
  --storage sqlite --storage-path state/crawl.db \
  --cache-validate \
  --emit ndjson \
  --explain
```

## Inspect progress

```bash
cargo run --release -- queue stats --queue-path state/queue.db
```

## Resume after interruption

The dedicated `resume` command is still blocked, so the current restart procedure is to run `crawl` again against the same database files:

```bash
cargo run --release -- crawl \
  --queue sqlite --queue-path state/queue.db \
  --storage sqlite --storage-path state/crawl.db \
  --emit ndjson
```

The SQLite queue backend reclaims any rows left in `in_flight`.

## Re-crawl cheaply

For repeated runs over the same target, enable cache validation. Crawlex will read prior SQLite page metadata and skip full processing when the cache row is fresh by age, `ETag`, `Last-Modified`, or `<head>` fingerprint.

```bash
cargo run --release -- crawl \
  --seed https://example.com \
  --method auto \
  --queue sqlite --queue-path state/queue.db \
  --storage sqlite --storage-path state/crawl.db \
  --cache-validate \
  --cache-max-age-secs 86400 \
  --emit ndjson
```

For a first pass that only builds the frontier, add `--prefetch --best-first` and optional `--score-keyword` terms. Run a later full crawl against the same queue/storage once the frontier is shaped.

## Export state for offline analysis

```bash
cargo run --release -- queue export --queue-path state/queue.db --out state/queue.ndjson
cargo run --release -- export-graph --storage-path state/crawl.db --out state/edges.ndjson
```

## Purge completed rows

```bash
cargo run --release -- queue purge --queue-path state/queue.db
```

Do this only after exporting what you still care about.
