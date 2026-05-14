# Slice 1: Canonical status taxonomy (enum + SQLite + events + SDK filter) [AFK]

## Parent

#1

## What to build

Introduce a canonical status enum used everywhere a per-URL or per-job state is reported: in-process types, SQLite rows, NDJSON events, and the TS SDK. Surface it on the SDK results read path with an optional `status` query filter so retry tooling can pull only the slices it cares about.

Values:
- per-URL: `queued`, `completed`, `disallowed`, `skipped`, `errored`, `cancelled`
- per-job terminal: `completed`, `errored`, `cancelled_due_to_timeout`, `cancelled_due_to_limits`, `cancelled_by_user`

Existing free-form status strings stay readable on legacy rows; new writes use the enum. Migration is forward-only.

## Acceptance criteria

- [ ] `crawlex` (Rust) exposes a `Status` enum with the canonical variants and a `terminal_reason` enum for jobs
- [ ] SQLite migration adds a `status` column on per-URL rows with an index, and a `terminal_reason` column on job rows
- [ ] NDJSON event payloads carry the canonical status; event contract version bumped per `docs/architecture/04-events-hooks-sdk.md`
- [ ] SDK results endpoint accepts a `status` query parameter; mirrored TS string-literal union exported
- [ ] Integration test seeds rows in every status and asserts SDK round-trip + filter
- [ ] `docs/reference/events.md` and `docs/architecture/04-events-hooks-sdk.md` updated with the enum
- [ ] Defaults preserve today's behavior on existing recipes

## Blocked by

None - can start immediately

## Progress (AFK pass 1)

Implementation landed; **needs `cargo check --all-features` + `cargo test status_taxonomy`
run by a human before move to `issues/done/`** (sandboxed agent could not invoke cargo).

Changes:
- `src/status.rs`: `Status` (per-URL) + `TerminalReason` (per-job) enums with
  snake_case serde, `as_str()`, `FromStr`, `all()`, unit tests.
- `src/lib.rs`: `pub mod status` + `pub use Status, TerminalReason`.
- `src/storage/sqlite.rs`:
  - Migration: `ALTER TABLE pages ADD COLUMN crawl_status TEXT` +
    `CREATE INDEX idx_pages_crawl_status`; `ALTER TABLE crawl_stats ADD COLUMN
    terminal_reason TEXT`. Forward-only — legacy rows stay readable.
  - `Op::SetPageCrawlStatus` + writer-thread handler that `UPDATE pages SET
    crawl_status = ?` for an existing row.
  - `SqliteStorage::set_page_crawl_status(url, status)` and
    `list_pages_with_status(filter, limit)`.
  - Free fn `list_pages_with_status_blocking(path, filter, limit) ->
    Vec<PageStatusRow>` for the read-only SDK results path. `PageStatusRow`
    is `serde::Serialize` and ships `crawl_status: Option<String>`.
- `src/events/envelope.rs`: bumped `EVENT_ENVELOPE_VERSION` from 1 to 2,
  added optional `status: Option<Status>` field on `EventEnvelope`, added
  `with_status()` builder, bumped the fallback serialize-failure string.
- `src/events/mod.rs`: doc comment version bump.
- `src/cli/args.rs`: new `PagesVerb::List(PagesListArgs)` with
  `--storage-path`, `--status`, `--limit`.
- `src/cli/mod.rs`: dispatch + `cmd_pages_list` — parses `--status` against
  the canonical taxonomy, opens the SQLite file read-only, prints JSON.
- `sdk/index.d.ts`: `UrlStatus` and `TerminalReason` string-literal unions,
  `PageStatusRow` interface, `BaseEnvelope.v: 2`, optional
  `BaseEnvelope.status?: UrlStatus`.
- `docs/architecture/04-events-hooks-sdk.md` + `docs/reference/events.md`:
  document the bumped version, the new `status` envelope field, and the
  canonical taxonomy (per-URL + per-job).
- `tests/status_taxonomy.rs`: enum wire round-trip + integration test that
  seeds one `pages` row per `Status` variant, calls
  `list_pages_with_status_blocking` with each filter, asserts each filter
  matches exactly one row.

Open follow-ups (not in this slice's AC):
- Wire `set_page_crawl_status` into the crawler's actual lifecycle
  transitions (currently only invoked by the test). Existing emit sites for
  `job.started`/`job.failed`/`extract.completed` still ship `status = None`
  until a follow-up populates them.
- Persist `terminal_reason` to `crawl_stats` at run end — the column exists
  but nothing writes to it yet.

