# Slice 7: Job TTL (max_runtime watchdog + result_retention reaper) [AFK]

## Parent

#1

## What to build

Configurable lifecycle bounds for jobs and their results. `job_max_runtime_secs` runs a watchdog that auto-cancels jobs exceeding the budget, marking the terminal state `cancelled_due_to_timeout`. Hitting `max_pages` (or other configured limits) marks `cancelled_due_to_limits`. `result_retention_secs` runs a background reaper that GCs job artifacts from SQLite after the window elapses.

## Acceptance criteria

- [ ] Config + CLI knobs `job_max_runtime_secs: Option<u64>` and `result_retention_secs: Option<u64>`
- [ ] Watchdog task cancels overrunning jobs and writes terminal reason `cancelled_due_to_timeout`
- [ ] Hitting `max_pages` writes terminal reason `cancelled_due_to_limits`
- [ ] Reaper task deletes job + per-URL rows whose `result_expires_at` has passed; SQLite migration adds `result_expires_at`
- [ ] Integration tests induce each terminal path (timeout, limits, user cancel) and assert the terminal reason on both job and per-URL rows
- [ ] Reaper test seeds expired + non-expired rows, runs the reaper, asserts only expired rows are gone
- [ ] Defaults (`None`) preserve today's behavior — no watchdog, no reaper

## Blocked by

#2 (canonical status taxonomy — needs `cancelled_due_to_timeout` / `cancelled_due_to_limits` enum values)

## Remote

GitHub issue #8

## Progress (AFK pass 1)

Storage-layer tracer landed; in-crawler watchdog/dispatcher wiring is the
next pass. **Sandbox blocked `cargo check` / `cargo test` for this run —
human or CI must verify before move to `issues/done/`.** Matches the
"cargo unavailable in sandbox" note on slice 8.

Changes:
- `src/config.rs`: new `job_max_runtime_secs: Option<u64>`,
  `result_retention_secs: Option<u64>`, and `max_pages: Option<u64>`
  fields with serde `default` and `Default` impl set to `None` so the
  legacy behaviour (no watchdog, no reaper, unbounded pages) is
  preserved on existing recipes.
- `src/cli/args.rs`: matching `--job-max-runtime-secs`,
  `--result-retention-secs`, `--max-pages` flags on `CrawlArgs`.
- `src/cli/mod.rs`: wires the three flags through both the
  CrawlArgs → Config build path and `apply_crawl_cli_overrides`.
- `src/storage/sqlite.rs`:
  - Migration: forward-only `ALTER TABLE pages ADD COLUMN
    result_expires_at INTEGER` + same on `crawl_stats` + indices.
  - New `ReapStats` struct (per-table delete counters).
  - `record_job_terminal_blocking(path, crawl_id, reason,
    retention_secs, now_unix)` — opens a fresh read-only-friendly
    connection, looks up the run's url, writes `terminal_reason` and
    (when retention is `Some`) stamps `result_expires_at` on both
    `crawl_stats` and the matching `pages` row. `retention = None`
    leaves the TTL columns NULL so the reaper ignores the row.
  - `reap_expired_blocking(path, now_unix) -> ReapStats` —
    `DELETE … WHERE result_expires_at IS NOT NULL AND <= ?1` on
    both tables, NULL-TTL rows are left alone.
- `tests/job_ttl.rs`: terminal-reason wire-string assertions,
  `record_job_terminal_blocking` happy path + None-retention path,
  reaper-selectivity test (expired vs fresh vs NULL/legacy), and a
  fresh-db no-op assertion locking down the default-preserves-behaviour
  acceptance bullet.

Open follow-ups (still in this slice's AC, deferred to pass 2):
- Crawler-side watchdog: `Crawler::run` needs to spawn a
  `tokio::time::sleep(job_max_runtime_secs)` task that flips an
  `Arc<AtomicBool>` consulted by the dispatch loop; on flip the run
  drains in-flight tasks, calls `record_job_terminal_blocking(…,
  CancelledDueToTimeout, …)`, and exits.
- `max_pages` enforcement: count `JobDisposition::Done` writes per
  run; on reaching the cap call `record_job_terminal_blocking(…,
  CancelledDueToLimits, …)`.
- User-cancel / Ctrl-C path: hook into the existing shutdown signal
  (if any) to record `CancelledByUser`.
- Reaper background task: spawn a `tokio::time::interval` task in
  `Crawler::run` when `result_retention_secs.is_some()` that calls
  `reap_expired_blocking` every few minutes.
- Integration tests inducing each terminal path end-to-end (timeout,
  limits, user cancel) — gated on the watchdog landing.
