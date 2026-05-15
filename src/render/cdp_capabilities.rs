//! Slice 31 — endpoint capability detection for the external CDP
//! provider. Keeps the public API vendor-neutral: we model what an
//! endpoint *supports*, not which vendor it is.
//!
//! Two endpoint kinds today:
//!
//!   * `GenericCdp`     — plain Chromium DevTools (`chrome --remote-debugging-port`).
//!                        crawlex must not assume any identity controls
//!                        on the connection URL.
//!   * `NativeStealth`  — cloakserve-like multiplexer that accepts
//!                        identity constraints (seed, timezone, locale,
//!                        proxy, geoip) as URL query parameters before
//!                        the WebSocket upgrade.
//!
//! Detection is a pure function of a successful `/json/version` probe
//! (`ProbeOk`). Probe failures are handled upstream and do not reach
//! this module — when the probe succeeds but no native-stealth signal
//! is present we fall back to `GenericCdp`, which is always safe.

use super::cdp_probe::ProbeOk;

/// Family of an external CDP endpoint, as inferred from its
/// `/json/version` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    GenericCdp,
    NativeStealth,
}

impl EndpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EndpointKind::GenericCdp => "generic_cdp",
            EndpointKind::NativeStealth => "native_stealth",
        }
    }
}

/// What an endpoint advertises beyond plain CDP. Stored on the render
/// pool so identity hints can be re-applied per session without
/// re-probing.
#[derive(Debug, Clone)]
pub struct EndpointCapabilities {
    pub kind: EndpointKind,
    /// True when the endpoint accepts identity constraints as URL
    /// query parameters on the HTTP CDP base URL.
    pub identity_params: bool,
    /// Vendor banner for telemetry. `None` for plain Chromium.
    pub vendor: Option<String>,
}

impl EndpointCapabilities {
    /// Plain Chromium DevTools — no extra capabilities.
    pub fn generic() -> Self {
        Self {
            kind: EndpointKind::GenericCdp,
            identity_params: false,
            vendor: None,
        }
    }

    /// Decide capabilities from a successful probe. Heuristic:
    ///
    ///   * explicit `Stealth-Provider` field → native-stealth, vendor = field;
    ///   * `Browser` banner starts with `cloakserve/` → native-stealth, vendor = banner;
    ///   * otherwise → generic CDP.
    ///
    /// This keeps the rule small and observable: a vendor that wants to
    /// opt in only needs to set one of those two fields on its
    /// `/json/version` payload. No brand-specific control flow elsewhere.
    pub fn detect(probe: &ProbeOk) -> Self {
        if !probe.stealth_provider.trim().is_empty() {
            return Self {
                kind: EndpointKind::NativeStealth,
                identity_params: true,
                vendor: Some(probe.stealth_provider.trim().to_string()),
            };
        }
        let banner = probe.browser.trim();
        let banner_lc = banner.to_ascii_lowercase();
        if banner_lc.starts_with("cloakserve/") || banner_lc.starts_with("cloakserve ") {
            return Self {
                kind: EndpointKind::NativeStealth,
                identity_params: true,
                vendor: Some(banner.to_string()),
            };
        }
        Self::generic()
    }

    /// Build the connection URL crawlex passes to chromiumoxide. For
    /// generic endpoints this returns `base` unchanged. For
    /// identity-aware endpoints it appends populated hints as query
    /// parameters, preserving any existing query string on `base`.
    pub fn build_connect_url(
        &self,
        base: &str,
        hints: &IdentityHints<'_>,
    ) -> Result<String, String> {
        if !self.identity_params || hints.is_empty() {
            return Ok(base.to_string());
        }
        let mut url = url::Url::parse(base)
            .map_err(|e| format!("external CDP url is not parseable (`{base}`): {e}"))?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(v) = hints.seed {
                q.append_pair("seed", v);
            }
            if let Some(v) = hints.timezone {
                q.append_pair("timezone", v);
            }
            if let Some(v) = hints.locale {
                q.append_pair("locale", v);
            }
            if let Some(v) = hints.proxy {
                q.append_pair("proxy", v);
            }
            if let Some(v) = hints.geoip {
                q.append_pair("geoip", v);
            }
        }
        Ok(url.to_string())
    }
}

/// High-level identity constraints crawlex may forward to a
/// native-stealth endpoint. All fields optional; missing fields are
/// simply omitted from the connection URL.
#[derive(Debug, Default, Clone, Copy)]
pub struct IdentityHints<'a> {
    pub seed: Option<&'a str>,
    pub timezone: Option<&'a str>,
    pub locale: Option<&'a str>,
    pub proxy: Option<&'a str>,
    pub geoip: Option<&'a str>,
}

