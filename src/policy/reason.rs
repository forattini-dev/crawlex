//! Decision / DecisionReason — the output of every `PolicyEngine::decide`
//! call. Serializable so they round-trip through NDJSON events and the
//! `decision_log` SQLite table (phase 5).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Fetch via HTTP spoof engine.
    Http,
    /// Fetch via headless Chromium.
    Render,
    /// Re-enqueue after the given delay.
    Retry { after_ms: u64 },
    /// Ask the ProxyRouter for a different proxy, then re-run.
    SwitchProxy,
    /// Push back to the queue for later (host cool-down, budget pressure).
    Defer { until_ms: u64 },
    /// Don't process — give up permanently.
    Drop,
    /// Collect artifacts (screenshot, full trace) because something smells off.
    CollectArtifacts,
    /// Increase per-request observability (vitals, network log) for this URL.
    IncreaseObservability,
    /// Pause the job and prompt the operator — see
    /// `src/render/handoff.rs` for the TUI contract. Used when the policy
    /// classifies a challenge as unsolvable by the stack (hard block, KYC
    /// form, bank 2FA) and handoff mode is enabled via `CRAWLEX_HANDOFF`.
    /// Mirrors the fields `HandoffRequest` carries so the scheduler can
    /// build one without any extra bookkeeping.
    HumanHandoff {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<String>,
        url: url::Url,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screenshot_path: Option<std::path::PathBuf>,
    },
}

impl Decision {
    pub fn as_tag(&self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Render => "render",
            Self::Retry { .. } => "retry",
            Self::SwitchProxy => "switch_proxy",
            Self::Defer { .. } => "defer",
            Self::Drop => "drop",
            Self::CollectArtifacts => "collect_artifacts",
            Self::IncreaseObservability => "increase_observability",
            Self::HumanHandoff { .. } => "human_handoff",
        }
    }
}

/// Structured reason attached to every decision. The `code` is
/// colon-separated (`category:subcategory`) and forms the `why=` field of
/// NDJSON events. Callers build one via the constructors; raw matching on
/// `code` is discouraged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionReason {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl DecisionReason {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: None,
        }
    }
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    // Canonical reasons — adding new ones here is the stable way to extend.
    pub fn initial_http() -> Self {
        Self::new("initial:http")
    }
    pub fn initial_render() -> Self {
        Self::new("initial:render")
    }
    pub fn antibot_challenge(vendor: &str) -> Self {
        Self::new(format!("render:antibot:{vendor}"))
    }
    pub fn js_only_content() -> Self {
        Self::new("render:js-only-content")
    }
    pub fn status_transient(status: u16) -> Self {
        Self::new(format!("retry:{status}"))
    }
    pub fn proxy_bad_score() -> Self {
        Self::new("switch_proxy:bad-score")
    }
    pub fn budget_exceeded() -> Self {
        Self::new("drop:budget-exceeded")
    }
    pub fn host_cooldown() -> Self {
        Self::new("defer:host-cooldown")
    }
}

impl std::fmt::Display for DecisionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "{}={}", self.code, d),
            None => f.write_str(&self.code),
        }
    }
}
