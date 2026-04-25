//! Tests for the Policy Engine's three decision points.

use crawlex::policy::{
    Decision, DecisionReason, PolicyContext, PolicyEngine, PolicyProfile, PolicyThresholds,
};
use crawlex::queue::FetchMethod;
use http::{HeaderMap, HeaderValue};
use url::Url;

fn ctx<'a>(
    url: &'a Url,
    host: &'a str,
    method: FetchMethod,
    thresholds: &'a PolicyThresholds,
) -> PolicyContext<'a> {
    PolicyContext {
        url,
        host,
        initial_method: method,
        response_status: None,
        response_headers: None,
        response_body: None,
        proxy_score: None,
        attempts: 0,
        render_budget_left: None,
        host_cooldown_ms_left: 0,
        thresholds,
    }
}

#[test]
fn pre_fetch_http_first_for_auto() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let c = ctx(&url, "example.com", FetchMethod::Auto, &th);
    let (d, r) = PolicyEngine::decide_pre_fetch(&c);
    assert_eq!(d, Decision::Http);
    assert_eq!(r.code, "initial:http");
}

#[test]
fn pre_fetch_render_respects_method() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let c = ctx(&url, "example.com", FetchMethod::Render, &th);
    let (d, _) = PolicyEngine::decide_pre_fetch(&c);
    assert_eq!(d, Decision::Render);
}

#[test]
fn pre_fetch_render_forbidden_in_fast_profile_falls_to_http() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Fast);
    let c = ctx(&url, "example.com", FetchMethod::Render, &th);
    let (d, _) = PolicyEngine::decide_pre_fetch(&c);
    assert_eq!(d, Decision::Http);
}

#[test]
fn pre_fetch_forensics_collects_artifacts_without_changing_method() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Forensics);
    let c = ctx(&url, "example.com", FetchMethod::Auto, &th);
    let (d, r) = PolicyEngine::decide_pre_fetch(&c);
    assert_eq!(d, Decision::CollectArtifacts);
    assert_eq!(r.code, "collect_artifacts:profile");
}

#[test]
fn post_fetch_escalates_on_cloudflare_signature() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    let body = br#"<html>Just a moment... cf-chl-bypass</html>"#;
    let mut c = ctx(&url, "example.com", FetchMethod::Auto, &th);
    c.response_status = Some(503);
    c.response_headers = Some(&headers);
    c.response_body = Some(body);
    let (d, r) = PolicyEngine::decide_post_fetch(&c);
    assert_eq!(d, Decision::Render);
    assert!(r.code.starts_with("render:antibot:cloudflare"));
}

#[test]
fn post_fetch_retries_on_503_within_cap() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let mut c = ctx(&url, "example.com", FetchMethod::Auto, &th);
    c.response_status = Some(503);
    c.attempts = 0;
    let (d, r) = PolicyEngine::decide_post_fetch(&c);
    assert!(matches!(d, Decision::Retry { .. }));
    assert_eq!(r.code, "retry:503");
}

#[test]
fn post_fetch_drops_on_503_after_max_retries() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let mut c = ctx(&url, "example.com", FetchMethod::Auto, &th);
    c.response_status = Some(503);
    c.attempts = th.max_retries; // already at cap
    let (d, _) = PolicyEngine::decide_post_fetch(&c);
    assert_eq!(d, Decision::Drop);
}

#[test]
fn post_fetch_switches_proxy_when_score_low() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let mut c = ctx(&url, "example.com", FetchMethod::Auto, &th);
    c.response_status = Some(200);
    c.proxy_score = Some(0.1); // below 0.4 floor
    let (d, _) = PolicyEngine::decide_post_fetch(&c);
    assert_eq!(d, Decision::SwitchProxy);
}

#[test]
fn post_error_retries_transient_then_drops() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let mut c = ctx(&url, "example.com", FetchMethod::Auto, &th);
    c.attempts = 0;
    let (d, _) = PolicyEngine::decide_post_error(&c, "dns");
    assert!(matches!(d, Decision::Retry { .. }));
    c.attempts = th.max_retries;
    let (d, _) = PolicyEngine::decide_post_error(&c, "dns");
    assert_eq!(d, Decision::Drop);
}

#[test]
fn post_error_retries_request_timeout_but_drops_body_limits() {
    let url = Url::parse("https://example.com/").unwrap();
    let th = PolicyThresholds::for_profile(PolicyProfile::Balanced);
    let c = ctx(&url, "example.com", FetchMethod::Auto, &th);

    let (d, r) = PolicyEngine::decide_post_error(&c, "request-timeout");
    assert!(matches!(d, Decision::Retry { .. }));
    assert_eq!(r.code, "retry:request-timeout");

    let (d, r) = PolicyEngine::decide_post_error(&c, "body-too-large");
    assert_eq!(d, Decision::Drop);
    assert_eq!(r.code, "drop:body-too-large");

    let (d, r) = PolicyEngine::decide_post_error(&c, "decoded-body-too-large");
    assert_eq!(d, Decision::Drop);
    assert_eq!(r.code, "drop:decoded-body-too-large");
}

#[test]
fn decision_reason_serializes_with_detail() {
    let r = DecisionReason::antibot_challenge("cloudflare").with_detail("render_forbidden");
    let s = serde_json::to_string(&r).unwrap();
    assert!(s.contains(r#""code":"render:antibot:cloudflare""#));
    assert!(s.contains(r#""detail":"render_forbidden""#));
}

#[test]
fn decide_post_challenge_matrix() {
    use crawlex::antibot::{ChallengeLevel, ChallengeSignal, ChallengeVendor, SessionState};
    use crawlex::policy::SessionAction;
    let u = Url::parse("https://example.com/").unwrap();
    let mk = |vendor: ChallengeVendor, level: ChallengeLevel| ChallengeSignal {
        vendor,
        level,
        url: u.clone(),
        origin: "https://example.com".into(),
        proxy: None,
        session_id: "s1".into(),
        first_seen: std::time::SystemTime::now(),
        metadata: serde_json::Value::Null,
    };
    let cases = [
        (
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::Suspected,
            SessionState::Clean,
            SessionAction::RotateProxy,
        ),
        (
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::ChallengePage,
            SessionState::Clean,
            SessionAction::ReopenBrowser,
        ),
        (
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::ChallengePage,
            SessionState::Warm,
            SessionAction::ReopenBrowser,
        ),
        (
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::ChallengePage,
            SessionState::Contaminated,
            SessionAction::GiveUp,
        ),
        (
            ChallengeVendor::CloudflareTurnstile,
            ChallengeLevel::WidgetPresent,
            SessionState::Clean,
            SessionAction::KillContext,
        ),
        (
            ChallengeVendor::Recaptcha,
            ChallengeLevel::WidgetPresent,
            SessionState::Warm,
            SessionAction::KillContext,
        ),
        (
            ChallengeVendor::AccessDenied,
            ChallengeLevel::ChallengePage,
            SessionState::Clean,
            SessionAction::GiveUp,
        ),
        (
            ChallengeVendor::Akamai,
            ChallengeLevel::HardBlock,
            SessionState::Blocked,
            SessionAction::GiveUp,
        ),
    ];
    for (vendor, lvl, st, expect) in cases {
        let sig = mk(vendor, lvl);
        let action = PolicyEngine::decide_post_challenge(&sig, st, None);
        assert_eq!(
            action, expect,
            "vendor={:?} lvl={:?} state={:?}",
            vendor, lvl, st
        );
    }
}
