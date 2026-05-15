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

## Work done (iteration 1)

Wired explicit, opt-in provider fallback end-to-end through config,
CLI, and the render pool's preflight loop. Fallback is OFF by
default — `enabled=false` plus an empty `order` list — and stays
inert until both are set, so existing operators see no behavioural
change. When armed, a primary preflight failure walks the
vendor-neutral chain, emitting a structured `provider.fallback`
event per attempt (and `provider.fallback.skipped` when a candidate
isn't usable in the current config). If every fallback fails the
returned error preserves the original primary error and lists every
attempted destination on a single log line.

Files added/changed:

- `src/config.rs`
  * New `ProviderFallbackConfig { enabled: bool, order: Vec<BrowserProvider> }`
    with `#[serde(default)]` on both fields so configs serialized
    before slice 36 deserialize untouched.
  * Helpers: `is_active()` (both flag AND non-empty order required);
    `chain_after(primary)` (skip primary, drop `Auto`, dedupe in
    first-occurrence order — Auto is a meta-selector, never a
    concrete fallback target).
  * `Config.provider_fallback` field, default initialised.
  * Unit tests: default disabled+empty, inertness when only enabled
    or only order, primary-skip + dedupe in chain, Auto stripped,
    snake_case serde round-trip.

- `src/cli/args.rs`
  * `--provider-fallback-enable` (bool flag) and
    `--provider-fallback-order` (comma-separated CSV) on `CrawlArgs`.

- `src/cli/mod.rs`
  * `resolve_provider_fallback(enable_flag, order_flag)` — flag wins
    over `CRAWLEX_PROVIDER_FALLBACK_ENABLE` /
    `CRAWLEX_PROVIDER_FALLBACK_ORDER` env vars. Unknown order tokens
    rejected with `expected comma-separated stock|cdp|auto, got
    \`<token>\``. Empty tokens (`" , stock ,, "`) tolerated.
  * Builder threads the resolver through `provider_fallback`.
  * `apply_crawl_cli_overrides` re-resolves only when the operator
    actually touched a flag/env; enable-only and order-only
    mutations are additive (don't blow away the other half from the
    config file). `validate_browser_provider` now runs AFTER the
    fallback resolution so a Stock primary can keep
    `external_cdp_url` when a Cdp fallback is armed.
  * `validate_browser_provider`: Stock branch keeps the endpoint
    when `provider_fallback.is_active()` AND the order contains
    `Cdp`. Otherwise the historical "strip stale endpoint" behaviour
    is unchanged.
  * Tests: defaults disabled+empty, flag enables with CSV order,
    env-only enable/order, flag wins over env, unknown token
    rejected with hint, empty tokens tolerated. `clear_env` also
    strips the two new vars.

- `src/render/pool.rs`
  * `preflight()` rewritten as a fallback loop. Delegates to
    `preflight_for_provider(BrowserProvider)`; on primary failure
    consults `chain_after(primary)`; per attempt emits
    `provider.fallback` (or `provider.fallback.skipped` when
    `provider_usable_for_fallback` returns false). Final error
    preserves original primary error and joins every attempted
    destination — operators can read the full path off a single log
    line.
  * `effective_primary_provider()` — `Auto` collapses to `Cdp` when
    `external_cdp_url` is set, else `Stock`; concrete providers map
    to themselves.
  * `provider_usable_for_fallback(p)` — `Cdp` needs an endpoint;
    `Stock` is always eligible (chrome resolution can still fail at
    preflight time); `Auto` is never a concrete target.
  * `unusable_reason(p)` — stable strings that flow into the
    skipped-event reason and the final error path.
  * Fallback events include `mismatch_policy` and `session_mode` so
    operators can confirm slice 34 / slice 35 decisions ride along
    unchanged through a provider switch.
  * Tests (`provider_fallback_wiring`): primary-routing helper
    coverage (Auto+endpoint -> Cdp; Auto - endpoint -> Stock;
    Stock/Cdp pass-through); usability matrix (Cdp without endpoint,
    Auto never, Stock always); two `#[tokio::test(flavor =
    "current_thread")]` runtime tests forcing a deterministic CDP
    probe failure against `127.0.0.1:1` paired with a non-existent
    `chrome_path`: one asserts primary-only error path when fallback
    is OFF, one asserts the wrapped error AND attempted-path string
    when fallback is ON. A third tokio test exercises the skipped
    path: Stock primary + Cdp fallback without an endpoint surfaces
    `no external_cdp_url configured` in the final error.

Key decisions:

- `Auto` is filtered from the fallback chain at the config layer
  (`chain_after`) rather than the runtime layer. A concrete chain
  is easier to reason about, and `Auto` already encodes a
  preference — letting it appear as a fallback target would make
  the chain order ambiguous and produce a no-op redirect in 100% of
  cases.
- Endpoint preservation for Stock+Cdp-fallback lives in
  `validate_browser_provider`, not in pool runtime. Keeping the
  invariant "if `browser_provider=stock` and no fallback to Cdp,
  endpoint MUST be None" in one place avoids drift between layers
  that read `external_cdp_url`.
- `apply_crawl_cli_overrides` treats flag and order mutations as
  additive. Otherwise an operator setting only `--provider-
  fallback-enable` from the CLI would silently zero out the order
  list defined in their config file. Matches the principle from
  slice 35 that operator opt-ins should compose, not overwrite.
- Final error string format `"preflight failed for primary
  \`<p>\`: <orig>; fallback attempted: [<p1>=> <e1>; <p2>=> <e2>]"`
  is one line so it lands intact in structured log aggregators.
  Separator `=>` (no spaces around it) keeps `tracing` field-style
  parsers from confusing the destination with a key.
- Acceptance criterion "respects fingerprint mismatch policy and
  session mode decisions" is satisfied implicitly: fallback never
  mutates `config.mismatch_policy` or
  `config.external_cdp_session_mode`. The fallback event includes
  both fields so a downstream observer can confirm the invariant
  per attempt without instrumenting the pool internals.
- The actual *runtime* hot-swap (mid-render switching of
  `external_cdp_url` / chrome path after a render-time failure) is
  intentionally not part of this slice. Preflight-time fallback
  covers the dominant operational case (operator-owned CDP host
  unreachable at startup) and is the minimum viable end-to-end
  slice that satisfies every acceptance bullet. Mid-render
  hot-swap belongs to a future slice — Pool's `Arc<Config>` is
  immutable, so a swap requires interior mutability + audit of
  every `self.config.external_cdp_url` reader.

## Blocker for next iteration

- Same harness limitation as slices 29–35: `cargo` and `git` Bash
  commands are rejected for approval in this worktree, so the
  changes could only be reviewed by inspection. Before moving the
  issue to `issues/done/`:
    1. `cargo check --all-targets --all-features`
    2. `cargo test --all-features` — in particular:
        * `config::tests::provider_fallback_*` (4 new tests)
        * `cli::browser_provider_tests::resolve_provider_fallback_*`
          (6 new tests)
        * `render::pool::provider_fallback_wiring::*` (8 new tests,
          including 3 `#[tokio::test]` cases)
    3. `git add -A && git commit -m "feat(provider): explicit
       provider fallback (slice 36)"` + `git mv issues/36-slice.md
       issues/done/36-slice.md`.
