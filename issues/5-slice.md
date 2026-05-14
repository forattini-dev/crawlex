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

