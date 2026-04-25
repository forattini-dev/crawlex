//! Wave1 — stealth shim leak-closure coverage.
//!
//! Renders the shim template against a live IdentityBundle and asserts
//! each leak-closure section is present with its expected override
//! signature. Trims the risk of a future refactor silently deleting one
//! of the 12 shim-local leak fixes.

use crawlex::identity::{persona_catalog, IdentityBundle, PersonaOs};
use crawlex::render::stealth::render_shim_from_bundle;

fn rendered() -> String {
    let b = IdentityBundle::from_chromium(131, 0xdead_beef);
    render_shim_from_bundle(&b)
}

#[test]
fn no_unsubstituted_placeholders() {
    let js = rendered();
    // Any `{{FOO}}` left in the output means a placeholder was added to
    // the JS template without wiring through Rust substitution.
    let bad = js.lines().find(|l| l.contains("{{") && l.contains("}}"));
    assert!(
        bad.is_none(),
        "unsubstituted placeholder: {}",
        bad.unwrap_or("")
    );
}

#[test]
fn shim_contains_outer_inner_scrollbar_override() {
    // Leak #1 — outerWidth/innerWidth reconciliation for scrollbar shape.
    let js = rendered();
    assert!(js.contains("window.innerWidth"));
    assert!(js.contains("outerWidth"));
    assert!(js.contains("outerHeight"));
}

#[test]
fn shim_contains_heap_size_limit_placeholder_filled() {
    // Leak #2 — BASE_LIMIT must carry the bundle's heap_size_limit.
    let js = rendered();
    assert!(js.contains("const BASE_LIMIT ="));
    // Default bundle: 2147483648. Regression if the constant drifts.
    assert!(js.contains("2147483648"));
}

#[test]
fn shim_contains_chrome_runtime_platform_os() {
    // Leak #4 — enum-like objects on chrome.runtime.
    let js = rendered();
    assert!(js.contains("PlatformOs"));
    assert!(js.contains("'linux'"));
    assert!(js.contains("OnInstalledReason"));
    assert!(js.contains("INSTALL: 'install'"));
}

#[test]
fn shim_contains_speech_synthesis_voice_override() {
    // Leak #8 — speechSynthesis.getVoices returns a plausible list.
    let js = rendered();
    assert!(js.contains("speechSynthesis"));
    assert!(js.contains("getVoices"));
    // Default persona is Linux/Intel → non-apple branch → Google voices.
    assert!(js.contains("Google US English"));
}

#[test]
fn shim_contains_raf_hidden_throttle() {
    // Leak #11 — requestAnimationFrame throttled to 1 Hz when hidden.
    let js = rendered();
    assert!(js.contains("requestAnimationFrame"));
    assert!(js.contains("visibilityState"));
    assert!(js.contains("'hidden'"));
    assert!(js.contains("1000"));
}

#[test]
fn shim_contains_performance_now_precision_clamp() {
    // Leak #12 — 5 µs grain.
    let js = rendered();
    assert!(js.contains("performance.now"));
    assert!(js.contains("0.005"));
    assert!(js.contains("Math.floor"));
}

#[test]
fn shim_contains_audio_sample_rate_pin() {
    // Leak #44 — AudioContext.sampleRate pinned from bundle.
    let js = rendered();
    assert!(js.contains("const RATE ="));
    // Default bundle: 48000.
    assert!(js.contains("48000"));
    assert!(js.contains("AudioContext"));
    assert!(js.contains("OfflineAudioContext"));
}

#[test]
fn shim_contains_enumerate_devices_stub() {
    // Leak #45 — navigator.mediaDevices.enumerateDevices fake list.
    let js = rendered();
    assert!(js.contains("enumerateDevices"));
    assert!(js.contains("'audioinput'"));
    assert!(js.contains("'audiooutput'"));
    assert!(js.contains("'videoinput'"));
}

#[test]
fn shim_contains_fonts_list_surface() {
    let js = rendered();
    assert!(js.contains("__crawlex_fonts__"));
    // Default persona is Linux → DejaVu cluster.
    assert!(js.contains("DejaVu"));
}

#[test]
fn shim_canvas_uses_seeded_prng_not_math_random() {
    // The plan requires canvas/audio jitter to be deterministic per
    // (bundle, session) — never Math.random(). Math.random() in the
    // rendered shim means a regression.
    let js = rendered();
    assert!(
        !js.contains("Math.random("),
        "Math.random() found in shim — jitter must be seeded"
    );
}

#[test]
fn shim_audio_uses_box_muller_gaussian() {
    let js = rendered();
    assert!(js.contains("boxMuller"));
    assert!(js.contains("mulberry32"));
    assert!(js.contains("Math.log"));
}

#[test]
fn shim_gpu_vendor_keyword_matches_persona() {
    // When a non-default persona renders, the GPU keyword placeholder
    // must flow through so section 24 (voices) picks the right branch.
    let mac = persona_catalog()
        .iter()
        .find(|p| p.os == PersonaOs::MacOs)
        .unwrap();
    let b = IdentityBundle::from_persona(mac, 131, 1);
    let js = render_shim_from_bundle(&b);
    // Apple-branch voice: Samantha.
    assert!(
        js.contains("Samantha"),
        "mac persona should get Samantha voice"
    );
    // Apple keyword must appear in the fonts guard surface.
    assert!(js.to_ascii_lowercase().contains("apple"));
}

#[test]
fn shim_webgl_params_use_bundle_max_texture_size() {
    // Leak #A.3 — MAX_TEXTURE_SIZE must be bundle-driven, not hardcoded.
    let b = IdentityBundle::from_chromium(131, 1);
    let js = render_shim_from_bundle(&b);
    // Default is 16384.
    assert!(js.contains("16384"));
    // And MAX_VIEWPORT_DIMS as a tuple literal.
    assert!(js.contains("32767, 32767") || js.contains("32767,32767"));
}

#[test]
fn shim_hw_concurrency_and_device_memory_from_bundle() {
    let js = rendered();
    // Default bundle: hardware_concurrency=8, device_memory=8.
    assert!(js.contains("hardwareConcurrency: 8"));
    assert!(js.contains("deviceMemory:  8") || js.contains("deviceMemory: 8"));
}
