# Slice 29: Neutral browser provider selection [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Add neutral provider selection for the render path while preserving the current stock Chromium behavior as the default. Expose stock, cdp, and auto provider modes through the existing configuration/CLI surface, allow an environment-variable endpoint for CDP mode, prevent silent local endpoint discovery, and emit a structured provider-selected event.

## Acceptance criteria

- [ ] Default behavior remains the current stock Chromium/fetcher flow with no new required flags
- [ ] Config and CLI accept `stock`, `cdp`, and `auto` provider modes using vendor-neutral naming
- [ ] External CDP endpoint can be supplied through config/CLI and an environment variable
- [ ] `auto` only considers external endpoints when explicitly selected or configured
- [ ] No CloakBrowser- or Camoufox-specific public flags are added
- [ ] A provider-selected event/log entry is emitted for stock and configured CDP modes
- [ ] Existing stock browser launch and stealth runtime tests still pass

## Blocked by

None - can start immediately
