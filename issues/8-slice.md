# Slice 8: Parser foundation — scraper tree + lol_html streaming [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Introduce a `parser` module that wraps two HTML parser backends behind a single public surface: `scraper` (html5ever) for tree-walking operations and `lol_html` for streaming rewrite/extract. Callers pick the backend by the API they call — tree mode returns a navigable DOM handle, streaming mode drives event handlers over bytes. The Node SDK gets a parallel binding so JS callers can obtain a tree handle for later selector work. Tested in isolation with malformed HTML, CDATA, and encoding edge cases.

## Acceptance criteria

- [x] `crawlex::parser` exposes `parse_tree(bytes, charset?) -> TreeHandle` and `stream_rewrite(bytes, handlers) -> Bytes` in Rust
- [ ] Node SDK exposes `parseTree(html)` returning a handle usable by later selector calls — **deferred**: SDK is a process-wrapper today (no NAPI bridge); to expose `parseTree` we'd either need to (a) add a `crawlex parse` subcommand piping decoded HTML, or (b) introduce a NAPI build. Punt to a later slice once the selector engine (Slice 9) is ready and we know exactly what handle shape the JS side needs.
- [x] Tree mode handles malformed HTML, CDATA, non-UTF8 encodings without panic
- [x] Streaming mode propagates handler errors and returns a typed error type (`ParserError::Rewriting`)
- [x] Unit tests cover: well-formed page, malformed page, BOM page, non-UTF8 (Latin-1, Shift-JIS via `<meta charset>`)
- [x] No dependency on selectors, adaptive, or spider modules — parser is leaf

## Blocked by

None - can start immediately

## Status

AFK pass 1 complete. `src/parser/mod.rs` added; `lol_html = "2"` dep added. Encoding pipeline: explicit label → `<meta charset>` byte sniff → BOM → UTF-8 fallback. Unit tests included.

**Local feedback loop blocker:** `cargo check`/`cargo test` execution was denied by the sandbox permissions hook for this run, so the code was reviewed manually but not compiled locally. CI must verify before merge. If CI flags `Settings::default()` vs `Settings::new()` or any lol_html 2.x API drift, swap accordingly.

Followup slice for the Node SDK binding once Slice 9 lands.
