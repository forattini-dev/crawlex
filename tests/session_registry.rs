//! Fase 6 — SessionRegistry + decide_scope unit coverage.
//!
//! Lives as an integration test (not inside `src`) so operators who
//! wire their own `Crawler` can use the same entry points the registry
//! offers: `get_or_create`, `mark`, `evict`, `expired`, `list`, plus
//! the pure `decide_scope` policy.

use std::sync::Arc;
use std::time::Duration;

use crawlex::antibot::{ChallengeLevel, ChallengeVendor, SessionState};
use crawlex::config::RenderSessionScope;
use crawlex::identity::{
    spawn_cleanup_task, EvictionReason, SessionArchive, SessionDropTarget, SessionEntry,
    SessionRegistry,
};
use crawlex::policy::{decide_scope, ScopeDecision, ScopeSignal};
use url::Url;

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn get_or_create_is_idempotent_and_bumps_urls() {
    let reg = SessionRegistry::new(60);
    let url = u("https://a.test/one");
    let first = reg.get_or_create("sess", RenderSessionScope::RegistrableDomain, &url);
    let again = reg.get_or_create("sess", RenderSessionScope::RegistrableDomain, &url);
    assert_eq!(first.id, "sess");
    assert_eq!(first.urls_visited, 1);
    assert_eq!(again.urls_visited, 2);
    assert_eq!(again.state, SessionState::Clean);
}

#[test]
fn mark_is_monotonic_on_the_consumer_side() {
    let reg = SessionRegistry::new(60);
    let _ = reg.get_or_create("s", RenderSessionScope::Url, &u("https://x.test/"));
    assert_eq!(
        reg.mark("s", SessionState::Warm),
        Some((SessionState::Clean, SessionState::Warm))
    );
    // No-op when the target matches the current state.
    assert_eq!(reg.mark("s", SessionState::Warm), None);
    // Transitions forward when asked.
    assert_eq!(
        reg.mark("s", SessionState::Contaminated),
        Some((SessionState::Warm, SessionState::Contaminated))
    );
}

#[test]
fn expired_fires_after_ttl_elapses() {
    let reg = SessionRegistry::new(60);
    let _ = reg.get_or_create("s", RenderSessionScope::Url, &u("https://x.test/"));
    // Zero-duration override so the entry is instantly expired.
    reg.set_ttl_override("s", Some(Duration::from_millis(0)));
    std::thread::sleep(Duration::from_millis(5));
    let expired = reg.expired();
    assert_eq!(expired, vec!["s".to_string()]);
}

#[test]
fn list_filters_by_state() {
    let reg = SessionRegistry::new(60);
    let _ = reg.get_or_create("a", RenderSessionScope::Url, &u("https://a.test/"));
    let _ = reg.get_or_create("b", RenderSessionScope::Url, &u("https://b.test/"));
    reg.mark("b", SessionState::Blocked);
    assert_eq!(reg.list(None).len(), 2);
    assert_eq!(reg.list(Some(SessionState::Blocked)).len(), 1);
    assert_eq!(reg.list(Some(SessionState::Clean)).len(), 1);
}

#[test]
fn decide_scope_demotes_on_login_page_when_wider_than_origin() {
    let d = decide_scope(
        RenderSessionScope::RegistrableDomain,
        &ScopeSignal::LoginPageDetected,
    );
    assert_eq!(d, ScopeDecision::DemoteTo(RenderSessionScope::Origin));
}

#[test]
fn decide_scope_keeps_on_login_page_when_already_narrow() {
    let d = decide_scope(RenderSessionScope::Url, &ScopeSignal::LoginPageDetected);
    assert_eq!(d, ScopeDecision::Keep);
}

#[test]
fn decide_scope_demotes_to_url_on_hard_block() {
    let d = decide_scope(
        RenderSessionScope::RegistrableDomain,
        &ScopeSignal::AntibotHostility(
            ChallengeVendor::CloudflareJsChallenge,
            ChallengeLevel::HardBlock,
        ),
    );
    assert_eq!(d, ScopeDecision::DemoteTo(RenderSessionScope::Url));
}

#[test]
fn decide_scope_forces_url_on_quarantine() {
    let d = decide_scope(RenderSessionScope::Origin, &ScopeSignal::HostQuarantined);
    assert_eq!(d, ScopeDecision::Force(RenderSessionScope::Url));
}

#[test]
fn decide_scope_keeps_on_cross_origin_fetch() {
    let d = decide_scope(RenderSessionScope::Origin, &ScopeSignal::CrossOriginFetch);
    assert_eq!(d, ScopeDecision::Keep);
}

#[test]
fn decide_scope_keeps_on_suspected_level() {
    let d = decide_scope(
        RenderSessionScope::RegistrableDomain,
        &ScopeSignal::AntibotHostility(
            ChallengeVendor::CloudflareTurnstile,
            ChallengeLevel::Suspected,
        ),
    );
    assert_eq!(d, ScopeDecision::Keep);
}

#[test]
fn scope_key_for_matches_scope_granularity() {
    let url = u("https://shop.example.com:8443/a/b?q=1");
    assert!(
        SessionRegistry::scope_key_for(RenderSessionScope::RegistrableDomain, &url,)
            .ends_with("example.com")
    );
    assert_eq!(
        SessionRegistry::scope_key_for(RenderSessionScope::Host, &url),
        "shop.example.com:8443"
    );
    assert!(
        SessionRegistry::scope_key_for(RenderSessionScope::Origin, &url)
            .starts_with("https://shop.example.com")
    );
    assert_eq!(
        SessionRegistry::scope_key_for(RenderSessionScope::Url, &url),
        url.as_str()
    );
}

// ----- cleanup task end-to-end -------------------------------------

struct NoopDrop {
    dropped: Arc<parking_lot::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl SessionDropTarget for NoopDrop {
    async fn drop_session(&self, id: &str) {
        self.dropped.lock().push(id.to_string());
    }
}

struct CaptureArchive {
    archived: Arc<parking_lot::Mutex<Vec<(String, EvictionReason)>>>,
}

#[async_trait::async_trait]
impl SessionArchive for CaptureArchive {
    async fn archive_session(
        &self,
        entry: &SessionEntry,
        reason: EvictionReason,
    ) -> crawlex::Result<()> {
        self.archived.lock().push((entry.id.clone(), reason));
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cleanup_task_evicts_expired_and_archives() {
    let reg = Arc::new(SessionRegistry::new(1));
    let _ = reg.get_or_create("s1", RenderSessionScope::Url, &u("https://s1.test/"));
    reg.set_ttl_override("s1", Some(Duration::from_millis(0)));
    let dropped = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let archived = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let drop_target: Arc<dyn SessionDropTarget> = Arc::new(NoopDrop {
        dropped: dropped.clone(),
    });
    let archive_sink: Arc<dyn SessionArchive> = Arc::new(CaptureArchive {
        archived: archived.clone(),
    });
    let handle = spawn_cleanup_task(
        reg.clone(),
        drop_target,
        Some(archive_sink),
        Duration::from_millis(20),
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    handle.abort();
    assert!(dropped.lock().iter().any(|id| id == "s1"));
    let a = archived.lock();
    assert!(a
        .iter()
        .any(|(id, r)| id == "s1" && *r == EvictionReason::Ttl));
}
