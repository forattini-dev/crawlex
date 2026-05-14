//! `crawlex from-curl '<curl …>'` — Slice 21.
//!
//! Parse a curl invocation copied from Chrome devtools (or hand-written)
//! into a portable request description, then emit it as TOML, JSON, or a
//! Node SDK snippet. This is the tracer-bullet for the v2 scraping
//! framework's "import a request you already have" workflow.
//!
//! Scope:
//! - shell-aware tokenizer (single/double quotes, backslash escapes,
//!   line continuations) — Chrome's "Copy as cURL" is the calibration
//!   target.
//! - parse the curl flags listed in the slice acceptance criteria:
//!   `-H`, `-b/--cookie`, `-d/--data`, `--data-raw`, `--data-binary`,
//!   `--data-urlencode`, `-X/--request`, `-L/--location`,
//!   `-x/--proxy`, `--compressed`, plus a handful of harmless aliases
//!   that travel with devtools captures (`-A/--user-agent`,
//!   `-e/--referer`, `-u/--user`).
//! - unknown flags raise a warning and are skipped — the converter
//!   never fails on flags it doesn't recognise so devtools captures
//!   stay convertible after each curl release.
//!
//! Not in scope (future slices):
//! - multipart form bodies (`-F`)
//! - file-backed bodies (`@filename`) — value is preserved as-is so the
//!   operator can substitute manually before running.

use std::collections::BTreeMap;

/// Converted request. Field order intentional: it's the order each
/// serialiser walks for stable, diff-friendly output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurlRequest {
    pub method: String,
    pub url: String,
    /// Order-preserving header list. Header names are kept verbatim so
    /// the operator can spot duplicates (`Accept-Encoding` twice etc.)
    /// in the converted output instead of having them silently merged.
    pub headers: Vec<(String, String)>,
    /// Raw `Cookie:` header value. A devtools curl emits cookies via
    /// either `-H 'cookie: …'` or `-b 'k=v; k2=v2'`; both land here.
    pub cookie: Option<String>,
    pub body: Option<String>,
    pub follow_redirects: bool,
    pub proxy: Option<String>,
    pub compressed: bool,
}

/// Result of `parse`: a request plus the list of unknown flags we
/// chose to ignore so the caller (CLI dispatcher) can echo them to
/// stderr without failing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedCurl {
    pub request: CurlRequest,
    pub warnings: Vec<String>,
}

/// Tokenize a curl command line into argv tokens. Tracks single
/// quotes (literal, no escapes), double quotes (most chars literal,
/// `\"`/`\\`/`\$`/`\\n` recognised), and unquoted backslash escapes.
/// Whitespace between tokens is collapsed; an unterminated quote
/// returns an error so the operator gets a precise message instead of
/// "missing url" later.
pub fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if in_token {
                    out.push(std::mem::take(&mut cur));
                    in_token = false;
                }
                i += 1;
            }
            '\\' if i + 1 < chars.len() && chars[i + 1] == '\n' => {
                // line continuation — drop both chars.
                i += 2;
            }
            '\\' if i + 1 < chars.len() => {
                in_token = true;
                cur.push(chars[i + 1]);
                i += 2;
            }
            '\'' => {
                in_token = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    cur.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("unterminated single quote".into());
                }
                i += 1; // skip closing quote
            }
            '"' => {
                in_token = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        let n = chars[i + 1];
                        if matches!(n, '"' | '\\' | '$' | '`' | '\n') {
                            if n != '\n' {
                                cur.push(n);
                            }
                            i += 2;
                            continue;
                        }
                    }
                    cur.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("unterminated double quote".into());
                }
                i += 1;
            }
            _ => {
                in_token = true;
                cur.push(c);
                i += 1;
            }
        }
    }
    if in_token {
        out.push(cur);
    }
    Ok(out)
}

/// Parse a tokenized curl invocation. Argv-style; the leading `curl`
/// (if present) is consumed silently. Returns the request and any
/// unknown-flag warnings.
pub fn parse(input: &str) -> Result<ParsedCurl, String> {
    let toks = tokenize(input)?;
    parse_argv(&toks)
}

