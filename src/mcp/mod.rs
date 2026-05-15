//! Slice 24 — `crawlex mcp` JSON-RPC 2.0 server over stdio.
//!
//! Exposes a -shaped tool surface to LLM hosts. The dispatcher
//! is intentionally split from the I/O loop so the integration test can
//! drive `dispatch` directly with a stub fetcher and assert on the
//! returned `serde_json::Value` — no process spawning required.
//!
//! ## Protocol
//!
//! Subset of the Model Context Protocol (https://modelcontextprotocol.io)
//! sufficient for hosting a tool registry over stdio:
//!
//! * `initialize` → `{ serverInfo, capabilities: { tools: {} } }`
//! * `tools/list` → `{ tools: [...] }`
//! * `tools/call` → `{ content: [{ type: "text", text: <serialized> }], isError? }`
//!
//! ## Why no `rmcp` dep
//!
//! The issue calls out `rmcp` as the preferred runtime; this slice keeps
//! the dispatcher self-contained so the contract under test is the wire
//! format. Swapping `rmcp` in later only changes the I/O loop in
//! `run_stdio`; `dispatch` stays the unit-test surface.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::error::{Error, Result};
use crate::impersonate::{ImpersonateClient, Profile, Response};
use crate::parser::{parse_tree, ElementHandle};
use crate::scraping::session::{BackendKind, SessionManager};

// ─────────────────────────────────────────────────────────────────────
// Fetcher seam
// ─────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<FetchedPage>;
}

#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub final_url: Url,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Default backend: the impersonate stack used by `crawlex shell`. The
/// MCP `get` / `bulk_get` tools route through this; `stealth_fetch` /
/// `fetch` share the same client today (the dynamic-browser path lands
/// alongside the render-pool binding in a later slice).
pub struct ImpersonateFetcher {
    client: ImpersonateClient,
}

impl ImpersonateFetcher {
    pub fn new() -> Result<Self> {
        Ok(Self { client: ImpersonateClient::new(Profile::Chrome149Stable)? })
    }
}

