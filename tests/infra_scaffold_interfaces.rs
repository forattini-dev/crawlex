//! Wave 1 — infrastructure-tier scaffold interface tests.
//!
//! These tests exist to pin the public contracts of the scaffold modules
//! so the (future) real implementations can't drift silently. They run
//! on both the full build and `crawlex-mini`; render-scoped modules
//! live under `#[cfg(feature = "cdp-backend")]` gates.

use std::str::FromStr;

use crawlex::antibot::solver::{
    build_solver, AntiCaptchaAdapter, CaptchaSolver, ChallengePayload, SolverError, SolverKind,
    TwoCaptchaAdapter, VlmAdapter,
};
use crawlex::antibot::ChallengeVendor;
use crawlex::identity::warmup::{SessionWarmup, WarmupPhase, WarmupPolicy};
use crawlex::proxy::residential::{
    build_provider, BrightDataStub, IPRoyalStub, OxylabsStub, ResidentialError,
    ResidentialProvider, ResidentialProviderKind,
};

#[test]
fn residential_provider_kind_roundtrip() {
    for raw in ["none", "brightdata", "oxylabs", "iproyal"] {
        let k: ResidentialProviderKind = raw.parse().unwrap();
        assert_eq!(k.as_str(), raw);
    }
    assert!(ResidentialProviderKind::from_str("quantum-proxy").is_err());
}

#[test]
fn residential_stub_adapters_refuse_cleanly() {
    let adapters: Vec<Box<dyn ResidentialProvider>> = vec![
        Box::new(BrightDataStub),
        Box::new(OxylabsStub),
        Box::new(IPRoyalStub),
    ];
    for a in adapters {
        let err = a.rotate("example.com").unwrap_err();
        assert!(matches!(err, ResidentialError::ProviderNotConfigured(_)));
    }
}

#[test]
fn build_residential_provider_none_is_disabled_by_default() {
    assert!(build_provider(ResidentialProviderKind::None).is_none());
    assert!(build_provider(ResidentialProviderKind::BrightData).is_some());
    assert!(build_provider(ResidentialProviderKind::Oxylabs).is_some());
    assert!(build_provider(ResidentialProviderKind::IPRoyal).is_some());
}

#[test]
fn warmup_state_machine_gates_login_until_budget_met() {
    let policy = WarmupPolicy {
        min_visits: 3,
        min_depth: 2,
        min_elapsed_secs: 0,
    };
    let mut w = SessionWarmup::new(policy);
    assert_eq!(w.phase(), WarmupPhase::Cold);
    assert_eq!(w.gate_login(), Err("warmup:cold"));

    w.record_visit(1);
    assert!(matches!(w.phase(), WarmupPhase::Warming { .. }));
    assert_eq!(w.gate_login(), Err("warmup:insufficient"));

    w.record_visit(2);
    w.record_visit(2);
    assert_eq!(w.phase(), WarmupPhase::Warm);
    assert_eq!(w.gate_login(), Ok(()));
}

#[test]
fn solver_kind_roundtrip() {
    for raw in ["none", "2captcha", "anticaptcha", "vlm"] {
        let k: SolverKind = raw.parse().unwrap();
        assert_eq!(k.as_str(), raw);
    }
}

#[test]
fn build_solver_none_is_disabled_by_default() {
    assert!(build_solver(SolverKind::None).is_none());
    assert!(build_solver(SolverKind::TwoCaptcha).is_some());
    assert!(build_solver(SolverKind::AntiCaptcha).is_some());
    assert!(build_solver(SolverKind::Vlm).is_some());
}

#[tokio::test]
async fn solver_stubs_refuse_without_credentials() {
    fn payload(vendor: ChallengeVendor) -> ChallengePayload {
        ChallengePayload {
            vendor,
            url: url::Url::parse("https://example.com/").unwrap(),
            sitekey: Some("k".into()),
            action: None,
            iframe_srcs: vec![],
            screenshot_png: None,
        }
    }
    let adapters: Vec<Box<dyn CaptchaSolver>> = vec![
        Box::new(TwoCaptchaAdapter),
        Box::new(AntiCaptchaAdapter),
        Box::new(VlmAdapter),
    ];
    for a in adapters {
        let err = a
            .solve(payload(ChallengeVendor::Recaptcha))
            .await
            .unwrap_err();
        assert!(matches!(err, SolverError::AdapterNotConfigured(_)));
    }
}

#[cfg(feature = "cdp-backend")]
mod render_scoped {
    use crawlex::render::android_profile::{parse_mobile_profile, AndroidDevice, AndroidProfile};
    use crawlex::render::handoff::{handoff_enabled, should_handoff, HandoffRequest};

    #[test]
    fn android_presets_have_coherent_cdp_payload() {
        let p = AndroidProfile::preset(AndroidDevice::Pixel7Pro);
        let cmds = p.cdp_commands();
        let methods: Vec<&str> = cmds.iter().map(|(m, _)| *m).collect();
        assert_eq!(
            methods,
            vec![
                "Emulation.setDeviceMetricsOverride",
                "Emulation.setUserAgentOverride",
                "Emulation.setTouchEmulationEnabled",
            ]
        );
        let metrics = p.device_metrics_payload();
        assert_eq!(metrics["width"], 412);
        assert_eq!(metrics["mobile"], true);
    }

    #[test]
    fn parse_mobile_profile_smoke() {
        assert!(parse_mobile_profile("pixel-7-pro").is_some());
        assert!(parse_mobile_profile("unknown").is_none());
    }

    #[test]
    fn handoff_is_disabled_by_default() {
        std::env::remove_var("CRAWLEX_HANDOFF");
        assert!(!handoff_enabled());
        // even a hard-block signal does not trigger handoff when env is off.
        let sig = crawlex::antibot::ChallengeSignal {
            vendor: crawlex::antibot::ChallengeVendor::CloudflareJsChallenge,
            level: crawlex::antibot::ChallengeLevel::HardBlock,
            url: url::Url::parse("https://example.com/").unwrap(),
            origin: "https://example.com".into(),
            proxy: None,
            session_id: "s".into(),
            first_seen: std::time::SystemTime::now(),
            metadata: serde_json::Value::Null,
        };
        assert!(!should_handoff(&sig));
        let req = HandoffRequest::from_signal(&sig, None);
        assert!(!req.should_pause());
    }
}
