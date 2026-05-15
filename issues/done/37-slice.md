# Slice 37: Native provider live validation harness [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Add optional live validation coverage and a manual validation workflow for native stealth providers. The harness should compare stock Chromium and a configured native CDP endpoint on representative fingerprint and bot-detection pages without making public benchmark claims.

## Acceptance criteria

- [ ] Live tests are ignored or gated unless an external endpoint is configured
- [ ] Harness can run stock Chromium and configured CDP provider against the same validation target set
- [ ] Validation captures provider-selected events, calibration summaries, and optional fingerprint reports
- [ ] Manual workflow covers BrowserScan and FingerprintJS-style comparisons without asserting public pass-rate claims
- [ ] Documentation warns that CloakBrowser binary use is user-managed and not redistributed by crawlex
- [ ] CI does not require CloakBrowser, Camoufox, or any third-party stealth binary
- [ ] Human review is required before using validation output in public docs or marketing material

## Blocked by

- `issues/31-slice.md`
- `issues/32-slice.md`
- `issues/33-slice.md`
