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

