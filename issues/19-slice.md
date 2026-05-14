# Slice 19: Dev-replay mode (dir + reddb backends) [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

A record-on-first-hit, replay-on-subsequent cache for spider development. Two backends, user-pickable: an on-disk directory of `{url-hash}.json+body` files, or the reddb store from slice 13. Activated by `--replay-dir <path>` or `--replay-db` CLI flags / equivalent Node SDK options. Replay short-circuits the network entirely.

## Acceptance criteria

- [ ] `replay` module exposes `record(request, response)` and `lookup(request) -> Option<Response>`
- [ ] Directory backend: each request keyed by deterministic hash of (method, url, body)
- [ ] reddb backend: stored in the same per-spider DB as adaptive fingerprints (different table/namespace)
- [ ] Cache miss in replay mode falls through to the network and records the result
- [ ] CLI flags `--replay-dir <path>` and `--replay-db` wired
- [ ] Integration test: first run records, second run replays without hitting the fixture server

## Blocked by

- Slice 17 (replay sits in the request path of the spider runtime)
