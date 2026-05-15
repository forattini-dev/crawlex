# Slice 25: v1 API removal + migration guide [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Remove v1 entrypoints (`crawl()`, v1 hook signatures) and publish a migration guide at `docs/guides/migrating-v1-to-v2.md` mapping every v1 API to its v2 equivalent. Note: was originally HITL because the final call on what to delete vs preserve as a thin shim requires human review, and the migration guide needs editorial pass.

## Acceptance criteria

- [ ] `crawl()` entrypoint removed from Rust core and Node SDK
- [ ] v1 hook signatures removed; `defineHooks` continues to work with v2 spider runtime
- [ ] `docs/guides/migrating-v1-to-v2.md` written, reviewed, and linked from README
- [ ] Migration guide covers: spider construction, hooks, events (envelope changes), CLI flags
- [ ] CHANGELOG entry for v2.0 marks breaking changes explicitly
- [ ] Existing tests using v1 APIs are removed or ported

## Blocked by

- Slice 17 (defineSpider must exist), Slice 18 (event contract changes)
