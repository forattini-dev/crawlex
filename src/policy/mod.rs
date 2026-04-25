//! Policy Engine — deterministic, explainable crawl decisions.
//!
//! Replaces the ad-hoc `FetchMethod::Auto` + `escalation::should_escalate`
//! coupling with a single function `PolicyEngine::decide(ctx)` returning
//! `(Decision, DecisionReason)`. Every call emits a `decision.made`
//! NDJSON event so operators can trace *why* a job went render (or got
//! retried, or dropped, or switched proxies) without reading code.

pub mod action_policy;
pub mod engine;
pub mod profile;
pub mod reason;

pub use action_policy::{ActionPolicy, ActionRule, ActionVerb};
pub use engine::{
    decide_scope, PolicyContext, PolicyEngine, ScopeDecision, ScopeSignal, SessionAction,
};
pub use profile::{PolicyProfile, PolicyThresholds};
pub use reason::{Decision, DecisionReason};
