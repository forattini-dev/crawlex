//! Module-boundary smoke tests for the reCAPTCHA v3 invisible solver.
//!
//! Confirms the public surface (parse `SolverKind`, build the adapter via
//! `build_solver`, dispatch a `ChallengePayload`) hangs together. Networked
//! pieces (the actual `solver::solve` pipeline that hits Google) are
//! exercised by the inline `#[cfg(test)]` modules under
//! `src/antibot/recaptcha/*.rs`.

#![cfg(feature = "cdp-backend")]

use std::str::FromStr;

use crawlex::antibot::solver::{build_solver, ChallengePayload, SolverError, SolverKind};
use crawlex::antibot::ChallengeVendor;

fn payload(vendor: ChallengeVendor, sitekey: Option<&str>) -> ChallengePayload {
    ChallengePayload {
        vendor,
        url: url::Url::parse("https://example.com/login").unwrap(),
        sitekey: sitekey.map(String::from),
        action: Some("login".into()),
        iframe_srcs: vec![],
        screenshot_png: None,
    }
}

#[test]
fn solver_kind_parses_recaptcha_invisible_aliases() {
    // Three string forms map to the same enum variant — keeps config
    // ergonomic across `--captcha-solver`, env vars, and TOML.
    for form in ["recaptcha-invisible", "recaptcha_invisible", "recaptcha"] {
        let kind = SolverKind::from_str(form).unwrap_or_else(|e| panic!("{form}: {e:?}"));
        assert_eq!(kind, SolverKind::RecaptchaInvisible);
    }
}

#[test]
fn build_solver_returns_recaptcha_invisible_adapter() {
    let solver = build_solver(SolverKind::RecaptchaInvisible)
        .expect("RecaptchaInvisible should be available with cdp-backend feature");
    assert_eq!(solver.name(), "recaptcha-invisible");
    assert!(solver
        .supported_vendors()
        .contains(&ChallengeVendor::Recaptcha));
    // Server-side replay only handles vanilla v3 — Enterprise / HCaptcha /
    // Turnstile are different protocols.
    assert!(!solver
        .supported_vendors()
        .contains(&ChallengeVendor::HCaptcha));
    assert!(!solver
        .supported_vendors()
        .contains(&ChallengeVendor::CloudflareTurnstile));
    assert!(!solver
        .supported_vendors()
        .contains(&ChallengeVendor::RecaptchaEnterprise));
}

#[tokio::test]
async fn recaptcha_invisible_rejects_unsupported_vendors() {
    // Make sure the dispatch refuses HCaptcha / Turnstile up-front rather
    // than falling through to the network and hitting Google with a wrong
    // sitekey — that would be wasted volume + a noisy error path.
    let solver = build_solver(SolverKind::RecaptchaInvisible).unwrap();
    let err = solver
        .solve(payload(ChallengeVendor::HCaptcha, Some("00000000-ffff-ffff-ffff-000000000001")))
        .await
        .unwrap_err();
    assert!(
        matches!(err, SolverError::UnsupportedVendor { .. }),
        "expected UnsupportedVendor, got {err:?}"
    );
}

#[tokio::test]
async fn recaptcha_invisible_requires_sitekey() {
    // Without a sitekey we can't even build the anchor URL — the adapter
    // surfaces `Upstream("missing sitekey")` instead of attempting a
    // request to a malformed Google endpoint.
    let solver = build_solver(SolverKind::RecaptchaInvisible).unwrap();
    let err = solver
        .solve(payload(ChallengeVendor::Recaptcha, None))
        .await
        .unwrap_err();
    assert!(
        matches!(err, SolverError::Upstream(ref s) if s.contains("sitekey")),
        "expected Upstream missing-sitekey, got {err:?}"
    );
}

#[test]
fn external_adapters_remain_prevention_first() {
    // The 3 external adapters (2captcha / anticaptcha / vlm) intentionally
    // refuse to run until env vars wire an API key. This test pins the
    // contract: a deployed adapter without keys is a no-op, not a silent
    // outbound call to a paid service.
    for kind in [SolverKind::TwoCaptcha, SolverKind::AntiCaptcha, SolverKind::Vlm] {
        let solver = build_solver(kind).expect("external adapter builds");
        // Solver `name()` is defined; no panics on construction.
        assert!(!solver.name().is_empty(), "{kind:?} has no name");
    }
}
