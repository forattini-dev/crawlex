//! Build the `oz` JSON payload that gets scrambled and shipped as field 4
//! of the protobuf reload request. Field numbers below match the public
//! grecaptcha JS bundle (reverse-engineered, stable for current invisible
//! v3). They will change when Google rotates the schema.
//!
//! Persona-aware: when `IdentityBundle` is present, fields 73 (UA-CH) and
//! 74 (client telemetry) are populated from the bundle so the solver
//! agrees with the rest of our crawler. Falls back to vanilla Chrome 136
//! Windows defaults when no bundle is wired.

use base64::{engine::general_purpose, Engine as _};
use rand::{Rng, RngExt};
use serde_json::json;

use crate::identity::IdentityBundle;

use super::telemetry;

/// Random-canvas-hash range mirrors the reference solver. We add a small
/// modulation by `bundle.canvas_audio_seed` so personas with different
/// seeds emit distinct hashes (otherwise two bundles with the same
/// session_seed would collide).
fn random_canvas_hash(rng: &mut impl Rng, bundle: Option<&IdentityBundle>) -> String {
    let base: u64 = rng.random_range(100_000_000u64..=4_294_967_295u64);
    let seeded = match bundle {
        Some(b) => base ^ (b.canvas_audio_seed & 0xffff_ffff),
        None => base,
    };
    seeded.to_string()
}

/// One canonical Linux/Windows Chrome WebGL renderer per GPU vendor.
/// Aligned with our `PersonaProfile.webgl_unmasked_renderer` so a future
/// bundle-aware path can pick a concrete row instead of randomizing.
const FALLBACK_RENDERERS: &[&str] = &[
    "ANGLE (Intel, Intel(R) UHD Graphics 630, OpenGL 4.5)",
    "ANGLE (Intel, Intel(R) Iris(R) Xe Graphics, OpenGL 4.6)",
    "ANGLE (NVIDIA, NVIDIA GeForce GTX 1660, OpenGL 4.6)",
    "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060, OpenGL 4.6)",
    "ANGLE (AMD, AMD Radeon RX 580, OpenGL 4.6)",
];

fn pick_renderer(rng: &mut impl Rng, bundle: Option<&IdentityBundle>) -> String {
    if let Some(b) = bundle {
        // Coherence: use the bundle's renderer so WebGL ↔ WebGL agreement
        // holds across reCAPTCHA's blob and our shim.
        if !b.webgl_unmasked_renderer.is_empty() {
            return b.webgl_unmasked_renderer.clone();
        }
    }
    let idx = rng.random_range(0..FALLBACK_RENDERERS.len());
    FALLBACK_RENDERERS[idx].to_string()
}

/// Construct the UA-CH brands JSON for field 73. When a bundle is present
/// we use its `ua_brands`; otherwise we fall back to the reference's
/// hardcoded Chrome 136 list — used only by no-persona smoke paths.
fn ua_ch_blob(bundle: Option<&IdentityBundle>) -> serde_json::Value {
    match bundle {
        Some(b) => {
            // ua_brands is a JSON-encoded array string in the bundle —
            // parse it back so the resulting blob has structured content,
            // not a string-wrapped string.
            let brands: serde_json::Value = serde_json::from_str(&b.ua_brands).unwrap_or(json!([]));
            json!({
                "brands": brands,
                "mobile": false,
                "platform": b.ua_platform.trim_matches('"'),
            })
        }
        None => json!({
            "brands": [
                ["Chromium", "136"],
                ["Not-A.Brand", "24"],
                ["Google Chrome", "136"]
            ],
            "mobile": false,
            "platform": "Windows",
        }),
    }
}

