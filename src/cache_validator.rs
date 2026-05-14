use http::HeaderMap;
use regex::Regex;
use scraper::{Html, Selector};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::storage::PageCacheMetadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheValidationStatus {
    Fresh,
    Stale,
    Unknown,
}

impl CacheValidationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheValidationOutcome {
    pub status: CacheValidationStatus,
    pub reason: String,
    pub new_etag: Option<String>,
    pub new_last_modified: Option<String>,
    pub new_head_fingerprint: Option<String>,
}

impl CacheValidationOutcome {
    pub fn fresh(reason: impl Into<String>) -> Self {
        Self {
            status: CacheValidationStatus::Fresh,
            reason: reason.into(),
            new_etag: None,
            new_last_modified: None,
            new_head_fingerprint: None,
        }
    }

    fn with_headers(mut self, headers: &HeaderMap) -> Self {
        self.new_etag = header_string(headers, "etag");
        self.new_last_modified = header_string(headers, "last-modified");
        self
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn is_fresh_by_age(meta: &PageCacheMetadata, max_age_secs: Option<u64>) -> bool {
    let Some(max_age_secs) = max_age_secs else {
        return false;
    };
    now_unix().saturating_sub(meta.saved_at_unix) <= max_age_secs
}

pub fn validate_response(
    meta: &PageCacheMetadata,
    status: u16,
    headers: &HeaderMap,
    body: &[u8],
) -> CacheValidationOutcome {
    if status == 304 {
        return CacheValidationOutcome::fresh("server returned 304").with_headers(headers);
    }
    if !(200..400).contains(&status) {
        return CacheValidationOutcome {
            status: CacheValidationStatus::Unknown,
            reason: format!("status {status} is not cache-validating"),
            new_etag: header_string(headers, "etag"),
            new_last_modified: header_string(headers, "last-modified"),
            new_head_fingerprint: None,
        };
    }

    let new_etag = header_string(headers, "etag");
    let new_last_modified = header_string(headers, "last-modified");

    if let (Some(old), Some(new)) = (meta.etag.as_deref(), new_etag.as_deref()) {
        if normalize_validator(old) == normalize_validator(new) {
            return CacheValidationOutcome {
                status: CacheValidationStatus::Fresh,
                reason: "etag matched".to_string(),
                new_etag,
                new_last_modified,
                new_head_fingerprint: None,
            };
        }
        return CacheValidationOutcome {
            status: CacheValidationStatus::Stale,
            reason: "etag changed".to_string(),
            new_etag,
            new_last_modified,
            new_head_fingerprint: None,
        };
    }

    let new_head_fingerprint = looks_like_html(headers, body)
        .then(|| String::from_utf8_lossy(body).to_string())
        .and_then(|html| compute_head_fingerprint(&html));

    if let (Some(old), Some(new)) = (
        meta.head_fingerprint.as_deref(),
        new_head_fingerprint.as_deref(),
    ) {
        if old == new {
            return CacheValidationOutcome {
                status: CacheValidationStatus::Fresh,
                reason: "head fingerprint matched".to_string(),
                new_etag,
                new_last_modified,
                new_head_fingerprint,
            };
        }
        return CacheValidationOutcome {
            status: CacheValidationStatus::Stale,
            reason: "head fingerprint changed".to_string(),
            new_etag,
            new_last_modified,
            new_head_fingerprint,
        };
    }

    if let (Some(old), Some(new)) = (meta.last_modified.as_deref(), new_last_modified.as_deref()) {
        if normalize_validator(old) == normalize_validator(new) {
            return CacheValidationOutcome {
                status: CacheValidationStatus::Fresh,
                reason: "last-modified matched".to_string(),
                new_etag,
                new_last_modified,
                new_head_fingerprint,
            };
        }
    }

    CacheValidationOutcome {
        status: CacheValidationStatus::Unknown,
        reason: "no matching validator".to_string(),
        new_etag,
        new_last_modified,
        new_head_fingerprint,
    }
}

pub fn compute_head_fingerprint(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let mut parts = Vec::new();

    if let Ok(sel) = Selector::parse("head title") {
        for node in document.select(&sel) {
            let text = compact_text(&node.text().collect::<Vec<_>>().join(" "));
            if !text.is_empty() {
                parts.push(format!("title={text}"));
            }
        }
    }

    if let Ok(sel) = Selector::parse("head meta") {
        for node in document.select(&sel) {
            let value = node.value();
            let key = value
                .attr("name")
                .or_else(|| value.attr("property"))
                .or_else(|| value.attr("http-equiv"));
            let content = value.attr("content");
            if let (Some(key), Some(content)) = (key, content) {
                let content = compact_text(content);
                if !content.is_empty() {
                    parts.push(format!(
                        "meta:{}={}",
                        key.trim().to_ascii_lowercase(),
                        content
                    ));
                }
            }
        }
    }

    if let Ok(sel) = Selector::parse("head link") {
        for node in document.select(&sel) {
            let value = node.value();
            let rel = value.attr("rel").unwrap_or("").trim().to_ascii_lowercase();
            let href = value.attr("href").unwrap_or("").trim();
            if !rel.is_empty() && !href.is_empty() {
                parts.push(format!("link:{rel}={href}"));
            }
        }
    }

    if let Ok(sel) = Selector::parse("head script[src]") {
        for node in document.select(&sel) {
            if let Some(src) = node.value().attr("src") {
                let src = src.trim();
                if !src.is_empty() {
                    parts.push(format!("script={src}"));
                }
            }
        }
    }

    if parts.is_empty() {
        let head = extract_head_text_fallback(html)?;
        if head.trim().is_empty() {
            return None;
        }
        parts.push(compact_text(&head));
    }

    parts.sort();
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\n");
    }
    Some(hex::encode(hasher.finalize()))
}

pub fn header_string(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn looks_like_html(headers: &HeaderMap, body: &[u8]) -> bool {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            let ct = ct.to_ascii_lowercase();
            ct.contains("text/html") || ct.contains("application/xhtml")
        })
        .unwrap_or_else(|| {
            let prefix = String::from_utf8_lossy(&body[..body.len().min(512)]);
            prefix.contains("<html")
                || prefix.contains("<!doctype html")
                || prefix.contains("<head")
        })
}

