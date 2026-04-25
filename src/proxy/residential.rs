//! Residential proxy pool integration — SCAFFOLD.
//!
//! Wave 1 infrastructure-tier scaffold for issue #34. Defines the trait
//! surface every residential-proxy provider (BrightData, Oxylabs, IPRoyal,
//! …) must implement so the rest of the crawler can rotate through rented
//! residential IPs the same way it rotates through datacenter proxies in
//! [`crate::proxy::router`].
//!
//! **Status:** scaffold only — every provider adapter in this file returns
//! `Error::ProviderNotConfigured`. A real deployment plugs the provider's
//! HTTP gateway URL / auth into the adapter via environment variables
//! (documented in `docs/infra-tier-operator.md`) and flips the CLI flag
//! `--residential-provider <brightdata|oxylabs|iproyal>` (to be wired into
//! [`crate::cli::args::CrawlArgs`] in a follow-up wave — this file
//! intentionally does *not* touch the existing CLI surface to avoid merge
//! conflicts with sibling waves).
//!
//! Design notes:
//! * **Provider is orthogonal to `ProxyRouter`.** A `ResidentialProvider`
//!   produces *fresh* `Url` entries that the router then scores and
//!   quarantines like any other proxy. Rotation policy inside the provider
//!   (sticky session vs. per-request) is provider-local; the router still
//!   gets to blacklist a specific endpoint if its score collapses.
//! * **Outcome reporting is bidirectional.** Router tells the provider
//!   when a proxy burned (`report_outcome`) so the provider can feed that
//!   back into its own session pool (e.g. BrightData's session expiry
//!   knob). Providers that don't care can no-op.
//! * **No async in the trait.** Rotation is expected to be a cheap lookup
//!   against a locally-cached session pool; any network IO (fetching a new
//!   session token) happens in a background refresh task that the provider
//!   owns. The sync trait keeps the crawler hot-path zero-await.

use crate::proxy::ProxyOutcome;
use std::borrow::Cow;
use std::fmt;
use url::Url;

/// The provider kinds we ship scaffolds for. Parsed from the
/// `--residential-provider` CLI flag (see operator docs) and the
/// `CRAWLEX_RES_PROVIDER` env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentialProviderKind {
    /// No residential pool — fall back to the datacenter list in
    /// `ProxyRouter`. Default for every CLI invocation.
    None,
    BrightData,
    Oxylabs,
    IPRoyal,
}

impl ResidentialProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::BrightData => "brightdata",
            Self::Oxylabs => "oxylabs",
            Self::IPRoyal => "iproyal",
        }
    }
}

impl fmt::Display for ResidentialProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ResidentialProviderKind {
    type Err = ResidentialError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "" => Ok(Self::None),
            "brightdata" | "bright-data" | "luminati" => Ok(Self::BrightData),
            "oxylabs" => Ok(Self::Oxylabs),
            "iproyal" | "ip-royal" => Ok(Self::IPRoyal),
            other => Err(ResidentialError::UnknownProvider(other.to_string())),
        }
    }
}

/// Errors surfaced by scaffold adapters.
#[derive(Debug, Clone)]
pub enum ResidentialError {
    /// Operator selected a provider but did not configure credentials.
    /// Scaffold adapters always raise this until the real impl lands.
    ProviderNotConfigured(&'static str),
    /// CLI / env value did not map to a known provider.
    UnknownProvider(String),
    /// Upstream gateway rejected the request (real impl only — reserved).
    Upstream(Cow<'static, str>),
}

impl fmt::Display for ResidentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderNotConfigured(p) => {
                write!(f, "residential provider `{p}` selected but not configured")
            }
            Self::UnknownProvider(p) => write!(f, "unknown residential provider `{p}`"),
            Self::Upstream(m) => write!(f, "residential provider upstream error: {m}"),
        }
    }
}

impl std::error::Error for ResidentialError {}

/// Trait every residential-pool adapter implements. Kept object-safe so the
/// crawler can hold a `Arc<dyn ResidentialProvider>` without generics
/// leaking into the scheduler.
pub trait ResidentialProvider: Send + Sync {
    /// Human-readable name for logs / metrics.
    fn name(&self) -> &'static str;

    /// Request a proxy URL suitable for reaching `host`. Providers that
    /// support geo pinning can inspect the host TLD / IP-intel hint to
    /// choose an exit country; scaffold adapters ignore it.
    ///
    /// Returns `ProviderNotConfigured` for stub adapters so the caller can
    /// transparently fall back to the datacenter `ProxyRouter`.
    fn rotate(&self, host: &str) -> Result<Url, ResidentialError>;

    /// Feed router outcomes back to the provider so it can retire burned
    /// sessions proactively (e.g. BrightData session TTL shortcut). Default
    /// impl is a no-op — providers override only when useful.
    fn report_outcome(&self, _proxy: &Url, _outcome: ProxyOutcome) {}
}

