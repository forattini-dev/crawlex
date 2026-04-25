//! PolicyEngine::decide — the one entry point the crawler calls at
//! three policy points per job (pre-fetch, post-fetch, post-error).
//!
//! The engine is pure: given a `PolicyContext`, it returns a
//! `(Decision, DecisionReason)` without side effects. The caller is
//! responsible for executing the decision and emitting the NDJSON event.

use http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::antibot::{ChallengeLevel, ChallengeSignal, ChallengeVendor, SessionState};
use crate::config::RenderSessionScope;
use crate::escalation::detect_antibot_vendor;
use crate::policy::profile::PolicyThresholds;
use crate::policy::reason::{Decision, DecisionReason};
use crate::queue::FetchMethod;

/// Signal feeding `decide_scope`. Represents the observation that
/// prompted the crawler to reconsider its current render-session scope.
#[derive(Debug, Clone)]
pub enum ScopeSignal {
    /// A login page was detected — forms with password fields. Demote
    /// to `Origin` so cookies collected on login don't leak across
    /// subdomains of the same registrable domain.
    LoginPageDetected,
    /// The page served a hard antibot wall. Contract the scope to the
    /// exact URL so forensic traces stay tightly bound to this one
    /// probe.
    AntibotHostility(ChallengeVendor, ChallengeLevel),
    /// Operator quarantined the host — treat any future render of it
    /// as its own fresh scope, never reusing state.
    HostQuarantined,
    /// A cross-origin fetch was observed. Kept for completeness — the
    /// current decision is `Keep`, but callers should still feed it.
    CrossOriginFetch,
}

/// Outcome of `decide_scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeDecision {
    /// Leave the current scope alone.
    Keep,
    /// Narrow the scope (RegistrableDomain → Origin, Origin → Url).
    /// Callers interpret "narrower than current" — the engine only
    /// returns a demotion when the target is strictly narrower.
    DemoteTo(RenderSessionScope),
    /// Widen the scope. Not produced by the initial rule set, but the
    /// variant exists so future callers can plug in without a signature
    /// change.
    PromoteTo(RenderSessionScope),
    /// Force a specific scope regardless of ordering (e.g. hosts on
    /// quarantine).
    Force(RenderSessionScope),
}

/// Ordinal for scope "width". Lower = tighter. Used by `decide_scope`
/// to decide whether a proposed change is a demotion or a promotion.
fn scope_rank(s: RenderSessionScope) -> u8 {
    match s {
        RenderSessionScope::Url => 0,
        RenderSessionScope::Origin => 1,
        RenderSessionScope::Host => 2,
        RenderSessionScope::RegistrableDomain => 3,
    }
}

/// Pure scope policy. Given the current scope + a signal, choose to
/// keep, demote, promote, or force a new one. Conservative by design:
/// login pages → `Origin`, hard blocks → `Url`, quarantine → forced
/// `Url`, everything else → `Keep`.
pub fn decide_scope(current: RenderSessionScope, signal: &ScopeSignal) -> ScopeDecision {
    match signal {
        ScopeSignal::LoginPageDetected => {
            // Only demote if we're currently wider than Origin.
            if scope_rank(current) > scope_rank(RenderSessionScope::Origin) {
                ScopeDecision::DemoteTo(RenderSessionScope::Origin)
            } else {
                ScopeDecision::Keep
            }
        }
        ScopeSignal::AntibotHostility(_, level) => match level {
            ChallengeLevel::HardBlock => {
                if scope_rank(current) > scope_rank(RenderSessionScope::Url) {
                    ScopeDecision::DemoteTo(RenderSessionScope::Url)
                } else {
                    ScopeDecision::Keep
                }
            }
            ChallengeLevel::ChallengePage | ChallengeLevel::WidgetPresent => {
                if scope_rank(current) > scope_rank(RenderSessionScope::Origin) {
                    ScopeDecision::DemoteTo(RenderSessionScope::Origin)
                } else {
                    ScopeDecision::Keep
                }
            }
            ChallengeLevel::Suspected => ScopeDecision::Keep,
        },
        ScopeSignal::HostQuarantined => ScopeDecision::Force(RenderSessionScope::Url),
        ScopeSignal::CrossOriginFetch => ScopeDecision::Keep,
    }
}

