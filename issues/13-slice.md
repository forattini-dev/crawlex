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
