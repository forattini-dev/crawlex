use serde::{Deserialize, Serialize, Serializer};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

use crate::antibot::{ChallengeLevel, ChallengeVendor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptEngine {
    HttpSpoof,
    Render,
    FallbackCommand,
}

impl AttemptEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HttpSpoof => "http_spoof",
            Self::Render => "render",
            Self::FallbackCommand => "fallback_command",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlAttemptRecord {
    pub crawl_id: u64,
    pub url: Url,
    pub attempt_index: u32,
    pub engine: AttemptEngine,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_redacted_proxy_url"
    )]
    pub proxy_requested: Option<Url>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_redacted_proxy_url"
    )]
    pub proxy_effective: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<ChallengeVendor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<ChallengeLevel>,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub observed_at: i64,
}

impl CrawlAttemptRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        crawl_id: u64,
        url: Url,
        attempt_index: u32,
        engine: AttemptEngine,
        proxy_requested: Option<Url>,
        proxy_effective: Option<Url>,
        status: Option<u16>,
        blocked: bool,
        block_reason: Option<String>,
        vendor: Option<ChallengeVendor>,
        level: Option<ChallengeLevel>,
        latency_ms: u64,
        error: Option<String>,
    ) -> Self {
        Self {
            crawl_id,
            url,
            attempt_index,
            engine,
            proxy_requested,
            proxy_effective,
            status,
            blocked,
            block_reason,
            vendor,
            level,
            latency_ms,
            error,
            observed_at: now_unix(),
        }
    }
}

/// Render a URL for operator-visible logs/events/storage without leaking proxy
/// credentials. Crawlex must still pass the original URL to the transport path;
/// this helper is only for serialization/persistence surfaces.
pub fn redact_url_credentials(url: &Url) -> String {
    if url.username().is_empty() && url.password().is_none() {
        return url.to_string();
    }
    let mut redacted = url.clone();
    let _ = redacted.set_username("[REDACTED]");
    let _ = redacted.set_password(None);
    redacted.to_string()
}

fn serialize_redacted_proxy_url<S>(value: &Option<Url>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(url) => serializer.serialize_some(&redact_url_credentials(url)),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlStats {
    pub crawl_id: u64,
    pub url: Url,
    pub attempts: Vec<CrawlAttemptRecord>,
    pub fallback_fetch_used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<AttemptEngine>,
    pub success: bool,
}

impl CrawlStats {
    pub fn new(crawl_id: u64, url: Url) -> Self {
        Self {
            crawl_id,
            url,
            attempts: Vec::new(),
            fallback_fetch_used: false,
            resolved_by: None,
            success: false,
        }
    }

    pub fn push_attempt(&mut self, attempt: CrawlAttemptRecord) {
        self.attempts.push(attempt);
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_proxy_credentials_in_attempt_json() {
        let proxy: Url = "socks5h://user:pass@proxy.example.test:31113"
            .parse()
            .unwrap();
        let attempt = CrawlAttemptRecord::new(
            1,
            "https://www.drogariasaopaulo.com.br/search?w=paracetamol"
                .parse()
                .unwrap(),
            1,
            AttemptEngine::HttpSpoof,
            Some(proxy.clone()),
            Some(proxy),
            Some(200),
            false,
            None,
            None,
            None,
            42,
            None,
        );

        let json = serde_json::to_string(&attempt).unwrap();
        assert!(!json.contains("user"));
        assert!(!json.contains("pass"));
        assert!(json.contains("socks5h://%5BREDACTED%5D@proxy.example.test:31113"));
    }
}