/// Action the crawler takes after a challenge has been detected. Returned
/// by [`PolicyEngine::decide_post_challenge`] — pure: the caller is
/// responsible for actually rotating/killing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAction {
    /// Keep using the session; no change needed (fallback only — rarely
    /// returned by the current rules).
    ReuseSession,
    /// Drop the current proxy out of rotation for this job and pick an
    /// alternative on the next attempt.
    RotateProxy,
    /// Drop the browser context + cookies; next request on this session
    /// starts a fresh one.
    KillContext,
    /// Respawn the entire Browser instance — used when the session has
    /// been warmed and the contamination survives a context kill.
    ReopenBrowser,
    /// Stop trying. Host goes to long quarantine; URL drops.
    GiveUp,
}

impl SessionAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReuseSession => "reuse_session",
            Self::RotateProxy => "rotate_proxy",
            Self::KillContext => "kill_context",
            Self::ReopenBrowser => "reopen_browser",
            Self::GiveUp => "give_up",
        }
    }
}

/// Snapshot handed to `decide`. Fields are all cheap-to-construct
/// references (`Option` + slices) — the engine never mutates context.
pub struct PolicyContext<'a> {
    pub url: &'a url::Url,
    pub host: &'a str,
    /// Initial fetch method hint carried by the Job (from CLI or seed).
    pub initial_method: FetchMethod,
    /// Only present on post-fetch invocation — `None` for pre-fetch.
    pub response_status: Option<u16>,
    pub response_headers: Option<&'a HeaderMap>,
    pub response_body: Option<&'a [u8]>,
    /// Proxy score in `[0.0, 1.0]`. `None` when no proxy in use.
    pub proxy_score: Option<f32>,
    /// Retry attempts already performed on this job.
    pub attempts: u32,
    /// Render budget remaining for the run (`None` = unlimited).
    pub render_budget_left: Option<u64>,
    /// Host-specific state: when set, host is under cooldown.
    pub host_cooldown_ms_left: u64,
    /// The policy thresholds selected by the profile or overridden by config.
    pub thresholds: &'a PolicyThresholds,
}

pub struct PolicyEngine;

impl PolicyEngine {
    /// Pre-fetch: decide which engine to hit first.
    pub fn decide_pre_fetch(ctx: &PolicyContext<'_>) -> (Decision, DecisionReason) {
        if ctx.thresholds.always_capture_artifacts {
            return (
                Decision::CollectArtifacts,
                DecisionReason::new("collect_artifacts:profile").with_detail("forensics"),
            );
        }

        // Budget: render path might be outright forbidden for this run.
        let render_allowed = ctx.render_budget_left.is_none_or(|n| n > 0)
            && !matches!(ctx.thresholds.max_render_jobs, Some(0));

        match ctx.initial_method {
            FetchMethod::HttpSpoof => (Decision::Http, DecisionReason::initial_http()),
            FetchMethod::Render if render_allowed => {
                (Decision::Render, DecisionReason::initial_render())
            }
            FetchMethod::Render => (Decision::Http, DecisionReason::initial_http()),
            FetchMethod::Auto => (Decision::Http, DecisionReason::initial_http()),
        }
    }

