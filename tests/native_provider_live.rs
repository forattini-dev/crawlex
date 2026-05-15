//! Live validation harness for native stealth browser providers (slice 37).
//!
//! Gated entirely on the `CRAWLEX_NATIVE_PROVIDER_VALIDATION` env var. CI
//! does NOT run this — the harness assumes a user-managed CDP endpoint
//! (CloakBrowser, Camoufox, etc.) is reachable. Stock-Chromium runs are
//! also opt-in so the suite is one consistent gated path rather than two
//! permission models.
//!
//! Output is captured as structured events (`provider.selected`,
//! `calibration.summary`) plus optional fingerprint reports — review
//! manually before quoting any number in public docs.

#![cfg(feature = "cdp-backend")]

use std::env;

fn enabled() -> bool {
    env::var("CRAWLEX_NATIVE_PROVIDER_VALIDATION")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn cdp_url() -> Option<String> {
    env::var("CRAWLEX_EXTERNAL_CDP_URL").ok()
}

#[tokio::test]
async fn stock_baseline_captures_provider_event() {
    if !enabled() {
        eprintln!("skipped: set CRAWLEX_NATIVE_PROVIDER_VALIDATION=1 to enable");
        return;
    }
    // Driver TODO: spin up stock Chromium, navigate to fixture target,
    // assert `provider.selected` event carries `browser_provider=stock`
    // and that no `calibration.summary` event fires (stock path skips
    // calibration).
}

#[tokio::test]
async fn native_provider_captures_calibration_summary() {
    if !enabled() {
        eprintln!("skipped: set CRAWLEX_NATIVE_PROVIDER_VALIDATION=1 to enable");
        return;
    }
    let Some(_endpoint) = cdp_url() else {
        eprintln!("skipped: CRAWLEX_EXTERNAL_CDP_URL not configured");
        return;
    };
    // Driver TODO: connect via external CDP, navigate to fixture target,
    // assert `provider.selected` carries `endpoint_kind` + `vendor`,
    // and that a `calibration.summary` event fires.
}

#[tokio::test]
async fn parity_run_visits_validation_target_set() {
    if !enabled() {
        eprintln!("skipped: set CRAWLEX_NATIVE_PROVIDER_VALIDATION=1 to enable");
        return;
    }
    let Some(_endpoint) = cdp_url() else {
        eprintln!("skipped: CRAWLEX_EXTERNAL_CDP_URL not configured");
        return;
    };
    // Driver TODO: run identical target set under stock + native, write
    // both event streams to a side-car directory (configurable via
    // CRAWLEX_NATIVE_PROVIDER_VALIDATION_DIR). The harness must NOT
    // assert pass-rates against third-party detection pages — only that
    // both providers complete the run and emit the expected events.
}
