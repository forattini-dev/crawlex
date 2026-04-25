//! P0-9 — vendor telemetry classifier coverage.
//!
//! Pure-Rust tests: no Chrome required. We feed canned request URLs +
//! (tiny) body fixtures into `classify_request` and assert the
//! resulting `PayloadShape` matches the vendor's expected bucket.

use crawlex::antibot::signatures::{px_signal, PX_SIGNALS};
use crawlex::antibot::telemetry::{
    classify_request, infer_akamai_fields, AkamaiField, ObservedRequest, PayloadShape,
    TelemetryTracker,
};
use crawlex::antibot::ChallengeVendor;
use std::time::{Duration, SystemTime};

fn u(s: &str) -> url::Url {
    url::Url::parse(s).unwrap()
}

fn req<'a>(url: &'a url::Url, method: &'a str, body: &'a [u8]) -> ObservedRequest<'a> {
    ObservedRequest {
        url,
        method,
        body,
        session_id: "t-session",
    }
}

// --- PX catalog coverage ---------------------------------------------

#[test]
fn px_signal_catalog_has_29_entries() {
    assert_eq!(PX_SIGNALS.len(), 29, "PX catalog must cover PX320..=PX348");
}

#[test]
fn px_signal_ids_match_canonical_range() {
    for (i, s) in PX_SIGNALS.iter().enumerate() {
        assert_eq!(s.id, format!("PX{}", 320 + i));
    }
}

#[test]
fn px_signal_lookup_case_insensitive() {
    assert_eq!(px_signal("px342").unwrap().name, "mouse_entropy");
    assert_eq!(px_signal("PX320").unwrap().name, "cdp_detection");
    assert!(px_signal("PX000").is_none());
}

// --- Per-vendor classifier coverage ----------------------------------

#[test]
fn akamai_v17_sensor_data_shape() {
    let url = u("https://site.example.com/_bm/_data");
    let body = br#"{"sensor_data":"1.7,-1,0,0,mmd=1,kact=xx,sc;1920,1080,uaend=x"}"#;
    let t = classify_request(&req(&url, "POST", body)).expect("must match akamai");
    assert_eq!(t.vendor, ChallengeVendor::Akamai);
    match &t.payload_shape {
        PayloadShape::AkamaiSensorDataV1_7 { keys_found } => {
            assert!(
                keys_found.iter().any(|k| k == "sensor_data"),
                "sensor_data key should be found; got {keys_found:?}"
            );
        }
        other => panic!("expected AkamaiSensorDataV1_7, got {other:?}"),
    }
}

#[test]
fn akamai_v2_sbsd_ek_shape() {
    let url = u("https://site.example.com/akam/11/abc");
    let body = br#"{"sbsd_ek":"aes-garbage","p":"..."}"#;
    let t = classify_request(&req(&url, "POST", body)).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::Akamai);
    assert_eq!(
        t.payload_shape,
        PayloadShape::AkamaiSensorDataV2 { has_sbsd_ek: true }
    );
}

#[test]
fn akamai_field_inference_detects_mouse_screen_typing_fp() {
    let payload = "uaend;mmd=1;touch=0;sc;1920,1080;kact;z;fpValstr=a";
    let fields = infer_akamai_fields(payload);
    assert!(fields.contains(&AkamaiField::MouseEvents));
    assert!(fields.contains(&AkamaiField::Screen));
    assert!(fields.contains(&AkamaiField::Typing));
    assert!(fields.contains(&AkamaiField::Fingerprint));
}

#[test]
fn perimeterx_collector_shape_extracts_px_ids() {
    let url = u("https://client.perimeterx.net/api/v2/collector?appId=PX");
    // Minimal deobfuscated-looking payload — keys only, values fake.
    let body = br#"{"PX320":"1","PX333":"Intel","PX342":"mouse","PX346":"false"}"#;
    let t = classify_request(&req(&url, "POST", body)).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::PerimeterX);
    match t.payload_shape {
        PayloadShape::PerimeterXCollector { event_ids } => {
            assert!(event_ids.contains(&"PX320".to_string()));
            assert!(event_ids.contains(&"PX333".to_string()));
            assert!(event_ids.contains(&"PX342".to_string()));
            assert!(event_ids.contains(&"PX346".to_string()));
        }
        other => panic!("expected PerimeterXCollector, got {other:?}"),
    }
}

#[test]
fn datadome_report_counts_signals() {
    let url = u("https://js.datadome.co/js/tags.js");
    let body = br#"{"a":1,"b":2,"c":3,"d":4,"e":5,"f":6}"#;
    let t = classify_request(&req(&url, "POST", body)).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::DataDome);
    match t.payload_shape {
        PayloadShape::DataDomeReport { signal_count } => assert!(signal_count >= 5),
        other => panic!("expected DataDomeReport, got {other:?}"),
    }
}

#[test]
fn cloudflare_turnstile_classified() {
    let url = u("https://challenges.cloudflare.com/turnstile/v0/api.js");
    let t = classify_request(&req(&url, "GET", b"")).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::CloudflareTurnstile);
    assert_eq!(
        t.payload_shape,
        PayloadShape::CloudflareChallenge { has_tk: false }
    );
}