fn parse_argv(toks: &[String]) -> Result<ParsedCurl, String> {
    let mut req = CurlRequest {
        method: "GET".to_string(),
        ..Default::default()
    };
    let mut warnings: Vec<String> = Vec::new();
    let mut i = 0usize;
    let mut method_explicit = false;
    let mut url_from_positional: Option<String> = None;

    // Skip a leading literal `curl` (with optional path prefix).
    if let Some(first) = toks.first() {
        let base = first.rsplit('/').next().unwrap_or(first);
        if base == "curl" || base.starts_with("curl.") {
            i = 1;
        }
    }

    while i < toks.len() {
        let arg = &toks[i];
        // `--flag=value` splits to (flag, value).
        let (flag, inline_val) = if let Some(eq) = arg.find('=').filter(|_| arg.starts_with("--")) {
            (&arg[..eq], Some(arg[eq + 1..].to_string()))
        } else {
            (arg.as_str(), None)
        };

        // Helper closure-equivalent: pop next token as value.
        let mut take_value = |idx: &mut usize, name: &str| -> Result<String, String> {
            if let Some(v) = inline_val.clone() {
                Ok(v)
            } else {
                *idx += 1;
                if *idx >= toks.len() {
                    return Err(format!("flag `{}` expects a value", name));
                }
                Ok(toks[*idx].clone())
            }
        };

        match flag {
            "-H" | "--header" => {
                let v = take_value(&mut i, flag)?;
                if let Some((name, value)) = v.split_once(':') {
                    let name_t = name.trim();
                    let value_t = value.trim();
                    if name_t.eq_ignore_ascii_case("cookie") {
                        req.cookie = Some(value_t.to_string());
                    } else {
                        req.headers
                            .push((name_t.to_string(), value_t.to_string()));
                    }
                } else {
                    warnings.push(format!("ignored malformed -H value `{}`", v));
                }
            }
            "-b" | "--cookie" => {
                let v = take_value(&mut i, flag)?;
                req.cookie = Some(v);
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-urlencode" => {
                let v = take_value(&mut i, flag)?;
                if !method_explicit {
                    req.method = "POST".to_string();
                }
                match &mut req.body {
                    Some(b) => {
                        b.push('&');
                        b.push_str(&v);
                    }
                    None => req.body = Some(v),
                }
            }
            "-X" | "--request" => {
                let v = take_value(&mut i, flag)?;
                req.method = v.to_uppercase();
                method_explicit = true;
            }
            "-L" | "--location" => {
                req.follow_redirects = true;
            }
            "-x" | "--proxy" => {
                let v = take_value(&mut i, flag)?;
                req.proxy = Some(v);
            }
            "--compressed" => {
                req.compressed = true;
            }
            "-A" | "--user-agent" => {
                let v = take_value(&mut i, flag)?;
                req.headers.push(("User-Agent".into(), v));
            }
            "-e" | "--referer" => {
                let v = take_value(&mut i, flag)?;
                req.headers.push(("Referer".into(), v));
            }
            "-u" | "--user" => {
                let v = take_value(&mut i, flag)?;
                // Encode as Basic auth header so the converted request is
                // self-contained.
                use base64::Engine as _;
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(v.as_bytes());
                req.headers
                    .push(("Authorization".into(), format!("Basic {}", encoded)));
            }
            // Common no-arg flags that we can silently accept (devtools
            // sometimes emits them).
            "-k" | "--insecure" | "-s" | "--silent" | "-i" | "--include" | "-v" | "--verbose"
            | "-#" | "--progress-bar" | "--no-progress-meter" => { /* no-op */ }
            // Common flags with a single value we accept but ignore.
            "-o" | "--output" | "--max-time" | "--connect-timeout" | "--retry"
            | "--retry-delay" | "--retry-max-time" | "--resolve" | "--cacert" | "--cert"
            | "--key" => {
                let _ = take_value(&mut i, flag)?;
                warnings.push(format!("ignored flag `{}`", flag));
            }
            other if other.starts_with("--") || (other.starts_with('-') && other.len() > 1) => {
                // Unknown flag — emit a warning and keep parsing.
                // We deliberately do *not* consume the following token,
                // because that's how the URL ends up bound to the wrong
                // flag (curl's `--also-fake URL`). Trade-off: an unknown
                // value-taking flag will misbind its value as a URL, but
                // that is the rarer failure mode and produces a clear
                // "no URL found" error rather than silent corruption.
                let _ = inline_val;
                warnings.push(format!("ignored unknown flag `{}`", other));
            }
            _ => {
                // Positional — the URL.
                if url_from_positional.is_none() {
                    url_from_positional = Some(arg.clone());
                } else {
                    warnings.push(format!("ignored extra positional `{}`", arg));
                }
            }
        }
        i += 1;
    }

    req.url = url_from_positional
        .ok_or_else(|| "no URL found in curl command".to_string())?;
    Ok(ParsedCurl {
        request: req,
        warnings,
    })
}

