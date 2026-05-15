# Slice 34: Fingerprint mismatch policy [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Classify calibration mismatches and add adapt versus strict behavior. The default adapt policy should warn and continue with calibrated values when possible; strict policy should fail before target navigation when a critical mismatch cannot be reconciled.

## Acceptance criteria

- [ ] Calibration compares expected session intent with the effective browser fingerprint
- [ ] Critical mismatch categories include browser family/version, proxy/IP/WebRTC coherence, locale/timezone, platform, and storage/profile contradictions
- [ ] Default adapt policy records mismatches, emits warning events, and continues with calibrated values where possible
- [ ] Strict policy fails before target navigation on critical unreconciled mismatches
- [ ] Non-critical mismatches are surfaced without failing strict runs unless explicitly classified as critical
- [ ] Error and event payloads include enough context to debug the mismatch without dumping the full fingerprint by default
- [ ] Tests cover adapt and strict behavior for critical and non-critical mismatches

## Blocked by

- `issues/32-slice.md`
- `issues/33-slice.md`

## Work done (iteration 1)

Classification + adapt/strict policy plumbed into external-CDP
calibration path.

Files changed:

- `src/config.rs`
    * New `MismatchPolicy` enum (`Adapt` default, `Strict`) at config
      scope so the type is available without the `cdp-backend`
      feature; the calibration module re-exports it. `parse()` +
      `as_str()` for round-trip with CLI/env strings.
    * `Config::mismatch_policy: MismatchPolicy` (default `Adapt`).
      Wired into the default initializer.

- `src/render/calibration.rs`
    * `pub use crate::config::MismatchPolicy;` re-export.
    * `MismatchCategory` — 8 variants: `BrowserFamily`,
      `BrowserVersion`, `ProxyIpWebrtc`, `Locale`, `Timezone`,
      `Platform`, `StorageProfile`, `GpuClass` (reserved for
      non-critical).
    * `MismatchSeverity` — `Critical` / `NonCritical`.
    * `Mismatch { category, severity, expected, observed, reconcilable }`
      — short string fields cap at 64 chars so event payloads don't
      leak the full fingerprint surface.
    * `ExpectedIdentity` — optional-per-axis declaration of session
      intent (browser_family, browser_major, platform, locale,
      timezone, proxy_egress_ipv4, profile_id, min_storage_quota).
    * `classify_mismatches(fp, expected) -> Vec<Mismatch>`:
        - BrowserFamily mismatch: `Critical`, NOT reconcilable.
        - BrowserVersion mismatch (major delta): `Critical`,
          reconcilable (shim rewrites JS-visible UA/UA-CH).
        - Platform mismatch (substring, case-insensitive): `Critical`,
          reconcilable.
        - Locale mismatch (case-insensitive): `Critical`, reconcilable.
        - Timezone mismatch (exact): `Critical`, reconcilable.
        - ProxyIpWebrtc: when `expected.proxy_egress_ipv4` is set,
          any *public* IPv4 in `fp.webrtc.ipv4` that does not equal
          the expected egress is `Critical`, NOT reconcilable
          (`is_public_ipv4` filters RFC1918 / loopback / CGNAT /
          link-local so a private leak is not flagged as proxy
          incoherence).
        - StorageProfile: observed quota < ½ of declared minimum, or
          `profile_id` declared and observed quota is zero. Both
          `Critical` and NOT reconcilable.
    * `has_unreconciled_critical(&[Mismatch]) -> bool` — the strict
      gate.
    * `MismatchReport<'a>` — event payload struct. Aggregates
      counts (`critical` / `non_critical` / `unreconciled_critical`),
      collects distinct category strings, and borrows the slice of
      `Mismatch` for the JSON dump. Deliberately does NOT carry the
      full `EffectiveFingerprint`.
    * Ten new unit tests covering: policy parsing + defaults; empty
      expectations → empty result; each critical category in turn;
      reconcilable flag per category; private-IP WebRTC is NOT a
      proxy leak; report counts + JSON shape; non-critical entries
      never trip `has_unreconciled_critical`; strict-safe path when
      every critical is reconcilable.

- `src/render/pool.rs`
    * `expected_identity_for_session()` — projects active config +
      `IdentityBundle` into `ExpectedIdentity`. Hard-codes
      `browser_family = "Chromium"` (the pool only manages
      Chromium-family endpoints), bundle's `ua_major` for
      browser_major, `ua_platform` stripped of quote chars, config
      locale/timezone (only when set), bundle id as `profile_id`.
      `proxy_egress_ipv4` and `min_storage_quota` are left `None`
      until operator-facing knobs are added — keeps adoption
      friction zero.
    * In the external-CDP render path, immediately after
      `ensure_calibrated` returns `Ok(fp)`:
        - Classify mismatches.
        - When non-empty, emit `event="calibration.mismatch"` at
          `WARN` level with `policy`, `critical`, `non_critical`,
          `unreconciled_critical`, `categories` (debug-printed
          vec), and a `report` JSON blob. The blob carries each
          `Mismatch` (category, severity, expected, observed,
          reconcilable) — no full-fingerprint dump.
        - When `policy == Strict` AND
          `has_unreconciled_critical(&mismatches)`, return
          `Err(Error::Render("strict calibration policy: ..."))`
          BEFORE `restore_session_state` and the target
          `NavigateParams` execute. The categories that triggered
          the abort are listed in the error message for debugging.
    * Slice 33 shim-override injection is unaffected — runs after
      the classify/warn block on the `Adapt` path.

