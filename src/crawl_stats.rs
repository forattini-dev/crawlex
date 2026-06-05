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

/// Operator-safe identity dimensions for measuring proxy/fingerprint health.
///
/// This stores logical provider/profile/session identifiers and observed exit IP
/// only; proxy credentials and raw proxy URLs stay out of this shape so it is
/// safe to serialize into crawl events and local storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessIdentity {
    pub target_host: String,
    pub endpoint_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sticky_identity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_ip: Option<String>,
    pub persona_id: String,
    pub tls_profile_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_profile_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ua_hash: Option<String>,
}

impl AccessIdentity {
    pub fn new(
        target_host: impl Into<String>,
        endpoint_class: impl Into<String>,
        persona_id: impl Into<String>,
        tls_profile_name: impl Into<String>,
    ) -> Self {
        Self {
            target_host: target_host.into(),
            endpoint_class: endpoint_class.into(),
            proxy_provider: None,
            proxy_profile_id: None,
            sticky_identity_id: None,
            exit_ip: None,
            persona_id: persona_id.into(),
            tls_profile_name: tls_profile_name.into(),
            headers_profile_hash: None,
            ua_hash: None,
        }
    }

    /// Stable, human-auditable grouping key for reputation rollups. We keep it
    /// readable for operational debugging; fields are already redacted/safe.
    pub fn access_identity_key(&self) -> String {
        [
            self.target_host.as_str(),
            self.endpoint_class.as_str(),
            self.proxy_provider.as_deref().unwrap_or("direct"),
            self.proxy_profile_id.as_deref().unwrap_or("default"),
            self.sticky_identity_id.as_deref().unwrap_or("non_sticky"),
            self.exit_ip.as_deref().unwrap_or("unknown_exit_ip"),
            self.persona_id.as_str(),
            self.tls_profile_name.as_str(),
            self.headers_profile_hash
                .as_deref()
                .unwrap_or("unknown_headers"),
            self.ua_hash.as_deref().unwrap_or("unknown_ua"),
        ]
        .join("|")
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_identity_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_identity: Option<AccessIdentity>,
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
            access_identity_key: None,
            access_identity: None,
            observed_at: now_unix(),
        }
    }

    pub fn with_access_identity(mut self, identity: AccessIdentity) -> Self {
        self.access_identity_key = Some(identity.access_identity_key());
        self.access_identity = Some(identity);
        self
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

    #[test]
    fn access_identity_key_separates_sticky_proxy_and_fingerprint() {
        let mut identity = AccessIdentity::new(
            "www.drogariasaopaulo.com.br",
            "vtex_api",
            "pixel",
            "chrome_99.0.4844.73_android12-pixel6",
        );
        identity.proxy_provider = Some("packetstream".into());
        identity.proxy_profile_id = Some("packetstream-default".into());
        identity.sticky_identity_id =
            Some("packetstream:www.drogariasaopaulo.com.br:vtex_api:pixel".into());
        identity.exit_ip = Some("192.0.2.10".into());
        identity.headers_profile_hash = Some("headers-a".into());
        identity.ua_hash = Some("ua-a".into());

        let base = identity.access_identity_key();
        assert!(base.contains("www.drogariasaopaulo.com.br"));
        assert!(base.contains("192.0.2.10"));
        assert!(base.contains("chrome_99.0.4844.73_android12-pixel6"));

        let mut changed_ip = identity.clone();
        changed_ip.exit_ip = Some("192.0.2.11".into());
        assert_ne!(base, changed_ip.access_identity_key());

        let mut changed_tls = identity.clone();
        changed_tls.tls_profile_name = "chrome_116.0.5845.180_win10".into();
        assert_ne!(base, changed_tls.access_identity_key());
    }

    #[test]
    fn attempt_json_carries_access_identity_without_proxy_credentials() {
        let proxy: Url = "socks5h://user:pass@proxy.example.test:31113"
            .parse()
            .unwrap();
        let mut identity = AccessIdentity::new(
            "www.drogariasaopaulo.com.br",
            "vtex_api",
            "pixel",
            "chrome_99.0.4844.73_android12-pixel6",
        );
        identity.proxy_provider = Some("packetstream".into());
        identity.proxy_profile_id = Some("packetstream-default".into());
        identity.sticky_identity_id =
            Some("packetstream:www.drogariasaopaulo.com.br:vtex_api:pixel".into());
        identity.exit_ip = Some("192.0.2.10".into());

        let attempt = CrawlAttemptRecord::new(
            1,
            "https://www.drogariasaopaulo.com.br/api/catalog_system/pub/products/search/paracetamol"
                .parse()
                .unwrap(),
            1,
            AttemptEngine::HttpSpoof,
            Some(proxy.clone()),
            Some(proxy),
            Some(206),
            false,
            None,
            None,
            None,
            42,
            None,
        )
        .with_access_identity(identity);

        let json = serde_json::to_string(&attempt).unwrap();
        assert!(json.contains("access_identity_key"));
        assert!(json.contains("sticky_identity_id"));
        assert!(json.contains("chrome_99.0.4844.73_android12-pixel6"));
        assert!(json.contains("192.0.2.10"));
        assert!(!json.contains("user"));
        assert!(!json.contains("pass"));
    }
}
