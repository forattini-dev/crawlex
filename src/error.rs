//! Error taxonomy for crawlex.
//!
//! Variants are structured so callers can match and surface structured
//! reasons in NDJSON `job.failed`/`decision.made` events (phase 3).
//! Free-form `String` payloads are reserved for genuinely unexpected
//! conditions; the common failure modes (antibot challenge, DNS, TLS,
//! engine, proxy, robots, URL scheme) have their own variants.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Anti-bot vendor a challenge response was attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntibotVendor {
    Cloudflare,
    DataDome,
    PerimeterX,
    Imperva,
    DistilNetworks,
    Akamai,
    Other,
}

impl AntibotVendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cloudflare => "cloudflare",
            Self::DataDome => "datadome",
            Self::PerimeterX => "perimeterx",
            Self::Imperva => "imperva",
            Self::DistilNetworks => "distilnetworks",
            Self::Akamai => "akamai",
            Self::Other => "other",
        }
    }
}

/// Which engine / path failed. Used by Policy Engine (phase 3) to pick the
/// next engine in the waterfall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    HttpSpoof,
    Render,
    Extract,
    Proxy,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HttpSpoof => "http-spoof",
            Self::Render => "render",
            Self::Extract => "extract",
            Self::Proxy => "proxy",
        }
    }
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("url parse: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("url scheme not supported: {0}")]
    UrlScheme(String),

    #[error("dns resolution failed for {host}: {reason}")]
    DnsResolution { host: String, reason: String },

    #[error("http: {0}")]
    Http(String),

    #[error("request timeout after {timeout_ms}ms")]
    RequestTimeout { timeout_ms: u128 },

    #[error("encoded body too large: limit={limit} bytes")]
    BodyTooLarge { limit: usize },

    #[error("decoded body too large: limit={limit} bytes")]
    DecodedBodyTooLarge { limit: usize },

    #[error(
        "decompression ratio too large: encoded={encoded} decoded={decoded} ratio_limit={ratio_limit}"
    )]
    DecompressionRatioTooLarge {
        encoded: usize,
        decoded: usize,
        ratio_limit: usize,
    },

    #[error("tls handshake/verify: {0}")]
    Tls(String),

    #[error("decompression: {0}")]
    Decompression(String),

    #[error("antibot challenge ({}): status={status} {note}", vendor.as_str())]
    AntibotChallenge {
        vendor: AntibotVendor,
        status: u16,
        note: String,
    },

    #[error("engine {} failed: {reason}", engine.as_str())]
    EngineFailed { engine: Engine, reason: String },

    /// Upstream site returned an error we shouldn't retry (4xx other than
    /// 408/429, 5xx that aren't transient). Tracks status so Policy can
    /// decide drop vs defer.
    #[error("site error: status={status} {reason}")]
    SiteError { status: u16, reason: String },

    #[error("render: {0}")]
    Render(String),

    #[error("render disabled: {0}")]
    RenderDisabled(String),

    #[error("queue: {0}")]
    Queue(String),

    #[error("storage: {0}")]
    Storage(String),

    #[error("proxy selection: {0}")]
    ProxySelection(String),

    #[error("config: {0}")]
    Config(String),

    #[error("robots disallow: {0}")]
    RobotsDisallow(String),

    #[error("hook abort: {0}")]
    HookAbort(String),

    #[error("job deferred: {0}")]
    JobDeferred(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Short structured tag, suitable for NDJSON `why=` fields and metric
    /// labels. Keep stable across versions — this becomes part of the
    /// public contract once events flow to users.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::UrlParse(_) => "url-parse",
            Self::UrlScheme(_) => "url-scheme",
            Self::DnsResolution { .. } => "dns",
            Self::Http(_) => "http",
            Self::RequestTimeout { .. } => "request-timeout",
            Self::BodyTooLarge { .. } => "body-too-large",
            Self::DecodedBodyTooLarge { .. } => "decoded-body-too-large",
            Self::DecompressionRatioTooLarge { .. } => "decompression-ratio-too-large",
            Self::Tls(_) => "tls",
            Self::Decompression(_) => "decompression",
            Self::AntibotChallenge { .. } => "antibot",
            Self::EngineFailed { .. } => "engine-failed",
            Self::SiteError { .. } => "site-error",
            Self::Render(_) => "render",
            Self::RenderDisabled(_) => "render-disabled",
            Self::Queue(_) => "queue",
            Self::Storage(_) => "storage",
            Self::ProxySelection(_) => "proxy-selection",
            Self::Config(_) => "config",
            Self::RobotsDisallow(_) => "robots-disallow",
            Self::HookAbort(_) => "hook-abort",
            Self::JobDeferred(_) => "job-deferred",
            Self::Other(_) => "other",
        }
    }
}
