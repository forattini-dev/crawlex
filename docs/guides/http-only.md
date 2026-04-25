# HTTP-Only Crawl

Use this pattern when you want scale and deterministic cost.

## Recommended command

```bash
cargo run --release -- crawl \
  --seed https://example.com \
  --seed https://blog.example.com \
  --method spoof \
  --max-depth 4 \
  --max-concurrent-http 500 \
  --queue sqlite --queue-path state/http-queue.db \
  --storage sqlite --storage-path state/http-crawl.db \
  --emit ndjson \
  --policy fast
```

## Why this shape

- `spoof` guarantees no Chrome pool
- SQLite gives restart safety
- `fast` avoids unnecessary render escalation if `auto` slips in later
- NDJSON keeps stdout integration-friendly

## Add cheap enrichment if needed

```bash
--crtsh --dns --wayback
```

## Avoid in this mode

- `--screenshot`
- `--metrics-vitals`
- `--method render`
- large action scripts