#[test]
fn cloudflare_challenge_platform_classified() {
    let url = u("https://example.com/cdn-cgi/challenge-platform/h/g/cv/result/abcd");
    let body = br#"{"tk":"abc","ts":0}"#;
    let t = classify_request(&req(&url, "POST", body)).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::CloudflareJsChallenge);
    assert_eq!(
        t.payload_shape,
        PayloadShape::CloudflareChallenge { has_tk: true }
    );
}

#[test]
fn hcaptcha_sitekey_extracted_from_query() {
    let url =
        u("https://hcaptcha.com/checkcaptcha/xyz?sitekey=10000000-ffff-ffff-ffff-000000000001");
    let t = classify_request(&req(&url, "POST", b"")).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::HCaptcha);
    match t.payload_shape {
        PayloadShape::HCaptchaExecute { sitekey } => {
            assert_eq!(
                sitekey.as_deref(),
                Some("10000000-ffff-ffff-ffff-000000000001")
            );
        }
        other => panic!("expected HCaptchaExecute, got {other:?}"),
    }
}

#[test]
fn recaptcha_enterprise_wins_over_plain() {
    // The enterprise pattern must be recognised before the plain one —
    // URLs contain "recaptcha" both times.
    let url = u("https://www.google.com/recaptcha/enterprise.js?render=SITEKEY");
    let t = classify_request(&req(&url, "GET", b"")).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::RecaptchaEnterprise);
}

#[test]
fn recaptcha_reload_extracts_k_and_v() {
    let url = u("https://www.google.com/recaptcha/api2/reload?k=MYK&v=MYV");
    let t = classify_request(&req(&url, "POST", b"")).unwrap();
    assert_eq!(t.vendor, ChallengeVendor::Recaptcha);
    match t.payload_shape {
        PayloadShape::RecaptchaReload { k, v } => {
            assert_eq!(k.as_deref(), Some("MYK"));
            assert_eq!(v.as_deref(), Some("MYV"));
        }
        other => panic!("expected RecaptchaReload, got {other:?}"),
    }
}

#[test]
fn innocent_urls_are_not_classified() {
    for plain in [
        "https://example.com/index.html",
        "https://news.ycombinator.com/",
        "https://cdn.example.com/static/app.js",
    ] {
        let url = u(plain);
        assert!(
            classify_request(&req(&url, "GET", b"")).is_none(),
            "url {plain} must not trigger classifier"
        );
    }
}

// --- Tracker threshold ------------------------------------------------

#[test]
fn telemetry_tracker_fires_at_threshold() {
    let mut tracker = TelemetryTracker::with_config(Duration::from_secs(30), 3);
    let now = SystemTime::now();
    assert!(!tracker.observe("s1", ChallengeVendor::PerimeterX, now));
    assert!(!tracker.observe(
        "s1",
        ChallengeVendor::PerimeterX,
        now + Duration::from_millis(10)
    ));
    assert!(tracker.observe(
        "s1",
        ChallengeVendor::PerimeterX,
        now + Duration::from_millis(20)
    ));
}

#[test]
fn tracker_window_expires_old_events() {
    let mut tracker = TelemetryTracker::with_config(Duration::from_secs(1), 3);
    let t0 = SystemTime::now();
    tracker.observe("s1", ChallengeVendor::Akamai, t0);
    tracker.observe("s1", ChallengeVendor::Akamai, t0);
    let later = t0 + Duration::from_secs(5);
    assert!(!tracker.observe("s1", ChallengeVendor::Akamai, later));
    assert_eq!(tracker.hits("s1", ChallengeVendor::Akamai), 1);
}

#[test]
fn tracker_isolates_sessions_and_vendors() {
    let mut tracker = TelemetryTracker::with_config(Duration::from_secs(30), 2);
    let now = SystemTime::now();
    tracker.observe("s1", ChallengeVendor::Akamai, now);
    tracker.observe("s2", ChallengeVendor::Akamai, now);
    tracker.observe("s1", ChallengeVendor::DataDome, now);
    assert_eq!(tracker.hits("s1", ChallengeVendor::Akamai), 1);
    assert_eq!(tracker.hits("s2", ChallengeVendor::Akamai), 1);
    assert_eq!(tracker.hits("s1", ChallengeVendor::DataDome), 1);
    assert_eq!(tracker.hits("s1", ChallengeVendor::PerimeterX), 0);
}

// --- Policy wiring ----------------------------------------------------

#[test]
fn policy_decide_on_telemetry_volume_rotates_for_akamai() {
    use crawlex::antibot::SessionState;
    use crawlex::policy::engine::{PolicyEngine, SessionAction};
    assert_eq!(
        PolicyEngine::decide_on_telemetry_volume(ChallengeVendor::Akamai, SessionState::Clean),
        SessionAction::RotateProxy
    );
    // Blocked is sticky → give up.
    assert_eq!(
        PolicyEngine::decide_on_telemetry_volume(ChallengeVendor::Akamai, SessionState::Blocked),
        SessionAction::GiveUp
    );
    // Captcha widgets don't warrant volume-based rotation.
    assert_eq!(
        PolicyEngine::decide_on_telemetry_volume(ChallengeVendor::HCaptcha, SessionState::Clean),
        SessionAction::ReuseSession
    );
}
