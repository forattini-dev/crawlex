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

