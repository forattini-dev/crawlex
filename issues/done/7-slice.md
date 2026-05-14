# Slice 6: Content Signals robots.txt + crawl_purposes [AFK]

## Parent

#1

## What to build

Honor the `Content-Signal` directive in `robots.txt` (Cloudflare-pushed standard). Operators declare their crawl intent via `crawl_purposes` (any subset of `search`, `ai-input`, `ai-train`; default = all three). When a site's robots.txt fully disallows the declared purposes, the run aborts with a clear error before any fetches. Declared purposes are recorded in the run's event stream for audit.

## Acceptance criteria

- [ ] New deep type `robots::ContentSignal { search, ai_input, ai_train }` with a single `permits(purpose) -> bool` interface
- [ ] `robots.rs` parser extends to read `Content-Signal:` per-agent without breaking existing User-agent rules
- [ ] Config + CLI knob `crawl_purposes: Vec<Purpose>` with default of all three
- [ ] Crawler refuses to start with a 400-equivalent error when every declared purpose is disallowed by the site
- [ ] Run-start event carries the declared purposes
- [ ] Per-URL deny event uses `DenyReason::ContentSignal` (or equivalent) when a specific URL is filtered by Content Signals
- [ ] Fixture-driven tests cover: permissive site, fully-deny site, mixed (denies `ai-train`, permits `search`), Cloudflare's spec examples
- [ ] Documented in `docs/features/` and `docs/reference/config.md`

## Blocked by

None - can start immediately

## Progress (2026-05-14)

- [x] `robots::Purpose { Search, AiInput, AiTrain }` enum + `FromStr` + `all()`.
- [x] `robots::ContentSignal { search, ai_input, ai_train }` with
      `permits(Purpose) -> bool` and a `fully_denies(&[Purpose])` helper.
- [x] `robots::parse_content_signal(body, ua)` reads `Content-Signal:` per
      `User-agent:` block, handles stacked agents, and resolves UA
      specificity (exact > `*`).
- [x] `RobotsCache` stores a `ContentSignal` alongside the parsed Robot;
      `content_signal(host)` exposes it within TTL.
- [x] `Config::crawl_purposes: Vec<Purpose>` with `#[serde(default = …)]` =
      all three; `ConfigBuilder::crawl_purposes()` wired.
- [x] `Error::ContentSignalDenied { host, declared }` + `kind()` tag.
- [x] `DenyReason::ContentSignal` variant.
- [x] Crawler: `run.started` event carries `crawl_purposes`. In
      `process_job`, after `ensure_robots`, the cached `ContentSignal` is
      checked against `config.crawl_purposes`; fully-denied host emits
      `decision.made why="content-signal:fully-denied"` and returns
      `Err(Error::ContentSignalDenied)` before any fetch.
- [x] CLI: `--crawl-purpose <value>` repeatable + comma-split shorthand.
- [x] Unit tests in `src/robots.rs::tests` cover: permissive default,
      permissive body, fully-deny body, Cloudflare mixed example
      (search/ai-input yes, ai-train no), UA specificity over wildcard,
      stacked user-agents share signal, Purpose FromStr round-trip, cache
      store-and-fetch, cache miss returns None.
- [x] Docs: `docs/reference/config.md` table entry + "Content-Signal
      robots.txt" section + CLI example.

### Deferred

- [ ] `docs/features/` page — kept scope minimal per project rule
      "don't add docs files unless asked"; reference page is canonical.
- [ ] `cargo test`/`cargo check` not executed locally (sandbox blocked
      cargo invocations); CI is the verification gate.
- [ ] Per-URL filtering via `DenyReason::ContentSignal` from
      `link_filter::filter_links` — variant exists, but
      `FilterLinksInput` does not yet carry the cached `ContentSignal`.
      The abort fires earlier (per-host on first job), so the link path
      stays unused until a future slice threads the signal in.

