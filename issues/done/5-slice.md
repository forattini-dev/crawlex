# Slice 4: Conditional re-crawl knobs (cache_max_age_secs + modified_since) [AFK]

## Parent

#1

## What to build

Add two conditional re-crawl knobs that drive `cache_validator`: `cache_max_age_secs` (skip pages whose stored fetch timestamp is younger than N seconds) and `modified_since` (Unix timestamp; skip pages whose stored `Last-Modified` is older than the threshold). Decision happens before the network where stored metadata is sufficient. Skipped pages emit a structured event with a precise reason.

## Acceptance criteria

- [ ] New deep function `cache_validator::evaluate_freshness(metadata, max_age, modified_since) -> CacheValidationOutcome` with no network dependency
- [ ] Config + CLI knobs `cache_max_age_secs: Option<u64>` and `modified_since: Option<u64>`
- [ ] Crawler short-circuits the fetch when `evaluate_freshness` returns `Fresh`
- [ ] Skip events carry reason `fresh-by-max-age` or `unmodified-since`
- [ ] Table-driven unit tests over `(metadata, max_age, modified_since) → outcome` covering: missing metadata, expired max_age, fresh under max_age, stale Last-Modified, fresh Last-Modified, both knobs combined
- [ ] Integration test seeds a row, configures the knobs, asserts the page is skipped and the event reason is correct
- [ ] Defaults (`None` on both) preserve today's behavior

## Blocked by

None - can start immediately

## Status (2026-05-14)

Implementation complete in WIP (uncommitted). All ACs satisfied:

- `cache_validator::evaluate_freshness(meta, max_age, modified_since) -> CacheValidationOutcome` lands at src/cache_validator.rs:75 with no network deps.
- Config knobs in src/config.rs:278 (`max_age_secs`) and src/config.rs:283 (`modified_since`) on `CacheValidationConfig`; both default `None`.
- CLI knobs in src/cli/args.rs:350 (`--cache-max-age-secs`) and src/cli/args.rs:354 (`--modified-since`); wired into config in src/cli/mod.rs:2010 and src/cli/mod.rs:2082.
- Crawler short-circuit at src/crawler.rs:388 (`maybe_complete_fresh_cache_by_age`), invoking `evaluate_freshness` and emitting cache_validation event with reason.
- Reasons `fresh-by-max-age` and `unmodified-since` returned literally; `no-freshness-decision` is the fall-through.
- Table-driven unit tests at src/cache_validator.rs:341 cover all 9 cases (no knobs, fresh/expired max_age, missing/stale/fresh last-modified, both knobs combined, unparseable date).
- Integration test tests/cache_freshness.rs seeds via real `SqliteStorage::save_raw_response`, asserts `Fresh` + reason for each knob plus the no-knobs default.

Blocker for commit: harness denied every `cargo`/`git` Bash invocation this iteration, so `cargo test --all-features` and `cargo check` were not run. Next iteration must (1) run the feedback loops, (2) commit with the message decisions listed below, (3) `git mv issues/5-slice.md issues/done/5-slice.md`.

Suggested commit: `feat(cache): pre-network freshness skip via cache_max_age_secs + modified_since` — touches Cargo.toml, src/cache_validator.rs, src/cli/args.rs, src/cli/mod.rs, src/config.rs, src/crawler.rs, tests/cache_freshness.rs.

