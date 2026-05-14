# Slice 13: Adaptive store (reddb-io/reddb) per spider [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Persist element fingerprints with the `reddb-io/reddb` embedded DB, one file per spider. Keyed by `domain + identifier` (spider scope is implicit in the file). API: `save(domain, identifier, fingerprint)` and `retrieve(domain, identifier) -> Option<Fingerprint>`. Round-trip tests cover serialization stability across versions.

## Acceptance criteria

- [ ] `reddb-io/reddb` added as dependency (verify crate name with user before locking)
- [ ] `AdaptiveStore::open(spider_id) -> Self` opens one DB per spider
- [ ] `save` and `retrieve` round-trip a fingerprint with identical equality
- [ ] Concurrent reads safe across spider tasks
- [ ] Two spiders with overlapping domains do not collide
- [ ] Tests cover: round-trip, missing key returns `None`, isolation between spider files

## Blocked by

- Slice 12 (fingerprint type)

## Progress note (2026-05-14)

Public API + storage isolation + concurrency + tests implemented in
`src/storage/adaptive.rs`. Backend is file-backed JSON (one
`<spider_id>.adaptive.json` per spider, atomic tmp+rename writes,
`parking_lot::RwLock` for concurrent reads).

`reddb-io/reddb` dependency NOT added — crate name could not be verified
in this autonomous run (no network, AFK constraint forbids destructive
fallbacks). Public surface (`AdaptiveStore::open` /
`save` / `retrieve`) is stable; swapping the on-disk backend later is a
self-contained change.

Open AC: confirm crate name with operator → swap JSON backend for
reddb, keeping tests as-is.