Key decisions:

- **Policy type lives in `config` not `calibration`.** Config is
  unconditional; calibration is `cdp-backend`-only. Defining the
  enum in config and re-exporting from calibration lets the
  `Config` struct hold a default-valued policy field without
  requiring the cdp-backend feature, while callers inside the
  calibration module still write `MismatchPolicy` ergonomically.

- **`reconcilable` is a per-mismatch property, not a category-wide
  one.** Browser *family* (Firefox vs Chromium) cannot be shimmed,
  but browser *version* (Chrome 120 vs 131) can be — the shim
  rewrites the JS-visible UA/UA-CH surface. Recording the flag on
  the entry rather than the category lets strict policy fail only
  when a divergence is genuinely beyond shim reach.

- **Proxy IP leak detection filters private addresses.** RFC1918,
  loopback, link-local, and CGNAT (100.64.0.0/10) ranges are NOT
  treated as egress IPs. A browser behind a proxy still surfaces
  its LAN IP via WebRTC; if we flagged that as a critical
  divergence every proxied run would fail strict. The check only
  fires on public IPv4 that doesn't equal the declared egress.

- **Storage/profile uses observable signals.** We can't read a
  profile's internal id from JS; the closest observable is
  `storage_quota`. A quota under half the declared minimum, or a
  zero quota under a declared profile id, indicates the live
  session is backed by a different storage profile than intended.

- **Event payload, not full fingerprint.** The `Mismatch` struct's
  `expected`/`observed` strings cap at 64 chars; the
  `MismatchReport` aggregates counts and lists categories. The
  full `EffectiveFingerprint` is still gated behind
  `CRAWLEX_CALIBRATION_REPORT=full` (slice 32 baseline) — slice 34
  adds NO new full-fingerprint dumps to default logs.

- **`Adapt` warns and continues unconditionally.** Even when every
  critical mismatch is unreconcilable, the default policy emits
  the warning event and lets slice 33 shim overrides apply the
  reconcilable axes. Operators that need a hard stop opt into
  `Strict`.

## Blocker for next iteration

- Same harness limitation as slices 29–33: `cargo`, `git`, and
  other Bash commands are rejected for approval in this worktree,
  so the changes could only be reviewed by inspection. Before
  moving to `issues/done/`:
    1. `cargo check --all-targets --all-features`
    2. `cargo test --all-features` — the 11 new
       `render::calibration::tests` cases must pass:
        - `policy_parse_round_trips`
        - `classify_no_expectations_returns_empty`
        - `classify_locale_and_timezone_flagged_critical_reconcilable`
        - `classify_browser_family_critical_not_reconcilable`
        - `classify_browser_major_reconcilable`
        - `classify_platform_reconcilable`
        - `classify_proxy_ip_leak_flagged_unreconcilable`
        - `classify_proxy_ip_private_address_is_not_a_leak`
        - `classify_storage_quota_below_minimum_is_unreconcilable`
        - `classify_profile_id_contradiction_when_storage_zero`
        - `mismatch_report_aggregates_counts_and_categories`
        - `non_critical_does_not_trigger_strict_failure`
        - `adapt_does_not_flag_strict_when_only_reconcilable_critical`
       Existing slice 32 (`count_mismatches_*`, `parse_probe_*`,
       `cache_*`) and slice 33 (`calibration_overrides_*`,
       `worker_shim_honours_calibration_overrides`) tests must
       remain green.
    3. A live external-CDP run with `mismatch_policy = Strict` and
       deliberately mismatched locale/timezone via reconcilable
       axes should continue without error (slice 33 overrides
       apply); a run with a non-Chromium endpoint or a known
       WebRTC leak should fail with the
       `event="calibration.mismatch"` warn line preceding the
       `strict calibration policy: ...` render error.
    4. `git add` + `git commit` and `git mv issues/34-slice.md
       issues/done/34-slice.md`.

- Wave-2 follow-ups (out of scope, listed for the next AFK
  iteration):
    * Operator surface for `proxy_egress_ipv4` and
      `min_storage_quota` — today these expectations live in code
      only because there is no CLI / config knob yet. Once added,
      the proxy/IP/WebRTC and storage/profile critical categories
      will fire on real runs.
    * Plumb `MismatchReport` into the structured event sink (in
      addition to `tracing::warn!`) so downstream consumers can
      alert on `unreconciled_critical > 0` without log parsing.
    * Promote `EffectiveFingerprint.policy` (the probe-supplied
      hint) into a third strict-tier that enforces probe-declared
      mismatch budgets — out of scope for slice 34 because the
      probe doesn't carry budgets yet.
