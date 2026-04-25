//! Captcha solver plug-in point — SCAFFOLD (issue #38).
//!
//! Defines the `CaptchaSolver` trait + stub adapters for commercial
//! captcha services (`2captcha`, `AntiCaptcha`) and VLM-based solvers
//! (OpenAI / Anthropic vision models). No real HTTP calls are made in
//! this wave — adapters return
//! `SolverError::AdapterNotConfigured("<name>")` so the policy engine
//! can cleanly fall back to the existing "avoidance" challenge mode.
//!
//! Crawlex policy remains **prevention-first** — solvers are an operator
//! opt-in, documented in `docs/infra-tier-operator.md`. Default CLI
//! behaviour keeps the solver disabled (`--captcha-solver none`) and
//! every adapter refuses to answer unless the operator explicitly wires
//! API credentials.
//!
//! The trait is async because real solver flows are long-poll style
//! (2captcha's balance check + submit + poll-until-ready can take 30s+).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::antibot::ChallengeVendor;

/// Kinds of solver adapters we ship scaffolds for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SolverKind {
    /// Default — solver entirely disabled.
    None,
    TwoCaptcha,
    AntiCaptcha,
    /// Vision-language model (prompt-engineered image reasoning).
    Vlm,
}

impl SolverKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::TwoCaptcha => "2captcha",
            Self::AntiCaptcha => "anticaptcha",
            Self::Vlm => "vlm",
        }
    }
}

impl std::str::FromStr for SolverKind {
    type Err = SolverError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "" => Ok(Self::None),
            "2captcha" | "twocaptcha" => Ok(Self::TwoCaptcha),
            "anticaptcha" | "anti-captcha" => Ok(Self::AntiCaptcha),
            "vlm" | "openai" | "anthropic" => Ok(Self::Vlm),
            other => Err(SolverError::UnknownAdapter(other.to_string())),
        }
    }
}

/// Data the solver needs to attempt the challenge. Shaped so the caller
/// can build it from either an HTML scrape (sitekey + url) or a fully
/// rendered page (screenshot bytes).
#[derive(Debug, Clone)]
pub struct ChallengePayload {
    pub vendor: ChallengeVendor,
    pub url: url::Url,
    pub sitekey: Option<String>,
    pub action: Option<String>,
    pub iframe_srcs: Vec<String>,
    pub screenshot_png: Option<Vec<u8>>,
}

/// What the adapter produces on success. Token shape varies per vendor —
/// the scheduler is responsible for injecting it back into the page
/// (e.g. filling `g-recaptcha-response`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolveResult {
    pub token: String,
    pub elapsed_ms: u64,
    pub adapter: &'static str,
}

/// Adapter-scoped error surface.
#[derive(Debug, Clone)]
pub enum SolverError {
    AdapterNotConfigured(&'static str),
    UnknownAdapter(String),
    UnsupportedVendor {
        adapter: &'static str,
        vendor: ChallengeVendor,
    },
    Upstream(String),
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdapterNotConfigured(a) => {
                write!(f, "captcha solver `{a}` selected but not configured")
            }
            Self::UnknownAdapter(a) => write!(f, "unknown captcha solver adapter `{a}`"),
            Self::UnsupportedVendor { adapter, vendor } => {
                write!(
                    f,
                    "solver `{adapter}` does not handle vendor `{}`",
                    vendor.as_str()
                )
            }
            Self::Upstream(m) => write!(f, "captcha solver upstream error: {m}"),
        }
    }
}

impl std::error::Error for SolverError {}

/// The plug-in point. Object-safe; held behind `Arc<dyn CaptchaSolver>`.
#[async_trait]
pub trait CaptchaSolver: Send + Sync {
    fn name(&self) -> &'static str;
    /// Which vendors this adapter can answer. Used by the scheduler to
    /// route challenges to the right solver (e.g. 2captcha handles
    /// recaptcha + hcaptcha but not DataDome).
    fn supported_vendors(&self) -> &'static [ChallengeVendor];

    async fn solve(&self, challenge: ChallengePayload) -> Result<SolveResult, SolverError>;
}

/// 2captcha.com adapter stub.
///
/// Real impl posts to `https://2captcha.com/in.php` + polls
/// `res.php?action=get&id=<id>`. Env: `CRAWLEX_SOLVER_2CAPTCHA_KEY`.
#[derive(Debug, Default)]
pub struct TwoCaptchaAdapter;

#[async_trait]
impl CaptchaSolver for TwoCaptchaAdapter {
    fn name(&self) -> &'static str {
        "2captcha-stub"
    }
    fn supported_vendors(&self) -> &'static [ChallengeVendor] {
        &[
            ChallengeVendor::Recaptcha,
            ChallengeVendor::RecaptchaEnterprise,
            ChallengeVendor::HCaptcha,
            ChallengeVendor::CloudflareTurnstile,
        ]
    }
    async fn solve(&self, _c: ChallengePayload) -> Result<SolveResult, SolverError> {
        Err(SolverError::AdapterNotConfigured("2captcha"))
    }
}

/// anti-captcha.com adapter stub.
///
/// Real impl targets the JSON-RPC API at `https://api.anti-captcha.com/`.
/// Env: `CRAWLEX_SOLVER_ANTICAPTCHA_KEY`.
#[derive(Debug, Default)]
pub struct AntiCaptchaAdapter;