    /// Post-fetch: saw a response, decide whether to escalate/retry/drop.
    /// Caller should already have called `decide_pre_fetch` and gotten a
    /// response back.
    pub fn decide_post_fetch(ctx: &PolicyContext<'_>) -> (Decision, DecisionReason) {
        let status = ctx.response_status.unwrap_or(0);
        let headers = ctx.response_headers;
        let body = ctx.response_body;

        // Proxy health first — if the proxy is bad, switching has lower
        // cost than any other decision.
        if let Some(score) = ctx.proxy_score {
            if score < ctx.thresholds.proxy_score_floor {
                return (Decision::SwitchProxy, DecisionReason::proxy_bad_score());
            }
        }

        // Anti-bot challenge detection FIRST: a 503 with a Cloudflare body
        // is not a transient 5xx — it's a wall, and re-fetching the same
        // way will fail the same way. Escalate to render (or drop if
        // render is forbidden) before retry kicks in.
        if let (Some(hdrs), Some(body)) = (headers, body) {
            if let Some(vendor) = detect_antibot_vendor(status, hdrs, body) {
                let render_allowed = ctx.render_budget_left.is_none_or(|n| n > 0)
                    && !matches!(ctx.thresholds.max_render_jobs, Some(0))
                    && ctx.initial_method != FetchMethod::Render;
                if render_allowed {
                    return (
                        Decision::Render,
                        DecisionReason::antibot_challenge(vendor.as_str()),
                    );
                } else {
                    return (
                        Decision::Drop,
                        DecisionReason::antibot_challenge(vendor.as_str())
                            .with_detail("render_forbidden"),
                    );
                }
            }
        }

        let render_allowed = ctx.render_budget_left.is_none_or(|n| n > 0)
            && !matches!(ctx.thresholds.max_render_jobs, Some(0))
            && ctx.initial_method != FetchMethod::Render;
        if status == 200
            && render_allowed
            && ctx.initial_method == FetchMethod::Auto
            && headers_look_html(headers)
            && body.is_some_and(looks_like_js_shell)
        {
            return (Decision::Render, DecisionReason::js_only_content());
        }

        if matches!(status, 401 | 403) {
            return (
                Decision::CollectArtifacts,
                DecisionReason::new("collect_artifacts:status").with_detail(status.to_string()),
            );
        }

        // Transient 5xx / rate-limited: retry with backoff unless
        // attempts exhausted.
        if matches!(status, 429 | 500 | 502 | 503 | 504) {
            if ctx.attempts + 1 >= ctx.thresholds.max_retries {
                return (
                    Decision::Drop,
                    DecisionReason::status_transient(status).with_detail("max_retries"),
                );
            }
            let backoff = ctx
                .thresholds
                .retry_base_ms
                .saturating_mul(1u64 << ctx.attempts.min(8));
            return (
                Decision::Retry { after_ms: backoff },
                DecisionReason::status_transient(status),
            );
        }

        // All signals healthy.
        (Decision::Http, DecisionReason::initial_http())
    }

    /// Post-challenge: map a detected `ChallengeSignal` + current
    /// `SessionState` to the recovery action. Pure function; caller
    /// executes the action and updates the session state via
    /// `SessionState::after_challenge`.
    ///
    /// Rules (conservative; falls back to `KillContext` for widget hits
    /// because we don't solve captchas):
    /// - `Suspected` → `RotateProxy` (weak signal, try a cleaner IP).
    /// - `ChallengePage` on a Clean session → `KillContext`.
    /// - `ChallengePage` on Warm/Contaminated/Blocked → `ReopenBrowser`.
    /// - `WidgetPresent` → `KillContext`.
    /// - `HardBlock` → `GiveUp`.
    pub fn decide_post_challenge(
        signal: &ChallengeSignal,
        session: SessionState,
        _proxy: Option<&url::Url>,
    ) -> SessionAction {
        if matches!(signal.level, ChallengeLevel::HardBlock)
            || matches!(signal.vendor, ChallengeVendor::AccessDenied)
        {
            return SessionAction::GiveUp;
        }

        match signal.vendor {
            ChallengeVendor::CloudflareTurnstile
            | ChallengeVendor::Recaptcha
            | ChallengeVendor::RecaptchaEnterprise
            | ChallengeVendor::HCaptcha
            | ChallengeVendor::GenericCaptcha => match signal.level {
                ChallengeLevel::Suspected => SessionAction::RotateProxy,
                ChallengeLevel::WidgetPresent | ChallengeLevel::ChallengePage => {
                    SessionAction::KillContext
                }
                ChallengeLevel::HardBlock => SessionAction::GiveUp,
            },
            ChallengeVendor::CloudflareJsChallenge
            | ChallengeVendor::DataDome
            | ChallengeVendor::PerimeterX
            | ChallengeVendor::Akamai => match (signal.level, session) {
                (ChallengeLevel::Suspected, _) => SessionAction::RotateProxy,
                (ChallengeLevel::WidgetPresent, _) => SessionAction::KillContext,
                (ChallengeLevel::ChallengePage, SessionState::Clean | SessionState::Warm) => {
                    SessionAction::ReopenBrowser
                }
                (
                    ChallengeLevel::ChallengePage,
                    SessionState::Contaminated | SessionState::Blocked,
                ) => SessionAction::GiveUp,
                (ChallengeLevel::HardBlock, _) => SessionAction::GiveUp,
            },
            ChallengeVendor::AccessDenied => SessionAction::GiveUp,
        }
    }

