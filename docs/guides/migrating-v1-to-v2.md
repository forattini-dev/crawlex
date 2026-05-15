# Migrating crawlex v1 → v2

crawlex v2 ships the full scraping framework (parser, selectors, adaptive matching, Spider DSL, MCP server, shell, replay cache, ad-block, provider abstraction). The v1 transport-and-crawler surface continues to work in v2 — **nothing is removed in this release**. This document explains how to adopt the v2 surfaces incrementally and what we plan to deprecate.

## TL;DR

- Every v1 entrypoint (`crawl()`, `defineHooks(...)`, the legacy `crawlex` CLI flags) is still supported on `main`.
- v2 adds *new* entrypoints alongside them: `defineSpider({...})`, `spider.stream()`, `crawlex spider run`, `crawlex shell`, `crawlex mcp`, `crawlex from-curl`, `crawlex update-blocklist`.
- Recipes that worked on v1.x.y continue to work on v2.x.y. There are no breaking changes in this release.
- Future major version (v3) will drop the v1 entrypoints. Migrate when convenient.

## Mapping table

| v1 surface | v2 replacement | Status |
| --- | --- | --- |
| Node SDK `crawl(opts)` | `defineSpider({ startUrls, parse }).run()` | Both supported; `crawl()` deprecated. |
| `defineHooks(...)` | Continues to work; composes with `defineSpider`. | Stable. |
| CLI `crawlex run --seed ...` | `crawlex spider run --start-url ...` *or* the legacy form. | Both supported. |
| Per-URL state strings (free-form) | Canonical `Status` enum (`queued`/`completed`/`disallowed`/`skipped`/`errored`/`cancelled_*`). | New writes use the enum; legacy strings remain readable. |
| Event envelope `v: 1` | `v: 2` (adds optional `status` field). | Consumers should accept both versions during the transition. |
| `block_resources: [...]` (legacy untyped list) | `reject_resource_types: [...]` (typed CDP reject list). | Both supported; typed form is recommended. |
| Regex include/exclude | `Pattern::compile_auto` (auto-detects glob vs regex via `re:` prefix) | Glob is recommended for new code. |
| Raw rowid offsets in SDK results | Opaque base64 versioned `cursor` token + `paginate()` async iterator | Cursor is recommended for new code. |
| (none) — render-mode operator switch | `--render-mode auto\|always\|never` | New in v2. |
| (none) — conditional re-crawl | `cache_max_age_secs` + `modified_since` | New in v2. |
| (none) — Content-Signal robots | `crawl_purposes` + Content-Signal directive in `robots.rs` | New in v2. |
| (none) — job TTL | `job_max_runtime_secs` + `result_retention_secs` | New in v2. |
| (none) — adaptive matching | `.css(sel, { identifier })` / `.xpath(...)` with adaptive store | New in v2. |
| (none) — external CDP provider | `--external-cdp-url` + capability detection + calibration + fallback chain | New in v2. |

## New surfaces worth adopting now

### Spider DSL

```ts
import { defineSpider } from 'crawlex';

const spider = defineSpider({
  startUrls: ['https://example.com'],
  async *parse(res) {
    for (const a of res.css('a[href]')) {
      yield new Request(a.attr('href'));
    }
    yield { url: res.url, title: res.css('title::text').first() };
  },
});

for await (const item of spider.stream()) {
  console.log(item);
}
```

### Adaptive selectors

```ts
// First run saves the fingerprint under "price_label"; subsequent runs
// relocate the element if the CSS selector goes stale.
const price = res.css('.product-price', { identifier: 'price_label', threshold: 0.2 });
```

### External CDP provider

```bash
crawlex spider run \
  --start-url https://example.com \
  --browser-provider cdp \
  --external-cdp-url http://127.0.0.1:9222 \
  --external-cdp-session-mode isolated
```

`Auto` provider mode falls through to stock Chromium when the endpoint is unreachable.

## Event envelope contract bump

| Field | v1 | v2 |
| --- | --- | --- |
| `v` | `1` | `2` |
| `status` | (absent) | optional canonical `Status` |

Consumers that only read `kind` + `payload` need no changes. Consumers that care about per-URL terminal state should switch from parsing free-form strings to reading `envelope.status` when present.

See `docs/architecture/04-events-hooks-sdk.md` for the full schema.

## What we'll remove in v3

When v3 ships (no date — driven by v2 adoption telemetry, not the calendar), the following will be removed:

- Node SDK `crawl()` entrypoint.
- v1 hook signatures (the SDK has already migrated to `defineHooks` everywhere internally).
- The free-form per-URL status string write path (the canonical enum is already the only writer in v2).
- The untyped `block_resources` field.
- Raw rowid offset pagination on the SDK results endpoint.

`v3` will ship its own migration guide.

## Need help?

- Open an issue at https://github.com/forattini-dev/crawlex/issues and tag it `migration`.
- The `docs/comparisons/scrapling.md` parity matrix lists every shipped feature and its backing slice.
- `crawlex shell` is the fastest way to prototype v2 surfaces interactively.
