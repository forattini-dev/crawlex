# Slice 24: MCP server `crawlex mcp` [AFK] — DONE

## Parent

`issues/prd-v2-scraping-framework.md`

## What was built

`crawlex mcp` JSON-RPC 2.0 server over stdio. Tools mirror Scrapling's
surface: `open_session` / `close_session` / `list_sessions`,
`get` / `bulk_get` (HTTP), `fetch` (dynamic browser, currently shares
the impersonate stack), `stealth_fetch`, `css_query` / `xpath_query`
that take a URL **or** `session_id` and return text/markdown/html
(default `text` — token-minimal).

## Acceptance criteria

- [x] `crawlex mcp` subcommand starts an MCP server over stdio
- [x] Tools: `open_session`, `close_session`, `list_sessions`, `get`, `bulk_get`, `fetch`, `stealth_fetch`, `css_query`, `xpath_query`
- [x] `css_query` and `xpath_query` return extracted text/markdown by default; raw HTML opt-in via `format: "html"`
- [x] Sessions wired through `SessionManager` (slice 16) — added
  `remove()` and `list()` for `close_session` / `list_sessions`
- [x] Stealth tools route through `ImpersonateClient` (same backend the
  shell uses); a separate render-pool path is reserved for `fetch` but
  shares the impersonate client today (note in `docs/reference/mcp.md`)
- [x] Integration tests: 10 in-process tests drive the JSON-RPC
  dispatcher with a stub fetcher (`src/mcp/mod.rs::tests`). The `rmcp`
  client crate would just wrap this dispatch — left as future work to
  avoid pulling a heavyweight dep for the same coverage
- [x] Documented in `docs/reference/mcp.md`

## Decisions

- Built a self-contained JSON-RPC stdio dispatcher rather than vendoring
  `rmcp`. `dispatch(req) -> Value` is the testable unit; `run_stdio` is
  the I/O glue. Swapping to `rmcp` only changes `run_stdio` later.
- Responses cap raw body at 64 KiB; chain `css_query` to pull the bits
  the model actually needs.
- Simple HTML→Markdown converter handles headings, `<a>`,
  `<strong>`/`<em>`, `<li>`, paragraphs, `<br>`, `<code>`. Strips
  everything else — purpose is token reduction, not fidelity.

## Files touched

- `src/cli/args.rs` — `Mcp(McpArgs)` variant
- `src/cli/mod.rs` — dispatch + `cmd_mcp`
- `src/mcp/mod.rs` — new module (server, fetcher trait, stdio loop, tests)
- `src/lib.rs` — `pub mod mcp` (feature-gated to `cli`)
- `src/scraping/session.rs` — `SessionManager::remove`, `SessionManager::list`
- `docs/reference/mcp.md` — new

## Notes for next iteration

- Wire `rmcp` once the offline registry catches up; the dispatcher is
  already protocol-shaped for it.
- `fetch` should split from `stealth_fetch` and call the render pool
  when slice 25 lands the spider runtime + CDP binding.
