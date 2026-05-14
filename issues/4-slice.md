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

## Progress (2026-05-14)

- [x] `crawlex::pattern::Glob` deep type added: `compile`, `matches`, `as_regex`
- [x] `crawlex::pattern::compile_pattern` helper auto-detects glob vs regex
      (any of `^$()[]{}|+\` triggers regex path; otherwise glob). Compilation
      happens once and yields an anchored `regex::Regex`, so the hot path is
      unchanged for the eventual `link_filter` wiring.
- [x] Table-driven unit tests in `src/pattern/mod.rs` cover: `*` not crossing
      `/`, `**` crossing `/`, exact match, leading `**/`, trailing `/**`,
      `?` semantics, regex metachars in glob input being literal,
      auto-detection picking each path, and the exclude-over-include
      precedence pattern.
- [x] `docs/reference/config.md` documents the grammar, auto-detection, and
      three regex → glob migration examples.

### Remaining for full acceptance

- [ ] `Config` does not yet expose `include_patterns` / `exclude_patterns`
      top-level fields — only the `link_filter` API takes `&[Regex]` today.
      Adding the config fields + serde-defaulted compile step + CLI flags
      belongs in the next iteration once the call site lands (the current
      `crawler::Crawler` does not yet invoke `extract::link_filter`).
- [ ] `DenyReason::IncludePattern` / `ExcludePattern` already exist on the
      filter side; no event-schema change needed once the fields ship.
- [ ] cargo test/check not executed locally this iteration (sandbox blocked
      cargo invocations); CI will be the verification gate.

