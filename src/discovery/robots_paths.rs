//! Surface expansion via robots.txt.
//!
//! Most sites leak their admin/API surface in `Disallow:` entries. We parse
//! those patterns and materialize concrete URLs the crawler can probe. This
//! often uncovers paths never linked from the public HTML.

use url::Url;

/// Extract candidate paths from a robots.txt body. Keeps both `Disallow:` and
/// `Allow:` entries (paths worth visiting are on both sides). Strips trailing
/// wildcards (`/admin/*` -> `/admin/`) and skips overly-broad patterns that
/// would produce noise (`/`, `*`, empty).
pub fn extract_paths(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in body.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if key != "disallow" && key != "allow" {
            continue;
        }
        let raw = val.trim();
        if raw.is_empty() || raw == "/" || raw == "*" {
            continue;
        }
        // Collapse wildcard globs into their literal prefix. We don't brute
        // the glob; we just probe the prefix, which almost always exists.
        let path = raw.split('*').next().unwrap_or(raw).trim_end_matches('$');
        if path.len() <= 1 {
            continue;
        }
        let norm = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        if seen.insert(norm.clone()) {
            out.push(norm);
        }
    }
    out
}

/// Materialize a list of robots paths into absolute URLs rooted at `origin`.
pub fn seed_urls(origin: &Url, paths: &[String]) -> Vec<Url> {
    let mut urls = Vec::new();
    for p in paths {
        if let Ok(u) = origin.join(p) {
            urls.push(u);
        }
    }
    urls
}
