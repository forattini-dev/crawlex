# Slice 24: MCP server `crawlex mcp` (rmcp) [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Standalone `crawlex mcp` binary built on the `rmcp` crate. Exposes MCP tools mirroring Scrapling's surface: `open_session` / `close_session` / `list_sessions`, `get` / `bulk_get` (HTTP), `fetch` (dynamic browser), stealth fetch, plus `css_query` / `xpath_query` that accept a URL or session_id and return extracted markdown/text rather than raw HTML — minimizing tokens sent to the LLM.

## Acceptance criteria

- [ ] `crawlex mcp` subcommand starts an MCP server over stdio
- [ ] Tools: `open_session`, `close_session`, `list_sessions`, `get`, `bulk_get`, `fetch`, `stealth_fetch`, `css_query`, `xpath_query`
- [ ] `css_query` and `xpath_query` return extracted text/markdown by default; raw HTML opt-in via param
- [ ] Sessions wired through the SessionManager from slice 16
- [ ] Stealth tools route through the existing antibot/reCAPTCHA stack
- [ ] Integration test: spin up server, call `get` and `css_query` via the rmcp client crate
- [ ] Documented in `docs/reference/mcp.md` (new file)

## Blocked by

- Slice 17 (sessions and spider runtime), Slice 10 (selectors)
