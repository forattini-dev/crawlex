# Native stealth provider validation workflow

crawlex integrates with user-managed native stealth browser providers (CloakBrowser, Camoufox, etc.) via the external-CDP path landed in slices 29–36. This document is the **manual** validation workflow for confirming a configured native provider behaves correctly end-to-end. It is gated behind a feature flag because:

- CI does not require any third-party stealth binary.
- crawlex does not redistribute CloakBrowser, Camoufox, or any other native binary — users obtain and run their own.
- Public benchmark claims are out of scope (see `docs/comparisons/scrapling.md` and the PRD out-of-scope section).

## When to run this

Run after:

- Touching `src/render/cdp_probe.rs`, `cdp_capabilities.rs`, or `calibration.rs`.
- Bumping any of slices 29 (provider switch), 30 (CDP path), 31 (capability detection), 32 (calibration), 33 (calibration shim), 34 (mismatch policy), 35 (session mode), 36 (fallback chain).
- Onboarding a new native provider vendor.

## Prerequisites

- A stock Chromium install reachable to `cargo test` (the existing CI default).
- A configured native CDP endpoint, e.g.:
  ```bash
  export CRAWLEX_EXTERNAL_CDP_URL=http://127.0.0.1:9222
  ```
- Endpoint must be reachable via the WS debugger URL pattern that slice 30 probes.

## Run

```bash
export CRAWLEX_NATIVE_PROVIDER_VALIDATION=1
cargo test --features cdp-backend --test native_provider_live -- --nocapture
```

The harness lives in `tests/native_provider_live.rs`. Without the env flag, every test no-ops and prints a skip message. With the flag set:

- `stock_baseline_captures_provider_event` — runs against stock Chromium, asserts `provider.selected` event fires with `browser_provider=stock` and no `calibration.summary` event.
- `native_provider_captures_calibration_summary` — connects to `CRAWLEX_EXTERNAL_CDP_URL`, asserts `provider.selected` carries `endpoint_kind` + `vendor`, and `calibration.summary` fires.
- `parity_run_visits_validation_target_set` — runs the same target set under both providers and writes the event streams side-by-side to a directory (`CRAWLEX_NATIVE_PROVIDER_VALIDATION_DIR`, defaults to a temp dir) for human review.

## Validation target set

These are the targets the harness drives. They are **representative**, not exhaustive:

- A simple HTML page that asserts the request reached the server (smoke check — does the provider even fetch?).
- A page that runs a synchronous fingerprint JS snippet and posts the result back (calibration check).
- A page that exercises `<canvas>`, `<webgl>`, and timing APIs (mismatch policy stress test).

> The harness does **not** drive BrowserScan, FingerprintJS, CreepJS, or any other third-party detection site. Users who want that comparison must do it manually and treat the result as their own data point, not a crawlex claim.

## What "pass" means

Per slice 37 AC, the harness does **not** assert public pass-rate claims. It asserts:

1. Both stock and native paths complete the run without crashing.
2. The expected event stream is emitted (slice 30 + 31 events for native, just slice 30 events for stock).
3. Calibration summary fires only on the native path.

Any further "did the provider beat detector X" comparison is **manual** and must be reviewed by a human before going into public docs or marketing material — see slice 37 AC #7.

## Reporting issues

If the harness uncovers a regression, open an issue tagged `native-provider` with:

- The two event streams (the harness writes them next to each other).
- The provider binary version + endpoint kind banner.
- The crawlex commit SHA.

Do **not** include captured fingerprint reports in public issues — they may contain device-specific data.
