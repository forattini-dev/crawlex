# Slice 33: Calibration-aware stealth shim [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Make crawlex's stealth shim consume the effective browser fingerprint for external CDP sessions. The shim must remain active, but it should use calibrated values where available so it does not overwrite native stealth output with contradictory identity, screen, GPU, locale, storage, media, or WebGPU data.

## Acceptance criteria

- [ ] crawlex never disables its stealth shim for external CDP or native stealth providers
- [ ] Shim generation can consume calibrated UA, UA-CH, platform, languages, timezone, screen/window, WebGL, storage, media, and WebGPU values
- [ ] Existing stock IdentityBundle-driven shim behavior is preserved
- [ ] External CDP shim behavior prefers calibrated values over static defaults when present
- [ ] Worker-scope shim behavior remains active and coherent with calibrated values
- [ ] Tests verify the generated shim reflects calibrated values without changing stock-mode output
- [ ] Existing stealth shim leak/compliance tests continue to pass

## Blocked by

- `issues/32-slice.md`
