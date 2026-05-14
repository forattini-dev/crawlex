# Slice 17: Spider DSL — defineSpider({startUrls, parse}) [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Introduce the v2 spider runtime. Users call `defineSpider({ startUrls, parse: async function*(res) {...} })` (Node) or the Rust trait equivalent. The `parse` async generator yields either scraped items (objects) or new `Request` instances. The runtime drives the existing scheduler/queue/checkpoint, marshals `Request` and `Response`, and integrates per-domain throttle and optional robots.txt obedience. v1 `crawl()` continues to work until slice 25.

## Acceptance criteria

- [ ] `defineSpider({...})` exists in Node SDK; Rust equivalent exposed
- [ ] `parse` generator yields are demuxed into items vs new requests
- [ ] Existing scheduler, queue, and checkpoint mechanics drive the new runtime unchanged
- [ ] Per-domain throttle / download delay configurable per spider
- [ ] Optional `robotsTxtObey` flag honors `Disallow` and `Crawl-delay` via existing robots module
- [ ] Pause-on-Ctrl-C and resume-from-checkpoint work end-to-end
- [ ] Integration test runs a minimal spider against a local fixture server

## Blocked by

- Slice 16 (session_id routing for multi-session spiders)

## Progress (2026-05-14, ralph iteration)

Tracer-bullet landed across Rust + Node SDK. v1 `crawl()` unaffected.

**Rust** (`src/scraping/spider.rs` + re-exports in `mod.rs`):
- `Spider` trait with `start_urls`, `parse(&Response) -> Vec<ParseYield>`, optional `start_requests` override.
- `ParseYield::Item(serde_json::Value)` / `ParseYield::Request(Request)` — demux.
- `SpiderConfig { download_delay, robots_txt_obey, user_agent, max_items }`.
- `SpiderRunner` — in-memory frontier (FIFO + dedup by `method+url`), per-host `delay_for(host, now)` throttle clock, robots gate via existing `RobotsCache`, routes through `SessionManager` (slice 16).
- `Checkpoint { pending, seen, items_emitted }` — `serde` round-trip; `seed(spider, Some(checkpoint))` resumes.
- `Fetcher` trait — tests inject a `MapFetcher`; real HTTP/render dispatcher lands when engine bindings ship (slice 25).
- 8 unit tests: demux, dedup, max-items pause, resume from checkpoint, JSON round-trip, per-domain throttle, robots Disallow shorts circuit, robots-off lets traffic through.

**Node SDK** (`sdk/crawlex-sdk.js` + `index.d.ts`):
- `defineSpider({ startUrls, parse, downloadDelayMs, robotsTxtObey, userAgent, maxItems })` — validates + freezes a spec.
- `runSpider(spec, { fetcher, robotsCache, signal, resume })` — async generator. Yields items; new `Request` yields re-enter the frontier. `handle.checkpoint()` snapshots; `handle.isPaused()` flags pause vs drain.
- Checkpoint wire shape matches Rust (`pending`/`seen`/`items_emitted`, snake_case session_id) — JS-paused runs can resume in Rust once the binding lands.
- Default fetcher: `node:http`/`node:https` so recipes work today.
- Robots: minimal Disallow + Crawl-delay evaluator. Caller supplies `Map<host, body>`; out-of-band robots fetch deferred to dispatcher (slice 25).
- Tests (`sdk/test/spider.test.js`): 7 tests against a local `http.createServer` fixture covering demux, maxItems+resume, robots block, downloadDelayMs throttle, request dedup, AbortSignal pause.

**Deferred** (out of scope for slice 17, called out in PRD):
- Real engine binding (HTTP/render/stealth dispatcher) — slice 25 territory.
- Persistent checkpoint storage (SQLite/file) — runner returns the struct; persisting it is a glue concern.
- Out-of-band robots.txt fetch — RobotsCache populated externally for now.
- Per-host Crawl-delay floor surfaced into Rust `delay_for` (texting_robots exposes it; needs RobotsCache accessor).

**Status**: code complete; **feedback loops not executed** — `cargo test` / `node --test` blocked by sandbox in this iteration. Code self-reviewed; next iteration should run `cargo test scraping::spider --lib` and `pnpm test` and, if green, move this file to `issues/done/`.

