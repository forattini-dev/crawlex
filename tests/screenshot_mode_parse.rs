//! Unit coverage for `parse_screenshot_mode`: the CLI surface has to reject
//! garbage up-front (before we spawn Chromium) and accept the three canonical
//! modes. Kept out of `#[ignore]` gating since it's pure string parsing —
//! runs in the default `cargo test --all-features` sweep.

#![cfg(feature = "cdp-backend")]

use crawlex::render::pool::{parse_screenshot_mode, ScreenshotCaptureMode};

#[test]
fn viewport_parses() {
    assert!(matches!(
        parse_screenshot_mode("viewport").unwrap(),
        ScreenshotCaptureMode::Viewport
    ));
    // Case-insensitive.
    assert!(matches!(
        parse_screenshot_mode("Viewport").unwrap(),
        ScreenshotCaptureMode::Viewport
    ));
}

#[test]
fn fullpage_parses_all_forms() {
    for s in ["fullpage", "FullPage", "full", "full_page"] {
        assert!(
            matches!(
                parse_screenshot_mode(s).unwrap(),
                ScreenshotCaptureMode::FullPage
            ),
            "failed for input `{s}`"
        );
    }
}

#[test]
fn element_parses_with_selector() {
    let m = parse_screenshot_mode("element:#dashboard").unwrap();
    match m {
        ScreenshotCaptureMode::Element { selector } => assert_eq!(selector, "#dashboard"),
        _ => panic!("expected Element mode"),
    }
}

#[test]
fn element_parses_with_complex_selector() {
    let m = parse_screenshot_mode("element:div.card[data-id='42']").unwrap();
    match m {
        ScreenshotCaptureMode::Element { selector } => {
            assert_eq!(selector, "div.card[data-id='42']");
        }
        _ => panic!("expected Element mode"),
    }
}

#[test]
fn element_without_selector_errors() {
    let err = parse_screenshot_mode("element:").unwrap_err();
    assert!(err.contains("requires a selector"), "got: {err}");
}

#[test]
fn unknown_mode_errors_clearly() {
    let err = parse_screenshot_mode("thumbnail").unwrap_err();
    assert!(
        err.contains("viewport|fullpage|element"),
        "error should list valid modes, got: {err}"
    );
}

#[test]
fn empty_string_errors() {
    assert!(parse_screenshot_mode("").is_err());
}
