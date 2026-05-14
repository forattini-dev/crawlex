# Slice 3: Glob include/exclude patterns in link_filter [AFK]

## Parent

#1

## What to build

Accept glob-style include/exclude patterns alongside the existing regex path in `extract::link_filter`. Grammar: `*` matches any chars except `/`; `**` matches any chars including `/`. Glob compilation happens once at config-load time into the existing anchored regex so the hot path is unchanged. Exclude rules deterministically win over include rules.

## Acceptance criteria

- [ ] New deep type `pattern::Glob` exposing `compile(&str) -> Result<Glob>` and `matches(&Url) -> bool`
- [ ] Config + CLI accept glob patterns in `include_patterns` / `exclude_patterns`; legacy regex form still supported behind a separate field or auto-detected escape hatch
- [ ] When a URL matches both, exclude wins; verified by table-driven test
- [ ] `DenyReason::ExcludePattern` / `DenyReason::IncludePattern` emitted unchanged
- [ ] Table-driven unit tests cover: `*` not crossing `/`, `**` crossing `/`, exact match, leading `**/`, trailing `/**`, exclude-over-include precedence
- [ ] Documented in `docs/reference/config.md` with at least three migration examples from regex to glob

## Blocked by

None - can start immediately

## Progress note (2026-05-14, ralph)

Implementation landed but uncommitted — `cargo` and `git` were denied
in this autonomous session, so neither verification nor commit could
run. Files staged but not added:

- `src/extract/pattern.rs` (new) — `Glob::compile`, `Pattern::{glob,
  regex, compile_auto, is_match}`. `re:` prefix is the regex escape
  hatch; everything else is glob. `*` stops at `/`, `**` crosses `/`,
  `?` is one non-`/`. Unit tests inline.
- `src/extract/mod.rs` — exports `pattern`.
- `src/extract/link_filter.rs` — `FilterLinksInput.excludes/includes`
  changed from `&[Regex]` to `&[Pattern]`; dispatch calls
  `p.is_match(...)`. Exclude-over-include precedence unchanged
  (already enforced by ordering in `filter_links`).
- `tests/link_filter.rs` — existing case updated to wrap regex via
  `Pattern::regex(...)`.
- `tests/glob_pattern.rs` (new) — table-driven grammar tests, plus
  exclude-wins-over-include and `compile_auto` mixed glob+regex.
- `docs/reference/config.md` — new "Include / exclude patterns"
  section with three migration examples.

Next iteration: run `cargo test --all-features` + `cargo check
--all-targets`, then commit and move this file to `issues/done/`.
CLI/config wiring (top-level `Config.include_patterns` /
`exclude_patterns`) is still TODO — no `Config` field exists yet,
so the slice's "Config + CLI accept glob patterns" bullet is partial:
the runtime surface is ready, only the deserialize entry point is
missing.

