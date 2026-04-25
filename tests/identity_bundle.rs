//! Tests for IdentityBundle coherence and the validator.

use crawlex::identity::{IdentityBundle, IdentityValidator, ValidationError};

#[test]
fn bundle_from_chromium_131_is_coherent() {
    let b = IdentityBundle::from_chromium(131, 0xdead_beef);
    IdentityValidator::check(&b).expect("131 bundle coherent");
    assert_eq!(b.ua_major, 131);
    assert!(b.ua.contains("131"));
    assert!(b.sec_ch_ua.contains(r#"v="131""#));
    assert_eq!(b.canvas_audio_seed, 0xdead_beef);
}

#[test]
fn bundle_from_chromium_149_is_coherent() {
    let b = IdentityBundle::from_chromium(149, 1);
    IdentityValidator::check(&b).expect("149 bundle coherent");
    assert_eq!(b.ua_major, 149);
    assert!(b.ua.contains("149"));
}

#[test]
fn validator_catches_ua_mismatch() {
    let mut b = IdentityBundle::from_chromium(131, 1);
    b.ua_major = 999; // sabotage
    match IdentityValidator::check(&b) {
        Err(ValidationError::UaMajorMismatch { major, .. }) => assert_eq!(major, 999),
        other => panic!("expected UaMajorMismatch, got {:?}", other),
    }
}

#[test]
fn validator_catches_locale_mismatch() {
    let mut b = IdentityBundle::from_chromium(131, 1);
    b.locale = "pt-BR".into();
    assert!(matches!(
        IdentityValidator::check(&b),
        Err(ValidationError::LocaleNotInLanguages { .. })
    ));
}

#[test]
fn validator_catches_tz_offset_drift() {
    let mut b = IdentityBundle::from_chromium(131, 1);
    b.tz_offset_min = -60; // São Paulo is +180
    assert!(matches!(
        IdentityValidator::check(&b),
        Err(ValidationError::TimezoneOffsetMismatch { .. })
    ));
}

#[test]
fn validator_catches_avail_exceeds_screen() {
    let mut b = IdentityBundle::from_chromium(131, 1);
    b.avail_screen_h = b.screen_h + 100;
    assert!(matches!(
        IdentityValidator::check(&b),
        Err(ValidationError::AvailExceedsScreen { axis: "h", .. })
    ));
}

#[test]
fn validator_catches_viewport_larger_than_screen() {
    let mut b = IdentityBundle::from_chromium(131, 1);
    b.viewport_w = b.screen_w + 100;
    assert!(matches!(
        IdentityValidator::check(&b),
        Err(ValidationError::ViewportExceedsScreen { axis: "w", .. })
    ));
}

#[test]
fn different_seeds_yield_distinct_bundle_ids() {
    let a = IdentityBundle::from_chromium(131, 1);
    let b = IdentityBundle::from_chromium(131, 2);
    assert_ne!(a.id, b.id);
    assert_ne!(a.canvas_audio_seed, b.canvas_audio_seed);
}
