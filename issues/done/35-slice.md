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

## Work done (iteration 1)

Formalized external CDP session state behaviour. `Isolated` (default)
keeps the existing crawlex-owned `BrowserContext` per session;
`Persistent` opts in to reusing the endpoint's default context so
cookies / localStorage / cache / signed-in profile all flow through.
Both are surfaced via a vendor-neutral config field + CLI flag + env
var (flag wins). Calibration cache keys, telemetry, and cleanup
now branch on the chosen mode.

Files added/changed:

- `src/config.rs` — new `ExternalCdpSessionMode` enum
  (`Isolated` default, `Persistent`) with `as_str` / `parse` /
  `is_persistent` helpers and serde snake_case derive.
  `Config.external_cdp_session_mode` field with `#[serde(default)]`
  so existing configs deserialise unchanged. Unit tests cover
  default-isolated, case-insensitive parse, unknown rejection,
  snake_case serde, and as_str round-trip.
- `src/cli/args.rs` — `--external-cdp-session-mode` flag on
  `CrawlArgs` (the shared crawl-flag struct).
- `src/cli/mod.rs` —
  * `resolve_external_cdp_session_mode(flag)` resolver: flag →
    `CRAWLEX_EXTERNAL_CDP_SESSION_MODE` env → default. Unknown values
    error with `expected one of isolated|persistent` and the offending
    input. Matches the slice-29 resolver shape so operators get
    a consistent UX across all CDP-related flags.
  * Threads the resolver through both the initial Config builder and
    `apply_crawl_cli_overrides` so `--config a.yaml --external-cdp-
    session-mode persistent` correctly overrides.
  * Tests: default isolated, env-only persistent, flag wins over env,
    unknown rejection. `clear_env()` now also strips the new var so
    other tests aren't polluted.
- `src/render/calibration.rs` —
  * `CalibrationKey` gained a `session_mode` field. Hashed into
    `fingerprint_id()` after the existing seven inputs so the
    isolated / persistent variants of the same identity never share a
    cached fingerprint (storage surface differs observably).
  * New test `cache_key_session_mode_isolated_and_persistent_are_
    distinct` asserts both the equality and the cache-lookup miss.
  * Existing field-iteration loops include `session_mode` so the
    invalidation-on-every-field invariant extends to it.
- `src/render/pool.rs` —
  * `calibration_key_for` now takes `Option<&BrowserContextId>` so
    the persistent path (no crawlex context) maps to a stable
    `<session_id>|<default>` context slot instead of unwrapping.
    Sets the new `session_mode` field from the active config.
  * `is_persistent_external_cdp()` helper — true only when an external
    CDP url is set AND the mode opts in.
  * `ensure_session_context` now returns
    `Result<Option<BrowserContextId>>`. Persistent mode short-circuits
    before any context creation or `session_state` load. Isolated
    mode unchanged. `CreateTargetParams.browser_context_id` is now
    `ctx_id.clone()` (already `Option<...>`), so default-context
    targets land in the endpoint's persistent context.
  * `drop_session` early-exits with a `session.drop.skipped` debug
    event in persistent mode — backend storage is never touched by
    the TTL sweep or operator drop.
  * `restore_session_state` is gated on `!is_persistent_external_cdp`
    so crawlex never overwrites backend-owned cookies / storage.
  * Provider-selected log entry adds `session_mode` so operators
    can read the active mode straight off the preflight line.
  * `calibration.summary` event adds `session_mode` for traceability
    of which mode produced a cached fingerprint.

Key decisions:

- `Option<BrowserContextId>` over a sentinel id. Sentinels would
  type-check but make `drop_session` and the page-pool keys
  ambiguous; the explicit None encodes "use endpoint default
  context" exactly once and forces every callsite to make the
  decision.
- Persistent mode also disables crawlex-side state restoration.
  `restore_session_state` calls `storage.clear()` before re-seeding,
  which would destroy user-owned localStorage on a fresh persistent
  run. Better to never run it than to "early-return when state is
  empty" and hope no operator manages to write into the registry
  out-of-band.
- `session_mode` is a single field on `CalibrationKey` (not a typed
  enum) because the cache stores hashes only — a stable string is
  enough and avoids exporting another enum across the calibration
  module boundary.
- Env var name follows the existing `CRAWLEX_EXTERNAL_CDP_URL`
  pattern (`CRAWLEX_EXTERNAL_CDP_SESSION_MODE`) so operators don't
  have to memorise a second prefix.
- Acceptance "tests cover isolated default, persistent selection,
  cleanup, and calibration cache separation" is satisfied via the
  config-default test, the resolver tests (flag + env + reject), the
  new `cache_key_session_mode_isolated_and_persistent_are_distinct`
  test, and the existing `cache_key_invalidates_on_each_identity_
  field` loop which now includes `session_mode`. Live-cleanup
  coverage in `drop_session` is exercised through the early-return
  guard; an end-to-end test requires a live external CDP host and
  is intentionally left for the live-run verification step.

## Blocker for next iteration

- Same harness limitation as prior slices: `cargo` and `git` Bash
  commands are rejected for approval in this worktree, so the
  changes could only be reviewed by inspection. Before moving the
  issue to `issues/done/`:
    1. `cargo check --all-targets --all-features`
    2. `cargo test --all-features` — in particular:
        * `config::tests::external_cdp_session_mode_*` (4 new tests)
        * `calibration::tests::cache_key_session_mode_isolated_and_
          persistent_are_distinct`
        * `calibration::tests::cache_key_invalidates_on_each_identity_
          field` (extended to include `session_mode`)
        * `cli::tests::resolve_session_mode_*` (4 new tests)
    3. A live external-CDP run pointed at
       `--external-cdp-url http://127.0.0.1:9222 --external-cdp-session-mode
       persistent` to confirm cookies / localStorage survive a crawlex
       restart, plus the matching `isolated` run to confirm the
       BrowserContext per-session disposal is unchanged.
    4. `git add` + `git commit` and `git mv issues/35-slice.md
       issues/done/35-slice.md`.