impl<'a> IdentityHints<'a> {
    pub fn is_empty(&self) -> bool {
        self.seed.is_none()
            && self.timezone.is_none()
            && self.locale.is_none()
            && self.proxy.is_none()
            && self.geoip.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(browser: &str, stealth: &str) -> ProbeOk {
        ProbeOk {
            web_socket_debugger_url: "ws://x/y".to_string(),
            browser: browser.to_string(),
            stealth_provider: stealth.to_string(),
        }
    }

    #[test]
    fn detect_generic_chrome_banner() {
        let cap = EndpointCapabilities::detect(&probe("Chrome/149.0.0.0", ""));
        assert_eq!(cap.kind, EndpointKind::GenericCdp);
        assert!(!cap.identity_params);
        assert!(cap.vendor.is_none());
    }

    #[test]
    fn detect_cloakserve_browser_banner() {
        let cap = EndpointCapabilities::detect(&probe("cloakserve/0.4.2 (chrome 149)", ""));
        assert_eq!(cap.kind, EndpointKind::NativeStealth);
        assert!(cap.identity_params);
        assert_eq!(cap.vendor.as_deref(), Some("cloakserve/0.4.2 (chrome 149)"));
    }

    #[test]
    fn detect_explicit_stealth_provider_wins_over_chrome_banner() {
        let cap = EndpointCapabilities::detect(&probe("Chrome/149", "stealth-cdp/1.0"));
        assert_eq!(cap.kind, EndpointKind::NativeStealth);
        assert!(cap.identity_params);
        assert_eq!(cap.vendor.as_deref(), Some("stealth-cdp/1.0"));
    }

    #[test]
    fn detect_unknown_browser_falls_back_to_generic() {
        let cap = EndpointCapabilities::detect(&probe("Other/1.0", ""));
        assert_eq!(cap.kind, EndpointKind::GenericCdp);
        assert!(!cap.identity_params);
    }

    #[test]
    fn build_url_generic_returns_base_unchanged_even_with_hints() {
        let cap = EndpointCapabilities::generic();
        let hints = IdentityHints {
            seed: Some("abc"),
            timezone: Some("UTC"),
            ..IdentityHints::default()
        };
        let out = cap
            .build_connect_url("http://127.0.0.1:9222", &hints)
            .unwrap();
        assert_eq!(out, "http://127.0.0.1:9222");
    }

    #[test]
    fn build_url_native_stealth_appends_only_set_hints() {
        let cap = EndpointCapabilities {
            kind: EndpointKind::NativeStealth,
            identity_params: true,
            vendor: Some("cloakserve/x".to_string()),
        };
        let hints = IdentityHints {
            seed: Some("session-42"),
            timezone: Some("Europe/Lisbon"),
            locale: Some("pt-PT"),
            proxy: Some("http://user:pass@proxy.example:3128"),
            geoip: Some("PT"),
        };
        let out = cap
            .build_connect_url("http://stealth.example:9222", &hints)
            .unwrap();
        let parsed = url::Url::parse(&out).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q.get("seed").map(String::as_str), Some("session-42"));
        assert_eq!(q.get("timezone").map(String::as_str), Some("Europe/Lisbon"));
        assert_eq!(q.get("locale").map(String::as_str), Some("pt-PT"));
        assert_eq!(
            q.get("proxy").map(String::as_str),
            Some("http://user:pass@proxy.example:3128"),
        );
        assert_eq!(q.get("geoip").map(String::as_str), Some("PT"));
    }

    #[test]
    fn build_url_empty_hints_returns_base_unchanged() {
        let cap = EndpointCapabilities {
            kind: EndpointKind::NativeStealth,
            identity_params: true,
            vendor: None,
        };
        let out = cap
            .build_connect_url("http://stealth.example:9222", &IdentityHints::default())
            .unwrap();
        assert_eq!(out, "http://stealth.example:9222");
    }

    #[test]
    fn build_url_preserves_existing_query() {
        let cap = EndpointCapabilities {
            kind: EndpointKind::NativeStealth,
            identity_params: true,
            vendor: None,
        };
        let hints = IdentityHints {
            seed: Some("s1"),
            ..IdentityHints::default()
        };
        let out = cap
            .build_connect_url("http://stealth.example:9222/?token=abc", &hints)
            .unwrap();
        let parsed = url::Url::parse(&out).unwrap();
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q.get("token").map(String::as_str), Some("abc"));
        assert_eq!(q.get("seed").map(String::as_str), Some("s1"));
    }
}
