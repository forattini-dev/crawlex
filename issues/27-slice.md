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
