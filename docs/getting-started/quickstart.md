# Quick Start

## 1. Run a fast HTTP-first crawl

```bash
cargo run --release -- crawl \
  --seed https://example.com \
  --method spoof \
  --max-depth 3 \
  --queue sqlite --queue-path state/queue.db \
  --storage filesystem --storage-path out/http-run \
  --emit ndjson
```

This keeps the run cheap: no Chrome pool, persistent queue state and filesystem artifacts under `out/http-run`.

## 2. Escalate to rendering when needed

```bash
cargo run --release -- crawl \
  --seed https://example.com/login \
  --method auto \
  --max-concurrent-render 2 \
  --wait-strategy networkidle \
  --metrics \
  --screenshot \
  --storage sqlite --storage-path state/rendered.db \
  --emit ndjson \
  --policy deep
```

Use `auto` when you want HTTP-first behavior with render escalation. Use `render` when every job should go straight to Chrome.

## 3. Inspect the queue and export state

```bash
cargo run --release -- queue stats --queue-path state/queue.db
cargo run --release -- queue export --queue-path state/queue.db --out state/queue.ndjson
cargo run --release -- export-graph --storage-path state/rendered.db --out state/edges.ndjson
```

## 4. Check how the client fingerprints

```bash
cargo run --release -- inspect-fingerprint https://tls.peet.ws/api/clean
cargo run --release -- test-stealth
```

`inspect-fingerprint` prints the raw response details for one URL. `test-stealth` hits a small built-in target set useful during development.

## 5. Resume a persisted run

The dedicated `resume` command is not implemented yet. The current workflow is to launch another `crawl` against the same SQLite queue and storage paths:

```bash
cargo run --release -- crawl \
  --queue sqlite --queue-path state/queue.db \
  --storage sqlite --storage-path state/rendered.db \
  --emit ndjson
```

Pending rows left in `queue.db` will be picked up again.