#[async_trait]
impl CaptchaSolver for AntiCaptchaAdapter {
    fn name(&self) -> &'static str {
        "anticaptcha-stub"
    }
    fn supported_vendors(&self) -> &'static [ChallengeVendor] {
        &[
            ChallengeVendor::Recaptcha,
            ChallengeVendor::RecaptchaEnterprise,
            ChallengeVendor::HCaptcha,
        ]
    }
    async fn solve(&self, _c: ChallengePayload) -> Result<SolveResult, SolverError> {
        Err(SolverError::AdapterNotConfigured("anticaptcha"))
    }
}

/// Vision-language-model adapter stub.
///
/// Real impl uploads the screenshot + a prompt ("identify the bus tiles,
/// return comma-separated indices") to an OpenAI / Anthropic vision
/// endpoint, parses the response, and synthesises a token by driving the
/// browser (via CDP) to click the named tiles. That last step makes the
/// VLM adapter fundamentally different from 2captcha — it returns a
/// `token` produced by *driving the real browser*, not a server-issued
/// solve blob. Env:
/// * `CRAWLEX_SOLVER_VLM_PROVIDER` = `openai` | `anthropic`
/// * `CRAWLEX_SOLVER_VLM_API_KEY`
/// * `CRAWLEX_SOLVER_VLM_MODEL` (optional, provider-specific default)
#[derive(Debug, Default)]
pub struct VlmAdapter;

#[async_trait]
impl CaptchaSolver for VlmAdapter {
    fn name(&self) -> &'static str {
        "vlm-stub"
    }
    fn supported_vendors(&self) -> &'static [ChallengeVendor] {
        // VLMs can in principle try any visual challenge; real impl
        // narrows by confidence. Scaffold advertises the common ones.
        &[
            ChallengeVendor::Recaptcha,
            ChallengeVendor::HCaptcha,
            ChallengeVendor::GenericCaptcha,
        ]
    }
    async fn solve(&self, _c: ChallengePayload) -> Result<SolveResult, SolverError> {
        Err(SolverError::AdapterNotConfigured("vlm"))
    }
}

/// Env vars consulted by the real adapters — centralised so the operator
/// doc and the adapters stay in sync.
pub mod env {
    pub const CRAWLEX_SOLVER: &str = "CRAWLEX_SOLVER";
    pub const CRAWLEX_SOLVER_2CAPTCHA_KEY: &str = "CRAWLEX_SOLVER_2CAPTCHA_KEY";
    pub const CRAWLEX_SOLVER_ANTICAPTCHA_KEY: &str = "CRAWLEX_SOLVER_ANTICAPTCHA_KEY";
    pub const CRAWLEX_SOLVER_VLM_PROVIDER: &str = "CRAWLEX_SOLVER_VLM_PROVIDER";
    pub const CRAWLEX_SOLVER_VLM_API_KEY: &str = "CRAWLEX_SOLVER_VLM_API_KEY";
    pub const CRAWLEX_SOLVER_VLM_MODEL: &str = "CRAWLEX_SOLVER_VLM_MODEL";
}

/// Factory used by the (future) CLI wire-up.
pub fn build_solver(kind: SolverKind) -> Option<Box<dyn CaptchaSolver>> {
    match kind {
        SolverKind::None => None,
        SolverKind::TwoCaptcha => Some(Box::new(TwoCaptchaAdapter)),
        SolverKind::AntiCaptcha => Some(Box::new(AntiCaptchaAdapter)),
        SolverKind::Vlm => Some(Box::new(VlmAdapter)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn payload(vendor: ChallengeVendor) -> ChallengePayload {
        ChallengePayload {
            vendor,
            url: url::Url::parse("https://example.com/").unwrap(),
            sitekey: Some("fake-sitekey".into()),
            action: None,
            iframe_srcs: vec![],
            screenshot_png: None,
        }
    }

    #[test]
    fn parses_known_adapters() {
        assert_eq!(
            SolverKind::from_str("2captcha").unwrap(),
            SolverKind::TwoCaptcha
        );
        assert_eq!(
            SolverKind::from_str("anticaptcha").unwrap(),
            SolverKind::AntiCaptcha
        );
        assert_eq!(SolverKind::from_str("vlm").unwrap(), SolverKind::Vlm);
        assert_eq!(SolverKind::from_str("none").unwrap(), SolverKind::None);
        assert!(SolverKind::from_str("bogus").is_err());
    }

    #[tokio::test]
    async fn twocaptcha_stub_refuses() {
        let a = TwoCaptchaAdapter;
        let err = a
            .solve(payload(ChallengeVendor::Recaptcha))
            .await
            .unwrap_err();
        assert!(matches!(err, SolverError::AdapterNotConfigured(_)));
    }

    #[tokio::test]
    async fn vlm_stub_refuses() {
        let a = VlmAdapter;
        let err = a
            .solve(payload(ChallengeVendor::HCaptcha))
            .await
            .unwrap_err();
        assert!(matches!(err, SolverError::AdapterNotConfigured(_)));
    }

    #[test]
    fn build_solver_honours_none() {
        assert!(build_solver(SolverKind::None).is_none());
        assert!(build_solver(SolverKind::TwoCaptcha).is_some());
    }

    #[test]
    fn supported_vendors_advertised() {
        let a = TwoCaptchaAdapter;
        assert!(a.supported_vendors().contains(&ChallengeVendor::Recaptcha));
    }
}