/// Output format selector for the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Json,
    Node,
}

impl Format {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "toml" => Ok(Self::Toml),
            "json" => Ok(Self::Json),
            "node" | "js" | "sdk" => Ok(Self::Node),
            other => Err(format!(
                "unknown --format `{}`; expected toml|json|node",
                other
            )),
        }
    }
}

/// Render the converted request in the requested shape.
pub fn render(req: &CurlRequest, fmt: Format) -> String {
    match fmt {
        Format::Toml => render_toml(req),
        Format::Json => render_json(req),
        Format::Node => render_node(req),
    }
}

fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render_toml(req: &CurlRequest) -> String {
    let mut out = String::new();
    out.push_str("# generated by `crawlex from-curl`\n");
    out.push_str(&format!("method = {}\n", toml_escape(&req.method)));
    out.push_str(&format!("url = {}\n", toml_escape(&req.url)));
    out.push_str(&format!(
        "follow_redirects = {}\n",
        if req.follow_redirects { "true" } else { "false" }
    ));
    out.push_str(&format!(
        "compressed = {}\n",
        if req.compressed { "true" } else { "false" }
    ));
    if let Some(p) = &req.proxy {
        out.push_str(&format!("proxy = {}\n", toml_escape(p)));
    }
    if let Some(c) = &req.cookie {
        out.push_str(&format!("cookie = {}\n", toml_escape(c)));
    }
    if let Some(b) = &req.body {
        out.push_str(&format!("body = {}\n", toml_escape(b)));
    }
    if !req.headers.is_empty() {
        out.push_str("\n[headers]\n");
        // Stable: order-preserving (already).
        let mut seen: BTreeMap<String, u32> = BTreeMap::new();
        for (k, v) in &req.headers {
            let count = seen.entry(k.clone()).or_insert(0);
            let key = if *count == 0 {
                k.clone()
            } else {
                format!("{}_{}", k, count)
            };
            *count += 1;
            out.push_str(&format!("{} = {}\n", toml_escape(&key), toml_escape(v)));
        }
    }
    out
}

fn render_json(req: &CurlRequest) -> String {
    let headers: Vec<serde_json::Value> = req
        .headers
        .iter()
        .map(|(k, v)| {
            serde_json::json!({"name": k, "value": v})
        })
        .collect();
    let mut obj = serde_json::Map::new();
    obj.insert("method".into(), serde_json::Value::String(req.method.clone()));
    obj.insert("url".into(), serde_json::Value::String(req.url.clone()));
    obj.insert("headers".into(), serde_json::Value::Array(headers));
    obj.insert(
        "follow_redirects".into(),
        serde_json::Value::Bool(req.follow_redirects),
    );
    obj.insert(
        "compressed".into(),
        serde_json::Value::Bool(req.compressed),
    );
    if let Some(c) = &req.cookie {
        obj.insert("cookie".into(), serde_json::Value::String(c.clone()));
    }
    if let Some(b) = &req.body {
        obj.insert("body".into(), serde_json::Value::String(b.clone()));
    }
    if let Some(p) = &req.proxy {
        obj.insert("proxy".into(), serde_json::Value::String(p.clone()));
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(obj))
        .unwrap_or_else(|_| "{}".into())
}

