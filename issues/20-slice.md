# Slice 20: Ad-block bundled + EasyList updater [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Bundle a baseline list of ~3,500 ad/tracker domains into the binary via `include_str!`. URL gate matches request hostnames against the list (including subdomain match). Add `crawlex update-blocklist` subcommand that fetches EasyList and merges it into a local override file consulted at runtime. Browser-mode rendering and HTTP fetches both consult the gate.

## Acceptance criteria

- [ ] Baseline list embedded at build time (~3,500 domains)
- [ ] `adblock::is_blocked(url) -> bool` handles subdomain matches
- [ ] `crawlex update-blocklist` fetches EasyList, parses domain rules, writes to a user-config path
- [ ] Runtime consults baseline + local override; override wins
- [ ] Spider config exposes opt-in flag `adBlock: bool` (default off — opt-in to preserve current behavior)
- [ ] Tests: baseline match, subdomain match, update flow round-trip

## Blocked by

None - can start immediately