fn normalize_validator(s: &str) -> String {
    s.trim()
        .trim_start_matches("W/")
        .trim_matches('"')
        .to_string()
}

fn compact_text(s: &str) -> String {
    static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("static regex"));
    WS.replace_all(s.trim(), " ").to_string()
}

fn extract_head_text_fallback(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<head")?;
    let start = lower[start..].find('>').map(|i| start + i + 1)?;
    let end = lower[start..]
        .find("</head>")
        .map(|i| start + i)
        .unwrap_or_else(|| html.len().min(start + 65_536));
    Some(html[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_fingerprint_ignores_body_changes() {
        let a = r#"<html><head><title>A</title><meta name="description" content="x"></head><body>one</body></html>"#;
        let b = r#"<html><head><meta name="description" content="x"><title>A</title></head><body>two</body></html>"#;
        assert_eq!(compute_head_fingerprint(a), compute_head_fingerprint(b));
    }

    #[test]
    fn etag_match_is_fresh() {
        let mut headers = HeaderMap::new();
        headers.insert("etag", "\"abc\"".parse().unwrap());
        let meta = PageCacheMetadata {
            url: url::Url::parse("https://example.com/").unwrap(),
            final_url: url::Url::parse("https://example.com/").unwrap(),
            status: 200,
            etag: Some("W/\"abc\"".to_string()),
            last_modified: None,
            head_fingerprint: None,
            saved_at_unix: now_unix(),
        };
        let out = validate_response(&meta, 200, &headers, b"");
        assert_eq!(out.status, CacheValidationStatus::Fresh);
    }
}
