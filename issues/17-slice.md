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
