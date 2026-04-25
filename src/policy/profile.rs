//! PolicyProfile — named preset of thresholds that shape every decision.
//!
//! `--profile fast|balanced|deep|forensics` on the CLI selects one of
//! these; a bespoke `Config.policy_thresholds` always wins.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyProfile {
    /// Max throughput, render only when the HTTP path literally can't
    /// produce usable content.
    Fast,
    /// Default balance. HTTP-first with conservative render escalation.
    #[default]
    Balanced,
    /// Prefer render whenever uncertain — better coverage at higher cost.
    Deep,
    /// Every request collects full artifacts (network log, screenshots,
    /// traces). For audit runs / debugging production targets.
    Forensics,
}

/// Numeric parameters a `PolicyEngine` reads to make decisions.
///
/// Values are tied to a `PolicyProfile` via `PolicyThresholds::for_profile`.
/// Callers may derive custom thresholds from a profile and then override
/// specific fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyThresholds {
    /// Max HTTP response body size (bytes) we're willing to parse/store.
    pub max_body_bytes: u64,
    /// Cap on wall-time per job (ms).
    pub max_job_ms: u64,
    /// Escalation is **forbidden** when the render budget for this run
    /// has been exhausted — keep HTTP-only even if antibot is detected.
    pub max_render_jobs: Option<u64>,
    /// Global retry cap; Policy honours `Config.retry_max` but never
    /// exceeds this.
    pub max_retries: u32,
    /// Backoff exponent base (ms); actual delay = base * 2^attempt.
    pub retry_base_ms: u64,
    /// How long to sit on a host after a 429/503 before trying again.
    pub host_cooldown_ms: u64,
    /// Proxy score floor — below this, policy issues `SwitchProxy`.
    pub proxy_score_floor: f32,
    /// When true, every fetch also triggers full artifact capture (screenshot,
    /// network log, metrics). Forensics profile only by default.
    pub always_capture_artifacts: bool,
}

impl PolicyThresholds {
    pub fn for_profile(p: PolicyProfile) -> Self {
        match p {
            PolicyProfile::Fast => Self {
                max_body_bytes: 10 * 1024 * 1024,
                max_job_ms: 15_000,
                max_render_jobs: Some(0),
                max_retries: 2,
                retry_base_ms: 500,
                host_cooldown_ms: 1_000,
                proxy_score_floor: 0.2,
                always_capture_artifacts: false,
            },
            PolicyProfile::Balanced => Self {
                max_body_bytes: 20 * 1024 * 1024,
                max_job_ms: 30_000,
                max_render_jobs: None,
                max_retries: 3,
                retry_base_ms: 500,
                host_cooldown_ms: 5_000,
                proxy_score_floor: 0.4,
                always_capture_artifacts: false,
            },
            PolicyProfile::Deep => Self {
                max_body_bytes: 50 * 1024 * 1024,
                max_job_ms: 60_000,
                max_render_jobs: None,
                max_retries: 5,
                retry_base_ms: 1_000,
                host_cooldown_ms: 15_000,
                proxy_score_floor: 0.5,
                always_capture_artifacts: false,
            },
            PolicyProfile::Forensics => Self {
                max_body_bytes: 100 * 1024 * 1024,
                max_job_ms: 120_000,
                max_render_jobs: None,
                max_retries: 5,
                retry_base_ms: 1_000,
                host_cooldown_ms: 30_000,
                proxy_score_floor: 0.7,
                always_capture_artifacts: true,
            },
        }
    }
}

impl Default for PolicyThresholds {
    fn default() -> Self {
        Self::for_profile(PolicyProfile::Balanced)
    }
}
