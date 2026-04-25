//! Fase 6 — session scope policy wiring.
//!
//! Verifies `decide_scope` returns the demotion/force scope decisions
//! the crawler uses when `session_scope_auto` is enabled. The crawler
//! hot-path test is in `session_registry.rs`; this file isolates the
//! policy matrix for scope transitions.

use crawlex::antibot::{ChallengeLevel, ChallengeVendor};
use crawlex::config::RenderSessionScope;
use crawlex::policy::{decide_scope, ScopeDecision, ScopeSignal};

#[test]
fn login_page_demotes_registrable_domain_to_origin() {
    assert_eq!(
        decide_scope(
            RenderSessionScope::RegistrableDomain,
            &ScopeSignal::LoginPageDetected,
        ),
        ScopeDecision::DemoteTo(RenderSessionScope::Origin)
    );
}

#[test]
fn login_page_is_noop_when_scope_already_origin() {
    assert_eq!(
        decide_scope(RenderSessionScope::Origin, &ScopeSignal::LoginPageDetected),
        ScopeDecision::Keep
    );
}

#[test]
fn hard_block_demotes_to_url() {
    let decision = decide_scope(
        RenderSessionScope::Origin,
        &ScopeSignal::AntibotHostility(ChallengeVendor::Akamai, ChallengeLevel::HardBlock),
    );
    assert_eq!(decision, ScopeDecision::DemoteTo(RenderSessionScope::Url));
}

#[test]
fn challenge_page_demotes_to_origin() {
    let decision = decide_scope(
        RenderSessionScope::Host,
        &ScopeSignal::AntibotHostility(
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::ChallengePage,
        ),
    );
    assert_eq!(
        decision,
        ScopeDecision::DemoteTo(RenderSessionScope::Origin)
    );
}

#[test]
fn quarantine_is_forced_regardless_of_current_scope() {
    for scope in [
        RenderSessionScope::RegistrableDomain,
        RenderSessionScope::Host,
        RenderSessionScope::Origin,
        RenderSessionScope::Url,
    ] {
        assert_eq!(
            decide_scope(scope, &ScopeSignal::HostQuarantined),
            ScopeDecision::Force(RenderSessionScope::Url)
        );
    }
}

#[test]
fn suspected_is_not_aggressive_enough_to_demote() {
    let decision = decide_scope(
        RenderSessionScope::RegistrableDomain,
        &ScopeSignal::AntibotHostility(ChallengeVendor::Recaptcha, ChallengeLevel::Suspected),
    );
    assert_eq!(decision, ScopeDecision::Keep);
}