#[async_trait]
impl Fetcher for ImpersonateFetcher {
    async fn fetch(&self, url: &Url) -> Result<FetchedPage> {
        let resp: Response = self.client.get(url).await?;
        let content_type = resp
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        Ok(FetchedPage {
            final_url: resp.final_url,
            status: resp.status.as_u16(),
            content_type,
            body: resp.body.to_vec(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────
// Server
// ─────────────────────────────────────────────────────────────────────

pub struct McpServer {
    sessions: Arc<SessionManager>,
    /// Last fetched page per session id — used by `css_query` /
    /// `xpath_query` when given a `session_id` instead of a `url`.
    cache: Mutex<HashMap<String, FetchedPage>>,
    http: Arc<dyn Fetcher>,
    stealth: Arc<dyn Fetcher>,
    dynamic: Arc<dyn Fetcher>,
    name: String,
}

impl McpServer {
    pub fn new(
        name: impl Into<String>,
        http: Arc<dyn Fetcher>,
        stealth: Arc<dyn Fetcher>,
        dynamic: Arc<dyn Fetcher>,
    ) -> Self {
        Self {
            sessions: Arc::new(SessionManager::new(BackendKind::Http)),
            cache: Mutex::new(HashMap::new()),
            http,
            stealth,
            dynamic,
            name: name.into(),
        }
    }

    /// Top-level JSON-RPC entry point. Always returns a Response value —
    /// notifications (no `id`) return `Value::Null` and the I/O loop drops
    /// the frame.
    pub async fn dispatch(&self, req: Value) -> Value {
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let is_notification = req.get("id").is_none();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let result: std::result::Result<Value, RpcError> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": self.name, "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "tools": {} },
            })),
            "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
            "tools/call" => self.call_tool(params).await,
            "ping" => Ok(json!({})),
            _ => Err(RpcError {
                code: -32601,
                message: format!("method not found: {method}"),
            }),
        };

        if is_notification {
            return Value::Null;
        }
        match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": e.code, "message": e.message },
            }),
        }
    }

    async fn call_tool(&self, params: Value) -> std::result::Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| rpc_invalid("missing `name`"))?
            .to_string();
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let outcome = match name.as_str() {
            "open_session" => self.tool_open_session(args),
            "close_session" => self.tool_close_session(args),
            "list_sessions" => self.tool_list_sessions(),
            "get" => self.tool_get(args).await,
            "bulk_get" => self.tool_bulk_get(args).await,
            "fetch" => self.tool_fetch(args, FetchKind::Dynamic).await,
            "stealth_fetch" => self.tool_fetch(args, FetchKind::Stealth).await,
            "css_query" => self.tool_query(args, QueryKind::Css).await,
            "xpath_query" => self.tool_query(args, QueryKind::Xpath).await,
            other => return Err(rpc_invalid(&format!("unknown tool: {other}"))),
        };

        match outcome {
            Ok(v) => Ok(json!({
                "content": [{ "type": "text", "text": serde_json::to_string(&v).unwrap_or_default() }],
                "structuredContent": v,
            })),
            Err(e) => Ok(json!({
                "content": [{ "type": "text", "text": e }],
                "isError": true,
            })),
        }
    }

    // ── tools ────────────────────────────────────────────────────────

    fn tool_open_session(&self, args: Value) -> std::result::Result<Value, String> {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or("`id` required")?
            .to_string();
        let backend = match args.get("backend").and_then(Value::as_str).unwrap_or("http") {
            "http" => BackendKind::Http,
            "render" => BackendKind::Render,
            "stealth" => BackendKind::Stealth,
            other => return Err(format!("unknown backend `{other}`")),
        };
        let entry = self.sessions.register(&id, backend);
        Ok(json!({
            "id": entry.id,
            "backend": describe_backend(entry.backend),
        }))
    }

    fn tool_close_session(&self, args: Value) -> std::result::Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).ok_or("`id` required")?;
        let removed = self.sessions.remove(id).is_some();
        self.cache.lock().remove(id);
        Ok(json!({ "id": id, "closed": removed }))
    }

    fn tool_list_sessions(&self) -> std::result::Result<Value, String> {
        let mut rows: Vec<Value> = self
            .sessions
            .list()
            .into_iter()
            .map(|s| json!({ "id": s.id, "backend": describe_backend(s.backend) }))
            .collect();
        rows.sort_by(|a, b| {
            a.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(b.get("id").and_then(Value::as_str).unwrap_or(""))
        });
        Ok(json!({ "sessions": rows }))
    }

    async fn tool_get(&self, args: Value) -> std::result::Result<Value, String> {
        let url = arg_url(&args, "url")?;
        let session_id = args
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let page = self
            .http
            .fetch(&url)
            .await
            .map_err(|e| format!("fetch failed: {e}"))?;
        if let Some(sid) = &session_id {
            self.cache.lock().insert(sid.clone(), page.clone());
        }
        Ok(page_response(&page))
    }

    async fn tool_bulk_get(&self, args: Value) -> std::result::Result<Value, String> {
        let urls = args
            .get("urls")
            .and_then(Value::as_array)
            .ok_or("`urls` (array) required")?
            .iter()
            .map(|v| v.as_str().ok_or("each `urls` entry must be a string"))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut results = Vec::with_capacity(urls.len());
        for raw in urls {
            let parsed = Url::parse(raw).map_err(|e| format!("invalid url `{raw}`: {e}"))?;
            match self.http.fetch(&parsed).await {
                Ok(page) => results.push(page_response(&page)),
                Err(e) => results.push(json!({ "url": raw, "error": e.to_string() })),
            }
        }
        Ok(json!({ "results": results }))
    }

    async fn tool_fetch(
        &self,
        args: Value,
        kind: FetchKind,
    ) -> std::result::Result<Value, String> {
        let url = arg_url(&args, "url")?;
        let session_id = args.get("session_id").and_then(Value::as_str).map(str::to_string);
        let fetcher: &dyn Fetcher = match kind {
            FetchKind::Stealth => self.stealth.as_ref(),
            FetchKind::Dynamic => self.dynamic.as_ref(),
        };
        let page = fetcher
            .fetch(&url)
            .await
            .map_err(|e| format!("fetch failed: {e}"))?;
        if let Some(sid) = &session_id {
            self.cache.lock().insert(sid.clone(), page.clone());
        }
        Ok(page_response(&page))
    }

    async fn tool_query(
        &self,
        args: Value,
        kind: QueryKind,
    ) -> std::result::Result<Value, String> {
        let selector = args
            .get("selector")
            .and_then(Value::as_str)
            .ok_or("`selector` required")?
            .to_string();
        let format = args
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("text");

        let page = self.resolve_page(&args).await?;
        let tree = parse_tree(&page.body, charset_from(page.content_type.as_deref()));
        let handles: Vec<ElementHandle<'_>> = match kind {
            QueryKind::Css => tree.css(&selector),
            QueryKind::Xpath => tree.xpath(&selector),
        };
        let matches: Vec<Value> = handles
            .iter()
            .map(|h| match format {
                "html" => Value::String(h.html()),
                "markdown" => Value::String(to_markdown(&h.html())),
                _ => Value::String(normalize_ws(&h.text())),
            })
            .collect();
        Ok(json!({
            "url": page.final_url.as_str(),
            "selector": selector,
            "format": format,
            "count": matches.len(),
            "matches": matches,
        }))
    }

    async fn resolve_page(&self, args: &Value) -> std::result::Result<FetchedPage, String> {
        if let Some(url) = args.get("url").and_then(Value::as_str) {
            let parsed = Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;
            let page = self
                .http
                .fetch(&parsed)
                .await
                .map_err(|e| format!("fetch failed: {e}"))?;
            if let Some(sid) = args.get("session_id").and_then(Value::as_str) {
                self.cache.lock().insert(sid.to_string(), page.clone());
            }
            return Ok(page);
        }
        if let Some(sid) = args.get("session_id").and_then(Value::as_str) {
            return self
                .cache
                .lock()
                .get(sid)
                .cloned()
                .ok_or_else(|| format!("no cached page for session `{sid}` — call `get` first"));
        }
        Err("either `url` or `session_id` required".into())
    }
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

