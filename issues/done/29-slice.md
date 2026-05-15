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

## Work done (iteration 1)

Implemented vendor-neutral `BrowserProvider` (stock/cdp/auto) end-to-end.
Files changed:

- `src/config.rs` — added `BrowserProvider` enum (snake_case serde, default `Stock`),
  `Config::browser_provider` field, unit tests covering default + parse + serde
  roundtrip.
- `src/cli/args.rs` — added `--browser-provider <stock|cdp|auto>`; documented
  `CRAWLEX_EXTERNAL_CDP_URL` as the env-var twin for `--external-cdp-url`.
- `src/cli/mod.rs`:
  - `build_config_from_args` now wires `browser_provider`, reads
    `CRAWLEX_EXTERNAL_CDP_URL` when no CLI flag, and calls
    `validate_browser_provider`.
  - `apply_crawl_cli_overrides` honours both env vars and re-validates after
    config-file merge.
  - New helpers: `resolve_external_cdp_url`, `read_external_cdp_env`,
    `resolve_browser_provider`, `validate_browser_provider`.
  - New `browser_provider_tests` unit module (mutex-guarded env access)
    covers default-stock, env-only, flag-wins-over-env, unknown-value rejection,
    cdp-without-endpoint error, stock-strips-stale-endpoint, auto-keeps-optional.
- `src/render/pool.rs` — `preflight()` now emits a structured
  `event="provider.selected"` tracing log entry with `provider="stock"|"cdp"`
  plus the endpoint or Chrome path.

Key decisions:

- Default kept as `Stock` so existing crawls see no behavioural change.
- `Cdp` requires an explicit endpoint (CLI or `CRAWLEX_EXTERNAL_CDP_URL`); no
  silent local discovery, no fallback. `Stock` strips any leftover
  `external_cdp_url` to avoid drift between provider and endpoint.
- `Auto` accepts either state but never probes for a local CDP — endpoint must
  be configured up-front to be considered.
- Provider-selected signal lives in the tracing layer (acceptance criterion
  allows event OR log entry); avoids hauling an `EventSink` through the pool
  for a single emit site.

## Blocker for next iteration

- Bash exec for `cargo check` / `cargo test` / `git status` / `git commit` is
  refused by the harness in this worktree (the rtk hook rewrites and the
  resulting commands need approval). The code changes were reviewed by
  inspection only — they still need:
  1. `cargo check --all-targets --all-features`
  2. `cargo test --all-features` (especially the new
     `browser_provider_tests` module and `config::tests::browser_provider_*`)
  3. A `git commit` once the build is green.
- File is left in `issues/` (not `issues/done/`) until the compile/test
  feedback loop passes.