/// Build the raw `oz` JSON bytes. Caller passes `site_url` (the page that
/// hosts the captcha) and an optional persona bundle.
pub fn build_oz(
    rng: &mut impl Rng,
    site_url: &str,
    bundle: Option<&IdentityBundle>,
    timestamp_ms: i64,
) -> Vec<u8> {
    let ts_b64 = general_purpose::STANDARD
        .encode(timestamp_ms.to_string().as_bytes())
        .trim_end_matches('=')
        .to_string();

    let mut nonce_bytes = [0u8; 24];
    for byte in nonce_bytes.iter_mut() {
        *byte = rng.random_range(0u8..=255);
    }
    let nonce = general_purpose::URL_SAFE_NO_PAD.encode(nonce_bytes);

    let canvas = random_canvas_hash(rng, bundle);
    let webgl = pick_renderer(rng, bundle);
    let client_blob = telemetry::generate(rng, bundle, &canvas, &webgl);

    // Field numbers literal — rotation here = solver breakage, ack'd in
    // module-level docs.
    let oz = json!({
        "5": rng.random_range(1000..=9999).to_string(),
        "6": rng.random_range(1..=10),
        "17": [nonce],
        "18": 2,
        "19": ts_b64,
        "28": site_url,
        "65": rng.random_range(1000..=5000),
        "73": ua_ch_blob(bundle),
        "74": client_blob,
    });

    // Compact serialization (no whitespace) — matches Python `separators=(",", ":")`.
    serde_json::to_vec(&oz).expect("oz JSON serializable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn oz_has_expected_field_keys() {
        let mut rng = StdRng::seed_from_u64(42);
        let bytes = build_oz(&mut rng, "https://example.com/", None, 1_700_000_000_000);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for k in &["5", "6", "17", "18", "19", "28", "65", "73", "74"] {
            assert!(v.get(*k).is_some(), "field {k} missing");
        }
    }

    #[test]
    fn oz_field_28_is_site_url() {
        let mut rng = StdRng::seed_from_u64(1);
        let bytes = build_oz(&mut rng, "https://2captcha.com", None, 0);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["28"].as_str().unwrap(), "https://2captcha.com");
    }

    #[test]
    fn oz_with_bundle_uses_persona_ua() {
        let mut rng = StdRng::seed_from_u64(7);
        let bundle = IdentityBundle::from_chromium(131, 999);
        let bytes = build_oz(&mut rng, "https://example.com", Some(&bundle), 0);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Field 74's `ua` reflects bundle UA, not Windows fallback.
        assert_eq!(v["74"]["ua"].as_str().unwrap(), bundle.ua);
        // Field 73's brands array is structured (not a JSON-encoded string).
        assert!(v["73"]["brands"].is_array());
    }

    #[test]
    fn oz_field_18_is_two() {
        // Constant in reference; treat as a regression guard.
        let mut rng = StdRng::seed_from_u64(2);
        let bytes = build_oz(&mut rng, "https://x.test", None, 0);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["18"].as_i64().unwrap(), 2);
    }

    #[test]
    fn oz_is_valid_json_round_trips() {
        // Replaces a brittle `no whitespace` check — string fields like the
        // WebGL renderer legitimately contain `, ` (e.g. "ANGLE (Intel,
        // Mesa Intel(R) UHD Graphics 630 (CFL GT2), OpenGL 4.6)"), so
        // testing the raw bytes for absence of `, ` was wrong.
        // What matters wire-side is that serde produces compact framing
        // (no spaces between tokens at the JSON structural level), which
        // `to_vec` guarantees by default — verify by round-trip.
        let mut rng = StdRng::seed_from_u64(3);
        let bytes = build_oz(&mut rng, "https://x.test", None, 0);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Re-serialise via `to_vec` and check byte-equality with the
        // canonical compact form — guarantees no extraneous structural
        // whitespace.
        let canonical = serde_json::to_vec(&v).unwrap();
        assert_eq!(bytes, canonical);
    }

    #[test]
    fn oz_canvas_hash_differs_per_session() {
        let bundle_a = IdentityBundle::from_chromium(131, 1);
        let bundle_b = IdentityBundle::from_chromium(131, 2);
        let mut rng_a = StdRng::seed_from_u64(0);
        let mut rng_b = StdRng::seed_from_u64(0);
        let a = build_oz(&mut rng_a, "https://x.test", Some(&bundle_a), 0);
        let b = build_oz(&mut rng_b, "https://x.test", Some(&bundle_b), 0);
        let va: serde_json::Value = serde_json::from_slice(&a).unwrap();
        let vb: serde_json::Value = serde_json::from_slice(&b).unwrap();
        // Different bundles → different canvas seeds → different hashes
        // even with identical RNG state.
        assert_ne!(va["74"]["canvas"].as_str(), vb["74"]["canvas"].as_str(),);
    }
}
