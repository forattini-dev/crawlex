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
- `--render-mode auto|always|never` — operator-level switch that wins
  over `--method`. `auto` (default) keeps today's behaviour:
  impersonate first, escalate to render via the policy engine when
  needed. `always` forces every seeded job onto the render path and
  bumps `max_concurrent_render` to at least one. `never` pins every
  job to the impersonate path, refuses any render escalation, and
  keeps the render pool from being instantiated.
- `--policy fast|balanced|deep|forensics`
- `--wait-strategy networkidle|load|domcontentloaded|fixed`
- `--wait-idle-ms <ms>`
- `--prefetch`
- `--best-first`
- `--score-keyword <term>`

### Queue and storage

- `--queue inmemory|sqlite`
- `--queue-path <path>`
- `--storage memory|sqlite|filesystem`
- `--storage-path <path>`
- `--cache-validate`
- `--cache-max-age-secs <seconds>`

### Render and browser

- `--max-concurrent-render <n>`
- `--chrome-path <path>`
- `--chrome-flag <flag>`
- `--external-cdp-url <url>`
- `--gpu-policy compat|stealth`
- `--flatten-shadow-dom`
- `--remove-overlays`
- `--remove-consent-popups`
- `--actions-file <json>`
- `--script-spec <path>`
- `--screenshot`
- `--screenshot-mode viewport|fullpage|element:<css>`
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

### Hooks and SDK bridge

- `--hook-script <lua-file>`
- `--hook-bridge stdio|fd:<n>`

### Anti-bot fallback

- `--fallback-fetch-command <cmd>`
- `--fallback-fetch-arg <arg>`
- `--fallback-fetch-timeout-ms <ms>`
- `--fallback-fetch-max-bytes <bytes>`

## Recent crawl-efficiency flags

`--cache-validate` asks the storage backend for prior page metadata and can skip full processing when the cached row is still fresh by `ETag`, `Last-Modified`, or `<head>` fingerprint. Add `--cache-max-age-secs 86400` to accept rows younger than one day without a validation probe.

`--prefetch` is a discovery-only pass. It still fetches or renders enough HTML to extract links and feed the queue, but it skips expensive page analysis and rendered-page persistence.

`--best-first` changes newly discovered URL priority. `--score-keyword` may be repeated to give matching paths/hosts/query strings an extra boost.

```bash
crawlex crawl \
  --seed https://docs.example.com \
  --queue sqlite --queue-path state/queue.db \
  --storage sqlite --storage-path state/crawl.db \
  --cache-validate \
  --prefetch \
  --best-first \
  --score-keyword api
```

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
