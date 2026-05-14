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

## Progress (AFK pass)

Implementation landed; `cargo check` / `cargo test` blocked by sandbox
(cargo not on the approved command list). Next operator must run:

    cargo check --all-targets --all-features
    cargo test --test cursor_pagination --all-features
    cargo test storage::cursor --all-features

Changes:
- `src/storage/cursor.rs`: `PageCursor { v, after_rowid, status }`,
  URL-safe base64 of versioned JSON. `CURSOR_VERSION = 1`. Decode
  rejects v=0, v>CURSOR_VERSION, malformed base64, malformed JSON,
  and missing `v`. Unit tests cover roundtrip (with/without status),
  raw-rowid leak guard, version tolerance/rejection, malformed inputs.
- `src/storage/mod.rs`: `pub mod cursor` (gated behind `feature = sqlite`).
- `src/storage/sqlite.rs`:
  - `PageList { rows, next_cursor: Option<String> }` —
    `skip_serializing_if = "Option::is_none"` so terminal pages omit
    the field.
  - `list_pages_with_status_paged_blocking(path, filter, limit, cursor)`:
    opens read-only, orders by `rowid ASC`, fetches `limit + 1` to
    detect overflow without a COUNT, mints `next_cursor` only on
    overflow. Decodes the inbound cursor and enforces that its
    `status` matches the request filter (mixing filters mid-iteration
    would silently drop or duplicate rows). `limit == 0` ⇒ single
    unbounded batch with no `next_cursor`.
- `src/cli/args.rs`: `PagesListArgs` gains `--cursor <token>` and a
  no-op `--json` (so `runJson` from the SDK can append `--json`
  unconditionally).
- `src/cli/mod.rs`: `cmd_pages_list` now calls the paged function and
  emits `{ rows, next_cursor? }`.
- `tests/cursor_pagination.rs`: integration suite — 5k rows × limit=100
  visit-exactly-once + no dupes across page boundaries; 250 rows split
  across a simulated restart (page1 → drop → page2 via cursor → page3);
  cursor/filter mismatch is a hard error; `limit = 0` returns no cursor.
  Seeds bypass the writer thread by opening rusqlite directly against a
  minimal `pages` schema matching the columns the read path selects.
- `sdk/index.d.ts`: `PageCursor` (opaque string), `PageListResponse`,
  `PaginatePagesOptions`, `paginatePages()` async iterator helper.
- `sdk/crawlex-sdk.js`: `paginatePages()` — re-invokes the native
  binary once per page via `runJson` and yields rows.
- `docs/architecture/04-events-hooks-sdk.md`: slice 8 section — wire
  shape, opacity/version rules, restart guarantee, filter-binding
  rule, TS SDK usage example.

### Notes

- Cursor stability: SQLite gives implicit `rowid` to any table without
  `WITHOUT ROWID`. `pages` has no such clause, so `rowid` is stable,
  monotone-on-insert, and survives restarts. Ordering by `rowid ASC`
  yields a deterministic page sequence; the cursor wraps the last
  rowid the caller saw rather than exposing it.
- Slice 1's `list_pages_with_status_blocking` (non-paged) is preserved
  for `tests/status_taxonomy.rs` — slice 8 ships alongside it rather
  than replacing it.
