# Slice 31: Native stealth endpoint capability detection [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Detect when a configured external CDP endpoint behaves like a native stealth multiplexer such as cloakserve, then pass high-level identity constraints through the endpoint contract before connecting. Keep the public API vendor-neutral and model this internally as capabilities rather than brand-specific control flow.

## Acceptance criteria

- [ ] Endpoint detection identifies generic CDP endpoints separately from cloakserve-like endpoints
- [ ] Detection is isolated behind a small capability interface
- [ ] For compatible endpoints, crawlex can attach seed, timezone, locale, proxy, and geoip constraints to the connection URL
- [ ] For generic CDP endpoints, crawlex connects without assuming query-string identity support
- [ ] No vendor-specific public CLI/config flags are introduced
- [ ] Detection failures fall back to generic CDP behavior when safe, or produce clear errors when not safe
- [ ] Tests cover generic CDP responses and cloakserve-like HTTP/CDP responses

## Blocked by

- `issues/30-slice.md`
