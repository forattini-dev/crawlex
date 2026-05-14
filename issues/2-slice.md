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

