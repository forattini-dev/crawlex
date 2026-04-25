//! Unit tests for the Chromium launch-args builder.
//!
//! `crawlex::render::pool::build_launch_args` is the single source of
//! truth for the argv handed to the Chrome binary. These tests pin down
//! the *shape* of that argv — not the exact ordering — so future
//! refactors (e.g. reordering, deduplication, extracting helpers) stay
//! safe as long as every flag the plan audited (#43, #17, viewport/DPR
//! from bundle, proxy bypass, WebRTC leak fix, `--noexpose-wasm`, UA
//! sourced from bundle) remains present.
//!
//! We avoid spinning up a real Browser here — the whole point of the
//! refactor was to make this pure so it runs in <1ms without Chromium.

use crawlex::identity::IdentityBundle;
use crawlex::render::pool::build_launch_args;
use url::Url;

/// Helper — find the single flag whose key matches `prefix` (e.g.
/// `--window-size=`). Panics when absent so the test failure points at
/// the missing flag rather than a None unwrap deep in an assertion.
fn find<'a>(flags: &'a [String], prefix: &str) -> &'a str {
    flags
        .iter()
        .find(|f| f.starts_with(prefix))
        .unwrap_or_else(|| panic!("flag {prefix:?} not in argv: {flags:#?}"))
}

fn has(flags: &[String], token: &str) -> bool {
    flags.iter().any(|f| f == token)
}

/// Default desktop bundle (Chrome 124-ish shape).
fn desktop_bundle() -> IdentityBundle {
    IdentityBundle::from_chromium(124, 0xdeadbeef)
}

/// Mutate `b` into a mobile-ish persona so the viewport/DPR test has
/// non-default values to assert on. We only touch device fields — UA
/// and UA-CH stay coherent with whatever `from_chromium` produced.
fn mobile_like(mut b: IdentityBundle) -> IdentityBundle {
    b.viewport_w = 390;
    b.viewport_h = 844;
    b.screen_w = 390;
    b.screen_h = 844;
    b.device_pixel_ratio = 3.0;
    b
}

#[test]
fn desktop_argv_carries_core_flags() {
    let b = desktop_bundle();
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags = build_launch_args(&b, None, &udd, "en-US,en", &[]);

    // Stability / noise.
    assert!(has(&flags, "--disable-dev-shm-usage"));
    assert!(has(&flags, "--no-first-run"));
    assert!(has(&flags, "--no-default-browser-check"));

    // Stealth.
    assert!(has(&flags, "--disable-blink-features=AutomationControlled"));

    // WebRTC leak fix (#S.3).
    assert!(has(
        &flags,
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp"
    ));

    // Feature toggles we turn ON — VAAPI (#43), TLS13 Kyber PQ (#17),
    // AcceptCH frame, Zstd. Exact string is pinned so the *set* of
    // features is enumerable from a single grep.
    assert!(has(
        &flags,
        "--enable-features=VaapiVideoDecoder,AcceptCHFrame,ZstdContentEncoding,EnableTLS13KyberPQ"
    ));

    // JS/WASM surface trim.
    assert!(has(&flags, "--js-flags=--noexpose-wasm"));

    // user-data-dir carries the exact path we passed in.
    let udd_flag = find(&flags, "--user-data-dir=");
    assert_eq!(udd_flag, "--user-data-dir=/tmp/crawlex-udd-test");
}

#[test]
fn desktop_viewport_and_dpr_come_from_bundle() {
    let b = desktop_bundle();
    let expected_size = format!("--window-size={},{}", b.viewport_w, b.viewport_h);
    let expected_dpr = format!("--force-device-scale-factor={}", b.device_pixel_ratio);
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags = build_launch_args(&b, None, &udd, "en-US,en", &[]);
    assert!(
        has(&flags, &expected_size),
        "expected {expected_size:?} in argv: {flags:#?}"
    );
    assert!(
        has(&flags, &expected_dpr),
        "expected {expected_dpr:?} in argv: {flags:#?}"
    );
}

