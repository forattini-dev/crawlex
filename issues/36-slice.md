# Slice 36: Explicit provider fallback [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Add opt-in provider fallback so crawlex can retry with a configured provider order only when the user explicitly enables it. Provider switching must be logged, reproducible, and never happen silently in the default stock or CDP modes.

## Acceptance criteria

- [ ] Provider fallback is disabled by default
- [ ] Users can configure an explicit provider order using vendor-neutral provider names
- [ ] crawlex does not switch providers unless fallback is explicitly enabled
- [ ] Fallback events record source provider, destination provider, and reason
- [ ] Fallback respects fingerprint mismatch policy and session mode decisions
- [ ] If no configured fallback provider is usable, crawlex reports the original failure and attempted fallback path
- [ ] Tests cover disabled fallback, configured fallback order, event emission, and failure when fallback providers are unavailable

## Blocked by

- `issues/29-slice.md`
- `issues/30-slice.md`
- `issues/34-slice.md`
- `issues/35-slice.md`