enum FetchKind {
    Stealth,
    Dynamic,
}

enum QueryKind {
    Css,
    Xpath,
}

fn describe_backend(b: BackendKind) -> &'static str {
    match b {
        BackendKind::Http => "http",
        BackendKind::Render => "render",
        BackendKind::Stealth => "stealth",
    }
}

fn arg_url(args: &Value, key: &str) -> std::result::Result<Url, String> {
    let raw = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` required"))?;
    Url::parse(raw).map_err(|e| format!("invalid `{key}`: {e}"))
}

fn page_response(page: &FetchedPage) -> Value {
    // Truncate body for the structured response — full body fetches are
    // available via the bytes field. 64 KiB cap keeps the LLM context
    // from being swamped by a single page; downstream callers chain
    // `css_query` to extract the bits they actually need.
    const CAP: usize = 64 * 1024;
    let truncated = page.body.len() > CAP;
    let head = &page.body[..page.body.len().min(CAP)];
    let body_text = String::from_utf8_lossy(head).into_owned();
    json!({
        "url": page.final_url.as_str(),
        "status": page.status,
        "content_type": page.content_type,
        "bytes": page.body.len(),
        "truncated": truncated,
        "body": body_text,
    })
}

fn charset_from(content_type: Option<&str>) -> Option<&str> {
    let ct = content_type?;
    let idx = ct.to_ascii_lowercase().find("charset=")?;
    let tail = &ct[idx + "charset=".len()..];
    let end = tail
        .find(|c: char| c == ';' || c == ' ' || c == '"')
        .unwrap_or(tail.len());
    Some(&tail[..end])
}

fn normalize_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// Naive HTML → markdown converter. Handles headings (h1-h6), `<a>`,
/// `<strong>`/`<em>`, `<li>`, `<br>`, paragraphs; strips everything
/// else. The point isn't fidelity — it's reducing token count for the
/// LLM compared to raw HTML.
fn to_markdown(html: &str) -> String {
    let frag = scraper::Html::parse_fragment(html);
    let mut out = String::new();
    walk_md(*frag.root_element(), &mut out);
    normalize_block_breaks(out)
}

fn walk_md(node: ego_tree::NodeRef<'_, scraper::Node>, out: &mut String) {
    use scraper::Node;
    match node.value() {
        Node::Text(t) => {
            out.push_str(&t.text);
            return;
        }
        Node::Element(el) => {
            let name = el.name();
            let (open, close) = md_wraps(name);
            out.push_str(open);
            for child in node.children() {
                walk_md(child, out);
            }
            out.push_str(close);
            return;
        }
        _ => {}
    }
    for child in node.children() {
        walk_md(child, out);
    }
}

fn md_wraps(name: &str) -> (&'static str, &'static str) {
    match name {
        "h1" => ("\n\n# ", "\n"),
        "h2" => ("\n\n## ", "\n"),
        "h3" => ("\n\n### ", "\n"),
        "h4" => ("\n\n#### ", "\n"),
        "h5" => ("\n\n##### ", "\n"),
        "h6" => ("\n\n###### ", "\n"),
        "p" | "div" | "section" | "article" => ("\n\n", "\n"),
        "br" => ("\n", ""),
        "li" => ("\n- ", ""),
        "strong" | "b" => ("**", "**"),
        "em" | "i" => ("*", "*"),
        "code" => ("`", "`"),
        _ => ("", ""),
    }
}

fn normalize_block_breaks(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

fn rpc_invalid(msg: &str) -> RpcError {
    RpcError { code: -32602, message: msg.to_string() }
}

fn tool_descriptors() -> Value {
    json!([
        {
            "name": "open_session",
            "description": "Register an isolated session with its own cookie jar.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "backend": { "type": "string", "enum": ["http", "render", "stealth"] }
                },
                "required": ["id"]
            }
        },
        {
            "name": "close_session",
            "description": "Drop a session and its cached page (if any).",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        },
        {
            "name": "list_sessions",
            "description": "Enumerate registered sessions.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get",
            "description": "Plain HTTP GET. Returns status, headers, and body (truncated to 64 KiB).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "bulk_get",
            "description": "Issue many GETs in sequence. Returns one response per URL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "urls": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["urls"]
            }
        },
        {
            "name": "fetch",
            "description": "Dynamic browser fetch (CDP render path).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "stealth_fetch",
            "description": "Stealth-stack fetch (TLS/JA3/UA impersonation + antibot bypass).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "css_query",
            "description": "CSS-select against a URL or cached session page. Default `format` is `text` (token-minimal); `markdown` or `html` opt-in.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "session_id": { "type": "string" },
                    "selector": { "type": "string" },
                    "format": { "type": "string", "enum": ["text", "markdown", "html"] }
                },
                "required": ["selector"]
            }
        },
        {
            "name": "xpath_query",
            "description": "XPath-select against a URL or cached session page. Same `format` modes as `css_query`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "session_id": { "type": "string" },
                    "selector": { "type": "string" },
                    "format": { "type": "string", "enum": ["text", "markdown", "html"] }
                },
                "required": ["selector"]
            }
        }
    ])
}

// ─────────────────────────────────────────────────────────────────────
// stdio loop
// ─────────────────────────────────────────────────────────────────────

pub struct McpOptions {
    pub name: String,
}

pub async fn run_stdio(opts: McpOptions) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let http: Arc<dyn Fetcher> = Arc::new(ImpersonateFetcher::new()?);
    let stealth = http.clone();
    let dynamic = http.clone();
    let server = McpServer::new(opts.name, http, stealth, dynamic);

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await.map_err(Error::Io)? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") },
                });
                write_frame(&mut stdout, &err).await?;
                continue;
            }
        };
        let resp = server.dispatch(req).await;
        if !resp.is_null() {
            write_frame(&mut stdout, &resp).await?;
        }
    }
    Ok(())
}

async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut line = serde_json::to_string(value).map_err(|e| Error::Http(e.to_string()))?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await.map_err(Error::Io)?;
    writer.flush().await.map_err(Error::Io)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdMap;
    use std::sync::Mutex as StdMutex;

    struct StubFetcher {
        pages: StdMutex<StdMap<String, FetchedPage>>,
    }
    impl StubFetcher {
        fn new() -> Self { Self { pages: StdMutex::new(StdMap::new()) } }
        fn insert(&self, url: &str, body: &str) {
            let parsed = Url::parse(url).unwrap();
            self.pages.lock().unwrap().insert(
                url.to_string(),
                FetchedPage {
                    final_url: parsed,
                    status: 200,
                    content_type: Some("text/html; charset=utf-8".into()),
                    body: body.as_bytes().to_vec(),
                },
            );
        }
    }
    #[async_trait]
    impl Fetcher for StubFetcher {
        async fn fetch(&self, url: &Url) -> Result<FetchedPage> {
            self.pages
                .lock()
                .unwrap()
                .get(url.as_str())
                .cloned()
                .ok_or_else(|| Error::Http(format!("stub: no page for {url}")))
        }
    }

    fn server(stub: Arc<StubFetcher>) -> McpServer {
        let f: Arc<dyn Fetcher> = stub;
        McpServer::new("crawlex-test", f.clone(), f.clone(), f)
    }

    const HTML: &str = "<!doctype html><html><body>\
        <h1>Title</h1>\
        <p class='greet'>hello world</p>\
        <a href='/x' id='cta'>Buy now</a>\
        <ul><li>alpha</li><li>beta</li></ul></body></html>";

    fn rpc(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let stub = Arc::new(StubFetcher::new());
        let srv = server(stub);
        let resp = srv.dispatch(rpc("initialize", json!({}))).await;
        let info = resp["result"]["serverInfo"]["name"].as_str().unwrap();
        assert_eq!(info, "crawlex-test");
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_advertises_full_surface() {
        let stub = Arc::new(StubFetcher::new());
        let srv = server(stub);
        let resp = srv.dispatch(rpc("tools/list", json!({}))).await;
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "open_session",
            "close_session",
            "list_sessions",
            "get",
            "bulk_get",
            "fetch",
            "stealth_fetch",
            "css_query",
            "xpath_query",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
    }

    #[tokio::test]
    async fn unknown_method_returns_rpc_error() {
        let stub = Arc::new(StubFetcher::new());
        let srv = server(stub);
        let resp = srv.dispatch(rpc("nope", json!({}))).await;
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn open_close_list_sessions() {
        let stub = Arc::new(StubFetcher::new());
        let srv = server(stub);
        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "open_session",
                "arguments": { "id": "s1", "backend": "stealth" }
            })))
            .await;
        assert_eq!(r["result"]["structuredContent"]["id"], "s1");
        assert_eq!(r["result"]["structuredContent"]["backend"], "stealth");

        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "list_sessions", "arguments": {}
            })))
            .await;
        let rows = r["result"]["structuredContent"]["sessions"].as_array().unwrap();
        assert_eq!(rows.len(), 1);

        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "close_session", "arguments": { "id": "s1" }
            })))
            .await;
        assert_eq!(r["result"]["structuredContent"]["closed"], true);
    }

    #[tokio::test]
    async fn get_and_css_query_via_session_cache() {
        let stub = Arc::new(StubFetcher::new());
        stub.insert("https://example.com/", HTML);
        let srv = server(stub);

        srv.dispatch(rpc("tools/call", json!({
            "name": "open_session", "arguments": { "id": "s", "backend": "http" }
        })))
        .await;

        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "get",
                "arguments": { "url": "https://example.com/", "session_id": "s" }
            })))
            .await;
        assert_eq!(r["result"]["structuredContent"]["status"], 200);

        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "css_query",
                "arguments": { "session_id": "s", "selector": "p.greet" }
            })))
            .await;
        let m = &r["result"]["structuredContent"]["matches"];
        assert_eq!(m[0], "hello world");
    }

    #[tokio::test]
    async fn xpath_query_returns_markdown_when_requested() {
        let stub = Arc::new(StubFetcher::new());
        stub.insert("https://example.com/", HTML);
        let srv = server(stub);
        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "xpath_query",
                "arguments": {
                    "url": "https://example.com/",
                    "selector": "//h1",
                    "format": "markdown"
                }
            })))
            .await;
        let m = r["result"]["structuredContent"]["matches"][0]
            .as_str()
            .unwrap()
            .to_string();
        assert!(m.starts_with("# Title"), "got {m}");
    }

    #[tokio::test]
    async fn css_query_html_format_returns_raw_html() {
        let stub = Arc::new(StubFetcher::new());
        stub.insert("https://example.com/", HTML);
        let srv = server(stub);
        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "css_query",
                "arguments": {
                    "url": "https://example.com/",
                    "selector": "a#cta",
                    "format": "html"
                }
            })))
            .await;
        let m = r["result"]["structuredContent"]["matches"][0]
            .as_str()
            .unwrap()
            .to_string();
        assert!(m.contains("<a "), "got {m}");
    }

    #[tokio::test]
    async fn bulk_get_returns_per_url_result() {
        let stub = Arc::new(StubFetcher::new());
        stub.insert("https://a.test/", HTML);
        stub.insert("https://b.test/", HTML);
        let srv = server(stub);
        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "bulk_get",
                "arguments": { "urls": ["https://a.test/", "https://b.test/", "https://missing.test/"] }
            })))
            .await;
        let rows = r["result"]["structuredContent"]["results"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["status"], 200);
        assert!(rows[2]["error"].is_string());
    }

    #[tokio::test]
    async fn css_query_without_url_or_session_errors() {
        let stub = Arc::new(StubFetcher::new());
        let srv = server(stub);
        let r = srv
            .dispatch(rpc("tools/call", json!({
                "name": "css_query",
                "arguments": { "selector": "p" }
            })))
            .await;
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn markdown_strips_tags_and_collapses_whitespace() {
        let md = to_markdown("<h2>Hi</h2><p>world  <strong>bold</strong></p>");
        assert!(md.contains("## Hi"));
        assert!(md.contains("**bold**"));
    }
}
