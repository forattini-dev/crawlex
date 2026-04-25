# CLI Reference

## Top-level commands

| Command | Purpose | Notes |
| --- | --- | --- |
| `crawl` | Run a crawl with the selected method, queue and storage | Main operator command |
| `resume` | Reserved resume entrypoint | Currently returns a "not yet implemented" error |
| `inspect-fingerprint <url>` | Print one response with TLS and ALPN details | Useful for fingerprint debugging |
| `test-stealth` | Hit built-in stealth test targets | Development-oriented |
| `queue stats` | Show SQLite queue counts by state | Requires `sqlite` feature |
| `queue purge` | Delete `done` and `failed` rows | Requires `sqlite` feature |
| `queue export` | Export queue rows to NDJSON | Requires `sqlite` feature |
| `export-graph` | Export SQLite graph edges to NDJSON | Reads the `edges` table from storage |

## Most important `crawl` flags

### Input and scope

- `--seed <url>`
- `--seeds-file <path>`
- `--max-depth <n>`
- `--same-host-only`
- `--include-subdomains`

### Execution mode

- `--method spoof|auto|render`
- `--policy fast|balanced|deep|forensics`
- `--wait-strategy networkidle|load|domcontentloaded|fixed`
- `--wait-idle-ms <ms>`

### Queue and storage

- `--queue inmemory|sqlite`
- `--queue-path <path>`
- `--storage memory|sqlite|filesystem`
- `--storage-path <path>`

### Render and browser

- `--max-concurrent-render <n>`
- `--chrome-path <path>`
- `--chrome-flag <flag>`
- `--actions-file <json>`
- `--screenshot`
- `--no-fetch-chromium`

### Proxy and rate control

- `--proxy <url>`
- `--proxy-file <path>`
- `--proxy-strategy round-robin|sequential|random|sticky-per-host`
- `--proxy-sticky-per-host`
- `--rate-per-host-rps <float>`

### Discovery enrichments

- `--crtsh`
- `--wayback`
- `--dns`
- `--peer-cert`
- `--rdap`
- `--no-robots-paths`
- `--no-well-known`
- `--no-pwa`
- `--no-favicon`
- `--follow-all-assets`

### Observability

- `--emit none|ndjson`
- `--explain`
- `--metrics`
- `--metrics-net`
- `--metrics-vitals`
- `--metrics-prometheus-port <port>`

## Config file mode

`--config <path>` loads a full `Config` JSON from disk. `--config -` reads it from stdin. Explicit CLI flags still override the values loaded from JSON.

```bash
cat config.json | cargo run --release -- crawl \
  --config - \
  --seed https://example.com \
  --emit ndjson
```

## Known CLI gaps

- `resume` is intentionally not wired yet.
- `--queue redis` returns a configuration error.
- several output-directory flags are present, but the actual persisted artifacts are owned by the storage backend and export subcommands.
