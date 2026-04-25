//! `CaptchaSolver` trait implementation for the in-house reCAPTCHA v3
//! invisible solver. Plugs the new server-side path into the existing
//! solver dispatch (`SolverKind` / `build_solver`) so callers don't need
//! to know about `recaptcha::solve` directly.

use async_trait::async_trait;
use std::time::Instant;

use crate::antibot::solver::{CaptchaSolver, ChallengePayload, SolveResult, SolverError};
use crate::antibot::ChallengeVendor;

use super::solver::{solve, SolveRequest};

/// Server-side reCAPTCHA v3 invisible solver. No browser required.
///
/// Configuration: none — this adapter is self-contained. Callers can
/// optionally inject an `IdentityBundle` via the policy/router layer to
/// improve coherence; without one we fall back to vanilla Chrome 136
/// Windows defaults.
#[derive(Debug, Default)]
pub struct RecaptchaInvisibleAdapter {
    /// Optional HTTP/HTTPS proxy. When set, all 3 hops (api.js, anchor,
    /// reload) go through it. Persona / proxy coherence is the caller's
    /// responsibility — pass the same proxy that the rest of the session
    /// uses.
    proxy_url: Option<String>,
}

impl RecaptchaInvisibleAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_proxy(mut self, proxy_url: impl Into<String>) -> Self {
        self.proxy_url = Some(proxy_url.into());
        self
    }
}

#[async_trait]
impl CaptchaSolver for RecaptchaInvisibleAdapter {
    fn name(&self) -> &'static str {
        "recaptcha-invisible"
    }

    fn supported_vendors(&self) -> &'static [ChallengeVendor] {
        // Server-side replay only handles plain reCAPTCHA v3. Enterprise
        // requires anchor-with-action verification we don't currently
        // synthesise; HCaptcha / Turnstile / DataDome are different
        // protocols entirely.
        &[ChallengeVendor::Recaptcha]
    }

    async fn solve(&self, c: ChallengePayload) -> Result<SolveResult, SolverError> {
        if !self.supported_vendors().contains(&c.vendor) {
            return Err(SolverError::UnsupportedVendor {
                adapter: "recaptcha-invisible",
                vendor: c.vendor,
            });
        }
        let site_key = c
            .sitekey
            .as_deref()
            .ok_or_else(|| SolverError::Upstream("missing sitekey".to_string()))?;
        let action = c.action.as_deref().unwrap_or("submit");

        let started = Instant::now();
        let req = SolveRequest {
            site_key,
            site_url: &c.url,
            action,
            // No bundle plumbed through `ChallengePayload` yet — wire it
            // when we extend the payload struct. Adapter still works,
            // just falls back to vanilla Chrome 136 defaults inside the
            // solver.
            bundle: None,
        };

        match solve(req, self.proxy_url.as_deref()).await {
            Ok(out) => Ok(SolveResult {
                token: out.token,
                elapsed_ms: out.elapsed_ms,
                adapter: "recaptcha-invisible",
            }),
            Err(e) => {
                // Convert the local error type into the trait's wider
                // `SolverError::Upstream` so the policy engine sees a
                // consistent surface. We also log the full chain at debug
                // to keep operator triage tractable without leaking
                // implementation details into the public error.
                tracing::debug!(
                    target: "antibot::recaptcha",
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = ?e,
                    "recaptcha invisible solve failed",
                );
                Err(SolverError::Upstream(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn payload(vendor: ChallengeVendor, sitekey: Option<&str>) -> ChallengePayload {
        ChallengePayload {
            vendor,
            url: Url::parse("https://example.com/login").unwrap(),
            sitekey: sitekey.map(String::from),
            action: Some("login".into()),
            iframe_srcs: vec![],
            screenshot_png: None,
        }
    }

    #[test]
    fn name_is_stable_identifier() {
        let a = RecaptchaInvisibleAdapter::new();
        assert_eq!(a.name(), "recaptcha-invisible");
    }

    #[test]
    fn only_vanilla_recaptcha_supported() {
        let a = RecaptchaInvisibleAdapter::new();
        let v = a.supported_vendors();
        assert!(v.contains(&ChallengeVendor::Recaptcha));
        assert!(!v.contains(&ChallengeVendor::RecaptchaEnterprise));
        assert!(!v.contains(&ChallengeVendor::HCaptcha));
        assert!(!v.contains(&ChallengeVendor::CloudflareTurnstile));
    }

    #[tokio::test]
    async fn refuses_unsupported_vendor() {
        let a = RecaptchaInvisibleAdapter::new();
        let err = a
            .solve(payload(ChallengeVendor::HCaptcha, Some("k")))
            .await
            .unwrap_err();
        assert!(matches!(err, SolverError::UnsupportedVendor { .. }));
    }

    #[tokio::test]
    async fn requires_sitekey() {
        let a = RecaptchaInvisibleAdapter::new();
        let err = a
            .solve(payload(ChallengeVendor::Recaptcha, None))
            .await
            .unwrap_err();
        // Without a sitekey we surface as `Upstream` per trait contract.
        assert!(matches!(err, SolverError::Upstream(_)));
    }

    #[test]
    fn with_proxy_attaches_url() {
        let a = RecaptchaInvisibleAdapter::new().with_proxy("http://user:pass@proxy.example:8080");
        assert!(a.proxy_url.is_some());
    }
}
