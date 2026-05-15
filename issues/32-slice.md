# Slice 32: Per-session browser fingerprint calibration [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Run a mandatory calibration step for external CDP sessions before navigating to the target. crawlex should serve a local calibration origin, measure the effective browser fingerprint, cache it for the render session, and emit a concise calibration event plus an optional full report.

## Acceptance criteria

- [ ] External CDP sessions navigate to a local `__crawlex_calibrate` HTTP origin before the target
- [ ] Calibration captures core identity, screen/window, locale/timezone, WebGL, canvas/audio sample, storage quota, media, WebRTC, permissions, plugins, `window.chrome`, performance memory, and WebGPU where available
- [ ] Calibration result is represented as an effective browser fingerprint model
- [ ] Calibration is cached per render session and invalidated when endpoint, seed, proxy, locale, timezone, profile, or context identity changes
- [ ] A calibration summary event includes browser product, platform, locale, timezone, WebGL renderer, mismatch count, and policy
- [ ] Full fingerprint report output is available only when explicitly requested
- [ ] Tests cover probe result parsing and cache-key invalidation

## Blocked by

- `issues/30-slice.md`
