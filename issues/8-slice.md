# Slice 8: Parser foundation — scraper tree + lol_html streaming [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Introduce a `parser` module that wraps two HTML parser backends behind a single public surface: `scraper` (html5ever) for tree-walking operations and `lol_html` for streaming rewrite/extract. Callers pick the backend by the API they call — tree mode returns a navigable DOM handle, streaming mode drives event handlers over bytes. The Node SDK gets a parallel binding so JS callers can obtain a tree handle for later selector work. Tested in isolation with malformed HTML, CDATA, and encoding edge cases.

## Acceptance criteria

- [ ] `crawlex::parser` exposes `parse_tree(bytes, charset?) -> TreeHandle` and `stream_rewrite(bytes, handlers) -> Bytes` in Rust
- [ ] Node SDK exposes `parseTree(html)` returning a handle usable by later selector calls
- [ ] Tree mode handles malformed HTML, CDATA, non-UTF8 encodings without panic
- [ ] Streaming mode propagates handler errors and returns a typed error type
- [ ] Unit tests cover: well-formed page, malformed page, gzip-decoded bytes, non-UTF8 (Latin-1, Shift-JIS) page
- [ ] No dependency on selectors, adaptive, or spider modules — parser is leaf

## Blocked by

None - can start immediately
