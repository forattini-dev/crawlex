//! Historical URL harvesting via the Internet Archive CDX API.
//!
//! Query: http://web.archive.org/cdx/search/cdx?url=*.<domain>/*&output=json&collapse=urlkey
//! The first row is a header (keys), subsequent rows are arrays of values.

use url::Url;

use crate::impersonate::ImpersonateClient;
use crate::Result;

pub async fn wayback_urls(client: &ImpersonateClient, domain: &str) -> Result<Vec<Url>> {
    let q = format!(
        "http://web.archive.org/cdx/search/cdx?url=*.{domain}/*&output=json&collapse=urlkey&fl=original&limit=5000"
    );
    let url = Url::parse(&q)?;
    let resp = client.get(&url).await?;
    if !resp.status.is_success() {
        return Ok(Vec::new());
    }
    let body = String::from_utf8_lossy(&resp.body);
    Ok(parse_cdx(&body))
}

/// Parse the JSON array-of-arrays format. Row 0 is a header; we extract the
/// `original` column (stringly typed; fl=original narrows us to that one).
pub fn parse_cdx(body: &str) -> Vec<Url> {
    let body = body.trim();
    if body.is_empty() || body == "[]" {
        return Vec::new();
    }
    let mut urls = Vec::new();
    // Naive: find every `"..."` whose content parses as an http(s) URL.
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                if bytes[end] == b'\\' {
                    end += 2;
                } else {
                    end += 1;
                }
            }
            if end <= bytes.len() {
                let s = &body[start..end.min(bytes.len())];
                if s.starts_with("http://") || s.starts_with("https://") {
                    if let Ok(u) = Url::parse(s) {
                        urls.push(u);
                    }
                }
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    urls
}
