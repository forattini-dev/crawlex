# Events, Hooks and SDK

## NDJSON event bus

`crawlex crawl --emit ndjson` writes one JSON envelope per line. Each envelope carries:

- protocol version
- UTC timestamp
- event kind
- optional `run_id`
- optional `session_id`
- optional `url`
- optional short `why`
- event-specific `data`

Consumers should ignore unknown event kinds and treat the stream as forward-compatible.

## Event kinds

Stable names exposed today:

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

## Hooks

The in-process hook registry exposes interception points such as:

- before each request
- after DNS resolution
- after TLS handshake
- after first byte
- on response body
- after page load or idle
- on discovery
- on job start or end
- on robots decisions
- on errors

Lua hook scripts are optional and require a build with `--features lua-hooks`.

## JavaScript wrapper

The Node wrapper exports:

- `crawl(opts)` for NDJSON streaming
- `crawlAll(opts)` for small runs
- `ensureInstalled(opts)` for release asset download
- `binaryPath()` and `assetBaseName()`
- `runJson(args)` for JSON-returning subcommands

The strongest integration surface today is the event stream from `crawl()`.