/// BrightData (née Luminati) adapter stub.
///
/// Real impl talks to `brd.superproxy.io:22225` with a session-pinned
/// username (`brd-customer-<id>-zone-<zone>-session-<rand>`). The `rotate`
/// call would mint a fresh session string every N minutes or on `ChallengeHit`.
#[derive(Debug, Default)]
pub struct BrightDataStub;

impl ResidentialProvider for BrightDataStub {
    fn name(&self) -> &'static str {
        "brightdata-stub"
    }
    fn rotate(&self, _host: &str) -> Result<Url, ResidentialError> {
        Err(ResidentialError::ProviderNotConfigured("brightdata"))
    }
}

/// Oxylabs residential adapter stub.
///
/// Real impl targets `pr.oxylabs.io:7777` with the
/// `customer-<user>-cc-<country>-sessid-<n>` username format.
#[derive(Debug, Default)]
pub struct OxylabsStub;

impl ResidentialProvider for OxylabsStub {
    fn name(&self) -> &'static str {
        "oxylabs-stub"
    }
    fn rotate(&self, _host: &str) -> Result<Url, ResidentialError> {
        Err(ResidentialError::ProviderNotConfigured("oxylabs"))
    }
}

/// IPRoyal residential adapter stub.
///
/// Real impl targets `geo.iproyal.com:12321` with the
/// `<user>:<pass>_country-<cc>_session-<n>` format.
#[derive(Debug, Default)]
pub struct IPRoyalStub;

impl ResidentialProvider for IPRoyalStub {
    fn name(&self) -> &'static str {
        "iproyal-stub"
    }
    fn rotate(&self, _host: &str) -> Result<Url, ResidentialError> {
        Err(ResidentialError::ProviderNotConfigured("iproyal"))
    }
}

/// Convenience constructor used by the (future) CLI wire-up. Returns
/// `None` for `ResidentialProviderKind::None` so callers can write
/// `if let Some(p) = build_provider(kind) { … }`.
pub fn build_provider(kind: ResidentialProviderKind) -> Option<Box<dyn ResidentialProvider>> {
    match kind {
        ResidentialProviderKind::None => None,
        ResidentialProviderKind::BrightData => Some(Box::new(BrightDataStub)),
        ResidentialProviderKind::Oxylabs => Some(Box::new(OxylabsStub)),
        ResidentialProviderKind::IPRoyal => Some(Box::new(IPRoyalStub)),
    }
}

/// Environment variables read by real adapters (documented here so the
/// operator doc and the code stay in sync).
pub mod env {
    /// Selects provider at runtime. Same value space as the CLI flag.
    pub const CRAWLEX_RES_PROVIDER: &str = "CRAWLEX_RES_PROVIDER";
    /// BrightData customer id / zone / password triple.
    pub const CRAWLEX_RES_PROXY_BRIGHTDATA_USER: &str = "CRAWLEX_RES_PROXY_BRIGHTDATA_USER";
    pub const CRAWLEX_RES_PROXY_BRIGHTDATA_PASS: &str = "CRAWLEX_RES_PROXY_BRIGHTDATA_PASS";
    pub const CRAWLEX_RES_PROXY_BRIGHTDATA_ZONE: &str = "CRAWLEX_RES_PROXY_BRIGHTDATA_ZONE";
    /// Oxylabs credentials.
    pub const CRAWLEX_RES_PROXY_OXYLABS_USER: &str = "CRAWLEX_RES_PROXY_OXYLABS_USER";
    pub const CRAWLEX_RES_PROXY_OXYLABS_PASS: &str = "CRAWLEX_RES_PROXY_OXYLABS_PASS";
    /// IPRoyal credentials.
    pub const CRAWLEX_RES_PROXY_IPROYAL_USER: &str = "CRAWLEX_RES_PROXY_IPROYAL_USER";
    pub const CRAWLEX_RES_PROXY_IPROYAL_PASS: &str = "CRAWLEX_RES_PROXY_IPROYAL_PASS";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_known_providers() {
        assert_eq!(
            ResidentialProviderKind::from_str("brightdata").unwrap(),
            ResidentialProviderKind::BrightData
        );
        assert_eq!(
            ResidentialProviderKind::from_str("oxylabs").unwrap(),
            ResidentialProviderKind::Oxylabs
        );
        assert_eq!(
            ResidentialProviderKind::from_str("iproyal").unwrap(),
            ResidentialProviderKind::IPRoyal
        );
        assert_eq!(
            ResidentialProviderKind::from_str("none").unwrap(),
            ResidentialProviderKind::None
        );
    }

    #[test]
    fn rejects_unknown_provider() {
        assert!(ResidentialProviderKind::from_str("bogus").is_err());
    }

    #[test]
    fn stubs_return_not_configured() {
        let p = BrightDataStub;
        let err = p.rotate("example.com").unwrap_err();
        assert!(matches!(err, ResidentialError::ProviderNotConfigured(_)));
    }

    #[test]
    fn build_provider_none_yields_none() {
        assert!(build_provider(ResidentialProviderKind::None).is_none());
        assert!(build_provider(ResidentialProviderKind::BrightData).is_some());
    }
}
