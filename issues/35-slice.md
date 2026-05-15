# Slice 35: External CDP session mode [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Formalize how external CDP sessions use browser state. Keep isolated sessions as the default and add an explicit persistent mode for users who intentionally want to reuse cookies, localStorage, cache, or an existing backend profile.

## Acceptance criteria

- [ ] External CDP sessions default to isolated crawlex-owned page/context behavior where possible
- [ ] Persistent session mode can be selected explicitly through vendor-neutral config/CLI
- [ ] Persistent mode reuses backend browser state without silently changing the default isolation behavior
- [ ] Calibration cache keys account for isolated versus persistent session mode
- [ ] Logs/events identify the selected session mode
- [ ] Cleanup behavior is defined for isolated sessions and does not destroy persistent user state
- [ ] Tests cover isolated default, persistent selection, cleanup, and calibration cache separation

## Blocked by

- `issues/30-slice.md`
- `issues/32-slice.md`
