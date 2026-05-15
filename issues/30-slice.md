# Slice 30: External CDP render path [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Connect crawlex to a user-provided CDP endpoint and render a normal target through that external browser. The slice should create a crawlex-owned page or context when possible, preserve session isolation by default, and return explicit errors for unreachable or incompatible endpoints.

## Acceptance criteria

- [ ] `cdp` provider mode connects to a configured external CDP endpoint
- [ ] A simple HTML target can be rendered through the external endpoint end-to-end
- [ ] crawlex creates and cleans up its own page or context where the endpoint supports it
- [ ] Session isolation remains the default for external CDP usage
- [ ] Unreachable, invalid, or incompatible CDP endpoints produce actionable errors before target work continues
- [ ] Provider-selected events distinguish external CDP from stock Chromium
- [ ] Unit/integration coverage exercises a generic local CDP endpoint when available

## Blocked by

- `issues/29-slice.md`
