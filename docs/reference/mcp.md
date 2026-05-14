# `crawlex mcp` — Model Context Protocol server

`crawlex mcp` starts a JSON-RPC 2.0 server over **stdio** that exposes
crawlex's stealth fetch stack and selector engine as MCP tools. LLM
hosts (Claude Desktop, custom agents, etc.) can call the tools to
fetch pages, manage isolated sessions, and extract content as text or
markdown — minimizing the tokens sent back to the model.

## Usage

```sh
crawlex mcp                 # serve on stdin/stdout
crawlex mcp --name agentx   # override the advertised server name
```

The server speaks newline-delimited JSON-RPC. Each request must be a
single JSON object on one line; responses are written one per line on
stdout. Notifications (requests without `id`) produce no response.

## Methods

| Method        | Result shape                                            |
|---------------|---------------------------------------------------------|
| `initialize`  | `{ protocolVersion, serverInfo, capabilities }`         |
| `tools/list`  | `{ tools: [{ name, description, inputSchema }] }`       |
| `tools/call`  | `{ content: [...], structuredContent, isError? }`       |
| `ping`        | `{}`                                                    |

## Tools

| Tool             | Purpose                                                                   |
|------------------|---------------------------------------------------------------------------|
| `open_session`   | Register an isolated session (`http`, `render`, or `stealth` backend).    |
| `close_session`  | Drop a session and its cached page.                                       |
| `list_sessions`  | Enumerate registered sessions.                                            |
| `get`            | Plain HTTP GET. Returns status, headers, body (capped at 64 KiB).         |
| `bulk_get`       | Sequential GET over many URLs. One result per URL.                        |
| `fetch`          | Dynamic browser (CDP render path) fetch.                                  |
| `stealth_fetch`  | Stealth-stack fetch (TLS/JA3/UA impersonation + antibot bypass).          |
| `css_query`      | CSS-select against a URL or session-cached page.                          |
| `xpath_query`    | XPath-select against a URL or session-cached page.                        |

### Query tools (`css_query`, `xpath_query`)

Both accept the same arguments:

```json
{
  "url": "https://example.com/",   // OR "session_id" must be set
  "session_id": "s1",              // optional; resolves to last fetched page
  "selector": "h1.title",
  "format": "text"                 // "text" (default) | "markdown" | "html"
}
```

**`text`** (default) is the lowest-token surface — just the
`textContent` of each match with whitespace collapsed.
**`markdown`** converts headings, `<a>`, `<strong>`/`<em>`, `<li>` and
paragraphs to Markdown; everything else is stripped. Use it when the
LLM needs structure but you want to save tokens vs raw HTML.
**`html`** returns each match's `outerHTML` unchanged.

### Session-scoped queries

`get` / `fetch` / `stealth_fetch` cache the most recent response per
`session_id` (when supplied). Subsequent `css_query` / `xpath_query`
calls can pass just `session_id` to re-query that cached page without
a new network round-trip.

## Example session

```text
> {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
< {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05",...}}

> {"jsonrpc":"2.0","id":2,"method":"tools/call",
   "params":{"name":"open_session","arguments":{"id":"s","backend":"stealth"}}}
< {"jsonrpc":"2.0","id":2,"result":{"structuredContent":{"id":"s","backend":"stealth"}}}

> {"jsonrpc":"2.0","id":3,"method":"tools/call",
   "params":{"name":"stealth_fetch","arguments":{"url":"https://example.com/","session_id":"s"}}}
< {"jsonrpc":"2.0","id":3,"result":{"structuredContent":{"status":200,...}}}

> {"jsonrpc":"2.0","id":4,"method":"tools/call",
   "params":{"name":"css_query","arguments":{"session_id":"s","selector":"h1","format":"markdown"}}}
< {"jsonrpc":"2.0","id":4,"result":{"structuredContent":{"matches":["# Example Domain"], ...}}}
```

## Notes

* The server is intentionally token-conservative: responses cap raw
  bodies at 64 KiB; chain `css_query` to extract only what you need.
* The `fetch` (dynamic browser) tool currently shares the impersonate
  client with `stealth_fetch` — the render-pool wiring lands alongside
  the spider runtime in a later slice.
* Errors during a tool call return `isError: true` plus a text message
  rather than a JSON-RPC error envelope, matching the MCP spec.
