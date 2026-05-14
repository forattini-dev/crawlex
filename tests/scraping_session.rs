//! Integration tests for slice 16: Request.session_id + SessionManager
//! routing. Pinned against the public re-exports so SDK consumers can
//! rely on the same surface.

use crawlex::scraping::{BackendKind, Request, SessionManager};

#[test]
fn cookie_isolation_between_two_stealth_sessions() {
    let mgr = SessionManager::new(BackendKind::Http);
    let a = mgr.register("alpha", BackendKind::Stealth);
    let b = mgr.register("beta", BackendKind::Stealth);

    let route_a = mgr.route(&Request::new("https://shop.test/cart").with_session("alpha"));
    let route_b = mgr.route(&Request::new("https://shop.test/cart").with_session("beta"));

    route_a.jar.as_ref().unwrap().set("sid", "alpha-cookie");
    route_b.jar.as_ref().unwrap().set("sid", "beta-cookie");

    assert_eq!(a.jar.get("sid").as_deref(), Some("alpha-cookie"));
    assert_eq!(b.jar.get("sid").as_deref(), Some("beta-cookie"));
    assert!(a.jar.get("sid") != b.jar.get("sid"));
}

#[test]
fn unknown_session_id_falls_back_to_default_backend() {
    let mgr = SessionManager::new(BackendKind::Render);
    let req = Request::new("https://x.test/").with_session("does-not-exist");
    let route = mgr.route(&req);
    assert_eq!(route.backend, BackendKind::Render);
    assert!(route.fallback);
    assert!(route.jar.is_none());
}

#[test]
fn request_default_session_is_none() {
    let req = Request::new("https://x.test/");
    assert!(req.session_id.is_none());
}
