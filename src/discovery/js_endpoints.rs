//! Extract URLs and API paths from JavaScript source bodies.
//!
//! Two sources of signal:
//! * Absolute `http(s)://...` URL literals in string constants.
//! * Path-like string constants beginning with `/` that match common API
//!   prefixes (`/api/`, `/v1/`, `/v2/`, `/graphql`, `/rest/`, `/rpc/`).
//!
//! We don't run the JS — pure static extraction, conservative regex-free
//! (bytes scan) to stay cheap. Output is deduped.

use std::collections::HashSet;
use url::Url;

/// Extract both absolute URLs and site-relative API paths. Paths are joined
/// against `base` to produce absolute URLs.
pub fn extract(base: &Url, js: &str) -> Vec<Url> {
    let mut set: HashSet<String> = HashSet::new();
    for lit in string_literals(js) {
        let trimmed = lit.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            if let Ok(u) = Url::parse(trimmed) {
                set.insert(u.to_string());
            }
            continue;
        }
        if trimmed.starts_with('/') && looks_like_api_path(trimmed) {
            if let Ok(u) = base.join(trimmed) {
                set.insert(u.to_string());
            }
        }
    }
    set.into_iter()
        .filter_map(|s| Url::parse(&s).ok())
        .collect()
}

fn looks_like_api_path(p: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "/api/",
        "/v1/",
        "/v2/",
        "/v3/",
        "/graphql",
        "/rest/",
        "/rpc/",
        "/admin/",
        "/internal/",
        "/.well-known/",
    ];
    PREFIXES.iter().any(|pre| p.starts_with(pre))
}

/// Yield every string literal ("...", '...', `...`) in the source — naive
/// byte walk, no template-expression awareness. Good enough for URL harvesting
/// since template ${} only interrupts on runtime, and we still capture the
/// surrounding static segments.
fn string_literals(src: &str) -> Vec<&str> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\'' || c == b'`' {
            let quote = c;
            let start = i + 1;
            let mut end = start;
            while end < b.len() {
                let c2 = b[end];
                if c2 == b'\\' {
                    end += 2;
                    continue;
                }
                if c2 == quote {
                    break;
                }
                end += 1;
            }
            if end <= b.len() && end >= start {
                // Only return literals with at least 2 chars — avoids noise.
                if end - start > 2 {
                    if let Ok(s) = std::str::from_utf8(&b[start..end]) {
                        out.push(s);
                    }
                }
            }
            i = end + 1;
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            // Line comment: skip to newline.
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            // Block comment.
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}
