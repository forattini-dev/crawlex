//! Wave1 — identity coherence matrix.
//!
//! Builds bundles across the full catalog × Chromium-major sweep and
//! asserts every one passes the validator. Guards against a future
//! bundle edit that breaks one persona row silently (unit tests on the
//! validator alone would miss that regression).

use crawlex::identity::{
    persona_catalog, IdentityBundle, IdentityValidator, PersonaGpu, PersonaOs,
};

fn majors() -> &'static [u32] {
    // 131 is the long-standing default; 149 is the Chrome-we-ship major.
    // Extend when a newer major lands in the bundle profile table.
    &[131, 149]
}

#[test]
fn every_persona_row_validates_on_every_major() {
    for row in persona_catalog() {
        // Android/mobile rows carry a `Linux armv8l` platform + Android
        // ua-ch-platform; they'd trip the desktop UA-OS match in the
        // validator. Skip mobile here; a dedicated mobile validator
        // pass is phase-2 work.
        if row.os == PersonaOs::AndroidMobile {
            continue;
        }
        for &m in majors() {
            for seed in [1u64, 42, 0xdead_beef, 0xBEEF_CAFE] {
                let b = IdentityBundle::from_persona(row, m, seed);
                let res = IdentityValidator::check(&b);
                assert!(
                    res.is_ok(),
                    "persona os={:?} gpu={:?} major={} seed={:#x} failed: {:?}",
                    row.os,
                    row.gpu,
                    m,
                    seed,
                    res
                );
            }
        }
    }
}

#[test]
fn hundred_bundles_all_pass() {
    // The plan calls for 100 bundles → 100 % coherent. We sweep seeds
    // against the four desktop rows (indices 0..=3) of the catalog.
    let desktop: Vec<_> = persona_catalog()
        .iter()
        .filter(|p| p.os != PersonaOs::AndroidMobile)
        .collect();
    let mut pass = 0;
    for i in 0..100u64 {
        let row = desktop[(i as usize) % desktop.len()];
        let b = IdentityBundle::from_persona(row, 131, i ^ 0x9E37_79B9_7F4A_7C15);
        IdentityValidator::check(&b)
            .unwrap_or_else(|e| panic!("bundle {} os={:?} failed: {:?}", i, row.os, e));
        pass += 1;
    }
    assert_eq!(pass, 100);
}

#[test]
fn persona_gpu_matches_bundle_webgl_vendor_keyword() {
    for row in persona_catalog() {
        let b = IdentityBundle::from_persona(row, 131, 1);
        let lower = b.webgl_renderer.to_ascii_lowercase();
        let needle = row.gpu.keyword();
        assert!(
            lower.contains(needle),
            "row {:?}: renderer {:?} missing GPU keyword {:?}",
            row.os,
            b.webgl_renderer,
            needle
        );
    }
}

#[test]
fn persona_fonts_are_plausible_for_os() {
    for row in persona_catalog() {
        let f = row.fonts_json;
        match row.os {
            PersonaOs::Linux => assert!(f.contains("DejaVu") || f.contains("Liberation")),
            PersonaOs::Windows => assert!(f.contains("Segoe UI") || f.contains("Arial")),
            PersonaOs::MacOs => assert!(f.contains("Helvetica") || f.contains("SF Pro")),
            PersonaOs::AndroidMobile => assert!(f.contains("Roboto")),
        }
    }
}

#[test]
fn default_bundle_heap_limit_is_two_gib() {
    // Regression guard: default-constructed bundle must expose the 2 GiB
    // desktop limit. If the default drifts, every rendered shim drifts
    // with it.
    let b = IdentityBundle::from_chromium(131, 7);
    assert_eq!(b.heap_size_limit, 2_147_483_648);
    assert_eq!(b.audio_sample_rate, 48000);
    assert!(b.max_texture_size >= 4096);
    assert!(b.scrollbar_width <= 24);
}

#[test]
fn nvidia_windows_persona_validates_and_renders() {
    // Specific row check: Win10 pt-BR NVIDIA must clear every cross-
    // layer check (platform, WebGL vendor, locale, timezone offset).
    let row = persona_catalog()
        .iter()
        .find(|p| matches!(p.gpu, PersonaGpu::Nvidia))
        .expect("nvidia row present");
    let b = IdentityBundle::from_persona(row, 149, 0x1234_5678);
    IdentityValidator::check(&b).expect("nvidia bundle coherent");
    assert!(b.webgl_renderer.contains("NVIDIA"));
    assert_eq!(b.locale, "pt-BR");
    assert_eq!(b.timezone, "America/Sao_Paulo");
}

#[test]
fn macos_persona_validates_and_has_apple_gpu() {
    let row = persona_catalog()
        .iter()
        .find(|p| p.os == PersonaOs::MacOs)
        .expect("mac row present");
    let b = IdentityBundle::from_persona(row, 131, 99);
    IdentityValidator::check(&b).expect("mac bundle coherent");
    assert!(b.webgl_renderer.to_ascii_lowercase().contains("apple"));
    assert_eq!(b.ua_platform, "\"macOS\"");
    // Mac scrollbars are overlay → 0.
    assert_eq!(b.scrollbar_width, 0);
}