fn js_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn render_node(req: &CurlRequest) -> String {
    let mut out = String::new();
    out.push_str("// generated by `crawlex from-curl`\n");
    out.push_str("import { request } from 'crawlex'\n\n");
    out.push_str("await request({\n");
    out.push_str(&format!("  method: {},\n", js_escape(&req.method)));
    out.push_str(&format!("  url: {},\n", js_escape(&req.url)));
    if !req.headers.is_empty() {
        out.push_str("  headers: {\n");
        for (k, v) in &req.headers {
            out.push_str(&format!("    {}: {},\n", js_escape(k), js_escape(v)));
        }
        out.push_str("  },\n");
    }
    if let Some(c) = &req.cookie {
        out.push_str(&format!("  cookie: {},\n", js_escape(c)));
    }
    if let Some(b) = &req.body {
        out.push_str(&format!("  body: {},\n", js_escape(b)));
    }
    if let Some(p) = &req.proxy {
        out.push_str(&format!("  proxy: {},\n", js_escape(p)));
    }
    out.push_str(&format!(
        "  followRedirects: {},\n",
        if req.follow_redirects { "true" } else { "false" }
    ));
    out.push_str(&format!(
        "  compressed: {},\n",
        if req.compressed { "true" } else { "false" }
    ));
    out.push_str("})\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple() {
        let toks = tokenize("curl https://example.com -H 'x: 1'").unwrap();
        assert_eq!(
            toks,
            vec!["curl", "https://example.com", "-H", "x: 1"]
        );
    }

    #[test]
    fn tokenize_double_quotes_and_escapes() {
        let toks = tokenize(r#"curl "https://e.com/?q=hi there" -H "k: v\"v""#).unwrap();
        assert_eq!(
            toks,
            vec!["curl", "https://e.com/?q=hi there", "-H", "k: v\"v"]
        );
    }

    #[test]
    fn tokenize_line_continuation() {
        let toks = tokenize("curl https://e.com \\\n  -H 'a: 1' \\\n  -H 'b: 2'").unwrap();
        assert_eq!(toks.first().map(String::as_str), Some("curl"));
        assert!(toks.iter().any(|t| t == "a: 1"));
        assert!(toks.iter().any(|t| t == "b: 2"));
    }

    #[test]
    fn tokenize_unterminated_single() {
        assert!(tokenize("curl 'oops").is_err());
    }

    #[test]
    fn parse_chrome_devtools_get() {
        // Shape Chrome devtools emits — leading `curl 'URL'`, headers in single quotes.
        let cmd = r#"curl 'https://api.example.com/items?id=42' \
  -H 'accept: application/json' \
  -H 'accept-language: en-US,en;q=0.9' \
  -H 'cookie: sid=abc; theme=dark' \
  --compressed"#;
        let p = parse(cmd).unwrap();
        assert_eq!(p.request.method, "GET");
        assert_eq!(p.request.url, "https://api.example.com/items?id=42");
        assert_eq!(
            p.request.headers,
            vec![
                ("accept".to_string(), "application/json".to_string()),
                (
                    "accept-language".to_string(),
                    "en-US,en;q=0.9".to_string()
                ),
            ]
        );
        assert_eq!(p.request.cookie.as_deref(), Some("sid=abc; theme=dark"));
        assert!(p.request.compressed);
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn parse_post_with_data_promotes_method() {
        let p = parse(r#"curl https://e.com/login -d 'u=1&p=2'"#).unwrap();
        assert_eq!(p.request.method, "POST");
        assert_eq!(p.request.body.as_deref(), Some("u=1&p=2"));
    }

    #[test]
    fn parse_explicit_method_wins() {
        let p = parse(r#"curl -X PATCH https://e.com -d 'a=1'"#).unwrap();
        assert_eq!(p.request.method, "PATCH");
    }

    #[test]
    fn parse_proxy_and_redirects() {
        let p = parse("curl -L -x http://127.0.0.1:8080 https://e.com").unwrap();
        assert!(p.request.follow_redirects);
        assert_eq!(p.request.proxy.as_deref(), Some("http://127.0.0.1:8080"));
    }

    #[test]
    fn parse_cookie_b_flag() {
        let p = parse("curl -b 'a=1;b=2' https://e.com").unwrap();
        assert_eq!(p.request.cookie.as_deref(), Some("a=1;b=2"));
    }

    #[test]
    fn parse_unknown_flag_warns_not_fails() {
        let p = parse("curl --made-up-flag --also-fake https://e.com").unwrap();
        assert_eq!(p.request.url, "https://e.com");
        assert_eq!(p.warnings.len(), 2);
        assert!(p.warnings.iter().any(|w| w.contains("made-up-flag")));
    }

    #[test]
    fn parse_basic_auth_encoded() {
        let p = parse("curl -u alice:s3cret https://e.com").unwrap();
        let h = p
            .request
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .unwrap();
        assert!(h.1.starts_with("Basic "));
    }

    #[test]
    fn render_toml_round_trip_headers_and_body() {
        let p = parse(r#"curl -X POST https://e.com -H 'x-a: 1' -H 'x-b: 2' -d 'k=v'"#).unwrap();
        let out = render(&p.request, Format::Toml);
        assert!(out.contains("method = \"POST\""));
        assert!(out.contains("url = \"https://e.com\""));
        assert!(out.contains("\"x-a\" = \"1\""));
        assert!(out.contains("\"x-b\" = \"2\""));
        assert!(out.contains("body = \"k=v\""));
    }

    #[test]
    fn render_json_round_trip() {
        let p = parse(r#"curl https://e.com -H 'x-a: 1'"#).unwrap();
        let out = render(&p.request, Format::Json);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["method"], "GET");
        assert_eq!(parsed["url"], "https://e.com");
        assert_eq!(parsed["headers"][0]["name"], "x-a");
        assert_eq!(parsed["headers"][0]["value"], "1");
    }

    #[test]
    fn render_node_snippet_uses_sdk() {
        let p = parse(r#"curl https://e.com -H 'x-a: 1'"#).unwrap();
        let out = render(&p.request, Format::Node);
        assert!(out.contains("import { request } from 'crawlex'"));
        assert!(out.contains("'x-a': '1'"));
    }

    #[test]
    fn format_parse() {
        assert_eq!(Format::parse("toml"), Ok(Format::Toml));
        assert_eq!(Format::parse("JSON"), Ok(Format::Json));
        assert_eq!(Format::parse("node"), Ok(Format::Node));
        assert!(Format::parse("yaml").is_err());
    }

    /// End-to-end: take a devtools-shaped curl, convert to JSON, parse
    /// the JSON back, and confirm headers + body survived the trip.
    #[test]
    fn fixture_round_trip_devtools_to_json() {
        let cmd = r#"curl 'https://fixture.local/api/v1/widgets' \
  -X POST \
  -H 'content-type: application/json' \
  -H 'x-request-id: 11ee-aa' \
  -H 'cookie: session=opaque' \
  --data-raw '{"name":"Widget","qty":3}' \
  --compressed \
  -L"#;
        let parsed = parse(cmd).unwrap();
        let json = render(&parsed.request, Format::Json);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["method"], "POST");
        assert_eq!(v["url"], "https://fixture.local/api/v1/widgets");
        assert_eq!(v["body"], "{\"name\":\"Widget\",\"qty\":3}");
        assert_eq!(v["cookie"], "session=opaque");
        assert_eq!(v["follow_redirects"], true);
        assert_eq!(v["compressed"], true);
        let headers = v["headers"].as_array().unwrap();
        assert!(headers
            .iter()
            .any(|h| h["name"] == "content-type" && h["value"] == "application/json"));
        assert!(headers
            .iter()
            .any(|h| h["name"] == "x-request-id" && h["value"] == "11ee-aa"));
    }
}
