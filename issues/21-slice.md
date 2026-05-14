# Slice 21: curl→config converter [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

`crawlex from-curl '<curl command>'` parses a curl invocation and emits an equivalent crawlex config (TOML/JSON) or Node SDK snippet. Supports headers, cookies, request body, HTTP method, redirects, and proxy flags. Useful for converting devtools-copied requests into crawlex runs.

## Acceptance criteria

- [ ] `from_curl` module parses curl flags: `-H`, `-b`, `-d/--data`, `-X`, `-L`, `-x/--proxy`, `--data-raw`, `--compressed`
- [ ] Subcommand `crawlex from-curl '<string>'` prints config to stdout
- [ ] Flag `--format toml|json|node` selects output shape
- [ ] Round-trip test: curl from Chrome devtools → config → request that hits the same fixture endpoint with identical headers/body
- [ ] Unknown flags warn and continue rather than fail

## Blocked by

None - can start immediately
