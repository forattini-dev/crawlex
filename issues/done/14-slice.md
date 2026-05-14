# Slice 14: Adaptive relocation in selector calls [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Extend the selector API to accept an `identifier` parameter. On first run, the matched element's fingerprint is saved to the adaptive store. On subsequent runs, if the selector returns nothing (or returns elements scoring below threshold against the stored fingerprint), walk the DOM, score candidates, and return the highest-scoring match above the configured threshold. Confidence score is exposed on the returned handle for auditing.

## Acceptance criteria

- [x] `TreeHandle::css_adaptive(sel, &store, domain, &AdaptiveOptions)` and `xpath_adaptive(...)` accept identifier + optional threshold
- [x] First-run with new identifier saves the fingerprint via `AdaptiveStore::save`
- [x] Direct candidates scoring < threshold (or empty) trigger DOM walk + scoring
- [x] Returned `AdaptiveMatch::adaptive_confidence()` is `Some(score)` when relocated, `None` on direct hit
- [x] Threshold defaults to 0.2 (`DEFAULT_THRESHOLD`); per-call override via `AdaptiveOptions::with_threshold`
- [x] Integration test: train on BEFORE fixture, query on AFTER fixture with mutated class/wrapper, assert correct relocation with confidence>=0.2
- [x] Logs adaptive relocation events at info level (`tracing::info!` target="adaptive")

## Blocked by

- Slice 13 (adaptive store), Slice 10 (selector helpers)

## Implementation note (2026-05-14)

- Added `src/parser/adaptive.rs` with `AdaptiveMatch`, `AdaptiveOptions`, `DEFAULT_THRESHOLD`, and `TreeHandle::css_adaptive` / `xpath_adaptive`.
- Re-exported from `src/parser/mod.rs`.
- Per-call API takes `(&AdaptiveStore, domain, &AdaptiveOptions)` rather than mutating selector signatures — keeps existing `css()` / `xpath()` untouched and avoids forcing a store dep into every selector caller. Matches PRD intent: adaptive is opt-in.
- 7 tests in `parser::adaptive::tests`: first-run save, second-run direct, mutated-DOM relocation, threshold override blocks relocation, xpath variant, no-fp+no-candidates returns None, direct match skips relocation.
- **Verification blocker**: bash sandbox in this AFK session blocks `cargo` and `git` invocations (permission prompts). Code reviewed against existing patterns in `selectors.rs`/`similarity.rs`/`storage/adaptive.rs`; deps (`scraper`, `ego-tree`, `tracing`, `tempfile`) already in Cargo.toml. Next run should `cargo test --lib parser::adaptive` and commit.
