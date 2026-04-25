//! robots.txt Sitemap: directive extraction + sitemap.xml URL parsing.
//! No external XML dep: we scan for `<loc>URL</loc>` spans, which covers
//! both regular sitemaps and sitemap indexes.

use url::Url;

/// Extract all Sitemap: URLs from a robots.txt body.
pub fn sitemap_urls_from_robots(body: &str) -> Vec<Url> {
    let mut out = Vec::new();
    for line in body.lines() {
        let l = line.trim();
        // Case-insensitive "sitemap:" prefix.
        if l.len() < 8 {
            continue;
        }
        let (head, rest) = l.split_at(8);
        if head.eq_ignore_ascii_case("sitemap:") {
            if let Ok(u) = Url::parse(rest.trim()) {
                out.push(u);
            }
        }
    }
    out
}

/// Extract all <loc>...</loc> URLs from a sitemap XML body.
pub fn urls_from_sitemap_xml(body: &str) -> Vec<Url> {
    let mut out = Vec::new();
    let mut cursor = 0;
    let bytes = body.as_bytes();
    while cursor < bytes.len() {
        let Some(start) = find_ci(&body[cursor..], "<loc>") else {
            break;
        };
        let abs_start = cursor + start + 5;
        let Some(end) = find_ci(&body[abs_start..], "</loc>") else {
            break;
        };
        let abs_end = abs_start + end;
        let raw = body[abs_start..abs_end].trim();
        if let Ok(u) = Url::parse(raw) {
            out.push(u);
        }
        cursor = abs_end + 6;
    }
    out
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let n = needle.as_bytes();
    let h = hay.as_bytes();
    if n.len() > h.len() {
        return None;
    }
    'outer: for i in 0..=h.len() - n.len() {
        for j in 0..n.len() {
            if !h[i + j].eq_ignore_ascii_case(&n[j]) {
                continue 'outer;
            }
        }
        return Some(i);
    }
    None
}
