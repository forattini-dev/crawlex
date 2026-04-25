//! DNS-over-HTTPS (DoH) configuration scaffold.
//!
//! Status: **opt-in, default OFF**. This module carries the configuration
//! surface (provider selection, env var, CLI helper) so the rest of the
//! stack can thread a `DohConfig` through without the actual transport
//! being wired yet. The transport switch itself (hickory's
//! `dns-over-https-rustls` feature) requires pulling in the rustls TLS
//! stack on top of the boringssl stack the crate already uses for HTTP —
//! doing that drive-by would bloat compile time and binary size, both of
//! which the mini build is explicitly tuned against.
//!
//! What you get today:
//! * `DohProvider` enum with the common public resolvers.
//! * `DohConfig::from_env()` / `DohConfig::parse(..)` so the CLI and
//!   programmatic callers agree on the same knob.
//! * `ensure_url()` returns the canonical DoH endpoint URL so an
//!   operator can point their own resolver at it if they're running
//!   hickory with DoH enabled via an external crate build.
//!
//! What flipping this on will later do (when the feature is wired):
//! * Swap the system-stub resolver in `dns_cache` + `discovery::dns`
//!   for a hickory resolver configured with
//!   `ResolverConfig::cloudflare_https()` (or equivalent per provider).
//! * Bypass the host's `/etc/resolv.conf`, so the ISP's logging
//!   resolver never sees the crawl queries.
//!
//! # Why default OFF
//!
//! A crawl that switches DNS transport mid-flight changes its network
//! signature in ways the operator may not have asked for. DoH is a
//! privacy/stealth feature, not a correctness feature — opt-in is the
//! honest default.

use url::Url;

/// Known public DoH providers. `Off` means "use the system stub
/// resolver" — the existing hickory-via-`builder_tokio` behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DohProvider {
    Off,
    Cloudflare,
    Google,
    Quad9,
    /// Operator-supplied endpoint, stored separately because we don't
    /// want to enum-explode on URLs.
    Custom,
}

impl DohProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            DohProvider::Off => "off",
            DohProvider::Cloudflare => "cloudflare",
            DohProvider::Google => "google",
            DohProvider::Quad9 => "quad9",
            DohProvider::Custom => "custom",
        }
    }

    /// Canonical DoH endpoint URL for the public providers. Returns
    /// `None` for `Off` and `Custom` — callers supply their own URL
    /// for Custom via `DohConfig::custom_url`.
    pub fn default_url(self) -> Option<&'static str> {
        match self {
            DohProvider::Cloudflare => Some("https://cloudflare-dns.com/dns-query"),
            DohProvider::Google => Some("https://dns.google/dns-query"),
            DohProvider::Quad9 => Some("https://dns.quad9.net/dns-query"),
            DohProvider::Off | DohProvider::Custom => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DohConfig {
    pub provider: DohProvider,
    pub custom_url: Option<Url>,
}

impl Default for DohConfig {
    fn default() -> Self {
        Self {
            provider: DohProvider::Off,
            custom_url: None,
        }
    }
}

impl DohConfig {
    /// Parse the CLI `--doh <value>` argument. Accepts the provider
    /// name (`cloudflare`, `google`, `quad9`, `off`) or an https URL
    /// which is treated as a Custom endpoint.
    pub fn parse(value: &str) -> Result<Self, String> {
        let v = value.trim();
        if v.is_empty() || v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("system") {
            return Ok(Self::default());
        }
        match v.to_ascii_lowercase().as_str() {
            "cloudflare" | "cf" | "1.1.1.1" => Ok(Self {
                provider: DohProvider::Cloudflare,
                custom_url: None,
            }),
            "google" | "8.8.8.8" => Ok(Self {
                provider: DohProvider::Google,
                custom_url: None,
            }),
            "quad9" | "9.9.9.9" => Ok(Self {
                provider: DohProvider::Quad9,
                custom_url: None,
            }),
            _ => {
                let u = Url::parse(v)
                    .map_err(|e| format!("invalid --doh value (not a provider or URL): {e}"))?;
                if u.scheme() != "https" {
                    return Err("--doh custom URL must use https://".into());
                }
                Ok(Self {
                    provider: DohProvider::Custom,
                    custom_url: Some(u),
                })
            }
        }
    }

    /// Read `CRAWLEX_DOH` from the environment, default Off when unset.
    /// Intended as a low-friction knob for container deployments where
    /// the caller can't easily pass a CLI flag.
    pub fn from_env() -> Self {
        match std::env::var("CRAWLEX_DOH") {
            Ok(v) => Self::parse(&v).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Whether DoH is actually enabled (provider != Off). Existing
    /// call-sites use this as the gate before trying to build a
    /// hickory DoH resolver.
    pub fn is_enabled(&self) -> bool {
        !matches!(self.provider, DohProvider::Off)
    }

    /// Endpoint URL the caller should hit — either the provider's
    /// canonical URL or the custom override. `None` when DoH is off.
    pub fn endpoint_url(&self) -> Option<Url> {
        match self.provider {
            DohProvider::Off => None,
            DohProvider::Custom => self.custom_url.clone(),
            other => other.default_url().and_then(|s| Url::parse(s).ok()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off() {
        let c = DohConfig::default();
        assert!(!c.is_enabled());
        assert!(c.endpoint_url().is_none());
    }

    #[test]
    fn parse_provider_aliases() {
        assert_eq!(
            DohConfig::parse("cloudflare").unwrap().provider,
            DohProvider::Cloudflare
        );
        assert_eq!(
            DohConfig::parse("1.1.1.1").unwrap().provider,
            DohProvider::Cloudflare
        );
        assert_eq!(
            DohConfig::parse("Google").unwrap().provider,
            DohProvider::Google
        );
        assert_eq!(
            DohConfig::parse("quad9").unwrap().provider,
            DohProvider::Quad9
        );
        assert_eq!(DohConfig::parse("off").unwrap().provider, DohProvider::Off);
        assert_eq!(DohConfig::parse("").unwrap().provider, DohProvider::Off);
    }

    #[test]
    fn parse_custom_url() {
        let c = DohConfig::parse("https://doh.example.test/dns-query").unwrap();
        assert_eq!(c.provider, DohProvider::Custom);
        assert_eq!(
            c.endpoint_url().unwrap().as_str(),
            "https://doh.example.test/dns-query"
        );
    }

    #[test]
    fn rejects_non_https_custom() {
        let err = DohConfig::parse("http://nope.test/dns-query").unwrap_err();
        assert!(err.contains("https"));
    }

    #[test]
    fn endpoint_url_cloudflare_default() {
        let c = DohConfig::parse("cloudflare").unwrap();
        assert_eq!(
            c.endpoint_url().unwrap().as_str(),
            "https://cloudflare-dns.com/dns-query"
        );
    }
}
