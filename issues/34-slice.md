# Slice 34: Fingerprint mismatch policy [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Classify calibration mismatches and add adapt versus strict behavior. The default adapt policy should warn and continue with calibrated values when possible; strict policy should fail before target navigation when a critical mismatch cannot be reconciled.

## Acceptance criteria

- [ ] Calibration compares expected session intent with the effective browser fingerprint
- [ ] Critical mismatch categories include browser family/version, proxy/IP/WebRTC coherence, locale/timezone, platform, and storage/profile contradictions
- [ ] Default adapt policy records mismatches, emits warning events, and continues with calibrated values where possible
- [ ] Strict policy fails before target navigation on critical unreconciled mismatches
- [ ] Non-critical mismatches are surfaced without failing strict runs unless explicitly classified as critical
- [ ] Error and event payloads include enough context to debug the mismatch without dumping the full fingerprint by default
- [ ] Tests cover adapt and strict behavior for critical and non-critical mismatches

## Blocked by

- `issues/32-slice.md`
- `issues/33-slice.md`
