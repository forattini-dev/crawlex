# Slice 8: Cursor pagination on results read path [AFK]

## Parent

#1

## What to build

Replace raw rowid offsets in the SDK results read path with an opaque base64-encoded cursor token. Token must remain valid across server restarts (no in-memory cursor state). Cursor composes cleanly with the `status` filter introduced in slice 1 so consumers can paginate through `errored` rows without scanning the full set.

## Acceptance criteria

- [ ] SDK results endpoint accepts `cursor: Option<String>` and returns the next cursor in the response
- [ ] Cursor encoding is opaque base64 over a versioned struct (no raw rowids leaked)
- [ ] Decoding tolerates older versions or returns a clear error for unknown versions
- [ ] Cursor + `status` filter work together (filter applied before cursor)
- [ ] Integration test paginates a 5k-row seeded SQLite store with `limit=100`, asserts all rows are visited exactly once and no duplication occurs across page boundaries
- [ ] Restart test: seed rows, request page 1, restart the SDK process, request page 2 with the same cursor, assert continuation works
- [ ] TS SDK exports the cursor type and a `paginate()` async iterator helper
- [ ] Documented in `docs/architecture/04-events-hooks-sdk.md`

## Blocked by

#2 (canonical status taxonomy — pagination ships best alongside the `status` filter)

## Remote

GitHub issue #9
