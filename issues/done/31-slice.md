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

## Work done (iteration 1)

Added a small, vendor-neutral capability layer on top of the existing
external-CDP probe. The pool now classifies the configured endpoint as
`generic_cdp` or `native_stealth` based on what `/json/version`
advertises, then attaches identity hints to the connection URL only
when the endpoint says it supports them.

Files changed:

- `src/render/cdp_probe.rs` — `VersionPayload` parses an optional
  non-standard `Stealth-Provider` field and surfaces it through a new
  `ProbeOk::stealth_provider: String` field. Existing fields and probe
  failure modes are unchanged, so slice 30's preflight contract still
  holds.
- `src/render/cdp_capabilities.rs` (new) — capability interface
  isolated behind `EndpointCapabilities { kind, identity_params,
  vendor }` plus an `EndpointKind { GenericCdp, NativeStealth }`
  enum. `EndpointCapabilities::detect(&ProbeOk)` is a pure function:
    * non-empty `Stealth-Provider` → `NativeStealth` with `vendor` =
      that field;
    * `Browser` banner starting with `cloakserve/` (case-insensitive)
      → `NativeStealth` with `vendor` = banner;
    * otherwise → `GenericCdp` with `identity_params=false`.
  `build_connect_url(base, &IdentityHints)` returns `base` unchanged
  for generic endpoints (or when no hints are set); for
  identity-aware endpoints it appends populated hints
  (`seed`, `timezone`, `locale`, `proxy`, `geoip`) as query pairs
  while preserving any existing query string. In-file tests cover
  every detection path and URL-builder branch.
- `src/render/mod.rs` — `pub mod cdp_capabilities` (gated on
  `cdp-backend`, alongside `cdp_probe`).
- `src/render/pool.rs` —
    * New field `cdp_capabilities:
      RwLock<Option<EndpointCapabilities>>` cached after preflight so
      `ensure_browser` can re-read it per session without re-probing.
    * `preflight()` now calls `EndpointCapabilities::detect` after a
      successful probe and stamps the result. The
      `event="provider.selected"` log line gains
      `endpoint_kind`, `identity_params`, and `vendor` fields so the
      decision is observable without parsing free text.
    * The cdp branch of `ensure_browser` reads the cached capability
      (defaults to generic on miss — safest), assembles
      `IdentityHints` from `config.timezone`, `config.locale`, and
      the per-job `proxy`, and connects via
      `caps.build_connect_url(...)`. Generic endpoints connect with
      the original URL, exactly as before slice 31. Native-stealth
      endpoints get the identity query string.
- `tests/external_cdp_capabilities.rs` (new) — wiremock-backed
  integration coverage of detection + URL building for:
    * a plain Chromium `/json/version` response (generic kind, base
      URL preserved even with hints);
    * a cloakserve-like `Browser` banner (native-stealth, all five
      hint pairs round-trip through the connection URL);
    * an explicit `Stealth-Provider` field on top of an unchanged
      `Chrome/...` banner (native-stealth, vendor = that field).

Key decisions:

- Capability is modelled as *what the endpoint supports*, not which
  vendor it is, so the `cdp` branch of `ensure_browser` stays free of
  brand-specific control flow. Adding another stealth multiplexer in
  the future means setting `Stealth-Provider` (zero-config) or
  extending the banner-matching rule in one place.
- No new public CLI/config flags were introduced. `seed`/`geoip` are
  plumbed through `IdentityHints` but left `None` for now: there is
  no dedicated config surface for them yet, and adding one belongs to
  a follow-up slice. `timezone`, `locale`, and per-job `proxy` are
  already first-class config and flow through immediately.
- "Detection failures fall back to generic CDP behavior when safe":
  if preflight has not run (e.g. the pool ever calls `ensure_browser`
  before `preflight`), `cdp_capabilities` is `None` and we treat the
  endpoint as `Generic` — never appending identity params. Probe-level
  failures continue to be hard preflight errors (slice 30), which is
  the "produce clear errors when not safe" half of the same criterion.
- Detection runs against a successful `/json/version` payload only.
  We deliberately do *not* probe twice (e.g. once HTTP, once over the
  WS upgrade); cloakserve-like multiplexers are expected to advertise
  themselves on the same JSON endpoint Chromium already exposes.

## Blocker for next iteration

- Same harness limitation as slices 29/30: `cargo`, `git`, and other
  Bash commands are rejected for approval in this worktree, so the
  changes could only be reviewed by inspection. Before moving the
  issue to `issues/done/`:
    1. `cargo check --all-targets --all-features`
    2. `cargo test --all-features` — in particular
       `external_cdp_capabilities` (new file) and the
       `cdp_capabilities::tests` module.
    3. `git add` + `git commit` and `git mv issues/31-slice.md
       issues/done/31-slice.md`.