#[test]
fn mobile_bundle_projects_mobile_viewport() {
    let b = mobile_like(desktop_bundle());
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags = build_launch_args(&b, None, &udd, "en-US,en", &[]);

    assert!(has(&flags, "--window-size=390,844"));
    assert!(has(&flags, "--force-device-scale-factor=3"));
}

#[test]
fn user_agent_is_sourced_from_bundle_not_profile() {
    let mut b = desktop_bundle();
    // A value no real `Profile::user_agent()` would ever emit —
    // proves the launcher trusts the bundle and not the profile.
    b.ua = "Mozilla/5.0 (CrawlexTest/UA-marker)".into();
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags = build_launch_args(&b, None, &udd, "en-US,en", &[]);

    assert!(has(
        &flags,
        "--user-agent=Mozilla/5.0 (CrawlexTest/UA-marker)"
    ));
}

#[test]
fn proxy_adds_server_and_bypass_list() {
    let b = desktop_bundle();
    let proxy = Url::parse("http://127.0.0.1:9050").unwrap();
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags = build_launch_args(&b, Some(&proxy), &udd, "en-US,en", &[]);

    let server = find(&flags, "--proxy-server=");
    assert!(server.contains("127.0.0.1:9050"), "got {server:?}");
    // Loopback bypass override — critical for local proxies not going
    // direct. See #S.3 rationale in `build_launch_args` doc.
    assert!(has(&flags, "--proxy-bypass-list=<-loopback>"));
}

#[test]
fn no_proxy_means_no_proxy_flags() {
    let b = desktop_bundle();
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags = build_launch_args(&b, None, &udd, "en-US,en", &[]);
    assert!(!flags.iter().any(|f| f.starts_with("--proxy-server=")));
    assert!(!flags.iter().any(|f| f.starts_with("--proxy-bypass-list=")));
}

#[test]
fn extra_flags_are_appended_and_can_override() {
    let b = desktop_bundle();
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    // Operator opts into real GPU and a custom Chrome feature.
    let extra = vec![
        "--use-gl=angle".to_string(),
        "--use-angle=gl".to_string(),
        "--enable-gpu-rasterization".to_string(),
    ];
    let flags = build_launch_args(&b, None, &udd, "en-US,en", &extra);
    for e in &extra {
        assert!(has(&flags, e), "extra {e:?} missing from argv");
    }
    // Extras come after defaults — Chromium last-wins semantics let
    // operators override `--disable-gpu` by adding `--enable-gpu`.
    let disable_pos = flags.iter().position(|f| f == "--disable-gpu").unwrap();
    let angle_pos = flags.iter().position(|f| f == "--use-gl=angle").unwrap();
    assert!(
        angle_pos > disable_pos,
        "extras must be appended AFTER defaults"
    );
}

#[test]
fn lang_flag_follows_languages_argument() {
    let b = desktop_bundle();
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags_en = build_launch_args(&b, None, &udd, "en-US,en", &[]);
    assert!(has(&flags_en, "--lang=en-US,en"));

    let flags_pt = build_launch_args(&b, None, &udd, "pt-BR,en", &[]);
    assert!(has(&flags_pt, "--lang=pt-BR,en"));
}

#[test]
fn zero_viewport_falls_back_to_conservative_desktop() {
    // Guard against bundles that somehow carry zeros — we never want
    // to emit `--window-size=0,0` and crash Chrome's compositor init.
    let mut b = desktop_bundle();
    b.viewport_w = 0;
    b.viewport_h = 0;
    b.device_pixel_ratio = 0.0;
    let udd = std::path::PathBuf::from("/tmp/crawlex-udd-test");
    let flags = build_launch_args(&b, None, &udd, "en-US,en", &[]);
    assert!(has(&flags, "--window-size=1920,1080"));
    assert!(has(&flags, "--force-device-scale-factor=1"));
}