    /// Map a `SessionAction::GiveUp` into `Decision::HumanHandoff` when
    /// the operator has opted into human-handoff via `CRAWLEX_HANDOFF=1`.
    /// Returns `None` for any other action (the caller keeps the existing
    /// `SessionAction` path).
    ///
    /// This is the single unification point: `src/render/handoff.rs`
    /// previously carried its own `HandoffDecision` enum because the
    /// policy-engine evolution wave hadn't fired yet. Wave 2 folds the
    /// handoff into `Decision::HumanHandoff`, so downstream NDJSON sinks,
    /// scheduler paths, and the decision log all see one canonical shape.
    pub fn maybe_human_handoff(
        action: SessionAction,
        signal: &ChallengeSignal,
        screenshot_path: Option<std::path::PathBuf>,
    ) -> Option<Decision> {
        if !matches!(action, SessionAction::GiveUp) {
            return None;
        }
        if !crate::render::handoff::handoff_enabled() {
            return None;
        }
        let req = crate::render::handoff::HandoffRequest::from_signal(signal, screenshot_path);
        Some(req.into_policy_decision())
    }

    /// P0-9 preemptive rotation: when the passive vendor-telemetry
    /// observer reports that a single `(session, vendor)` bucket has
    /// exceeded its volume threshold, the pool can call this to convert
    /// that observation into a concrete `SessionAction` without waiting
    /// for a full `ChallengeSignal`.
    ///
    /// Pure: the tracker decides "hit"; this decides "what to do".
    /// Current rule: `RotateProxy` always — we never kill context
    /// preemptively because the page might still render successfully.
    pub fn decide_on_telemetry_volume(
        vendor: crate::antibot::ChallengeVendor,
        session: SessionState,
    ) -> SessionAction {
        // Blocked sessions cannot be rescued by a proxy swap.
        if matches!(session, SessionState::Blocked) {
            return SessionAction::GiveUp;
        }
        // Vendors that embed widgets (captchas) in otherwise-usable
        // pages aren't worth rotating for on volume alone — the widget
        // itself will POST a lot as the user "interacts".
        match vendor {
            crate::antibot::ChallengeVendor::HCaptcha
            | crate::antibot::ChallengeVendor::Recaptcha
            | crate::antibot::ChallengeVendor::RecaptchaEnterprise => SessionAction::ReuseSession,
            _ => SessionAction::RotateProxy,
        }
    }

    /// Post-error: decide retry vs drop based on error kind + attempts.
    pub fn decide_post_error(
        ctx: &PolicyContext<'_>,
        err_kind: &str,
    ) -> (Decision, DecisionReason) {
        if ctx.attempts + 1 >= ctx.thresholds.max_retries {
            return (
                Decision::Drop,
                DecisionReason::new(format!("drop:{err_kind}:max_retries")),
            );
        }
        // DNS / TLS / transient network errors: retry with backoff.
        match err_kind {
            "dns" | "tls" | "io" | "http" | "request-timeout" => {
                let backoff = ctx
                    .thresholds
                    .retry_base_ms
                    .saturating_mul(1u64 << ctx.attempts.min(8));
                (
                    Decision::Retry { after_ms: backoff },
                    DecisionReason::new(format!("retry:{err_kind}")),
                )
            }
            _ => (
                Decision::Drop,
                DecisionReason::new(format!("drop:{err_kind}")),
            ),
        }
    }
}

fn headers_look_html(headers: Option<&HeaderMap>) -> bool {
    headers
        .and_then(|h| h.get("content-type"))
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            let ct = ct.to_ascii_lowercase();
            ct.contains("text/html") || ct.contains("application/xhtml")
        })
        .unwrap_or(false)
}

fn looks_like_js_shell(body: &[u8]) -> bool {
    let max = body.len().min(96 * 1024);
    let lower = String::from_utf8_lossy(&body[..max]).to_ascii_lowercase();
    if lower.contains("enable javascript to run this app")
        || lower.contains("requires javascript")
        || lower.contains("javascript is required")
    {
        return true;
    }

    let has_mount = lower.contains("id=\"root\"")
        || lower.contains("id='root'")
        || lower.contains("id=\"app\"")
        || lower.contains("id='app'")
        || lower.contains("id=\"__next\"")
        || lower.contains("id='__next'");
    if !has_mount || !lower.contains("<script") {
        return false;
    }

    let anchor_count = lower.matches("<a ").take(4).count();
    let paragraph_count = lower.matches("<p").take(3).count();
    let content_markers = lower.contains("<article")
        || lower.contains("<main")
        || anchor_count >= 3
        || paragraph_count >= 2;
    !content_markers
}
