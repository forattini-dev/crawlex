# Slice 33: Calibration-aware stealth shim [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Make crawlex's stealth shim consume the effective browser fingerprint for external CDP sessions. The shim must remain active, but it should use calibrated values where available so it does not overwrite native stealth output with contradictory identity, screen, GPU, locale, storage, media, or WebGPU data.

## Acceptance criteria

- [ ] crawlex never disables its stealth shim for external CDP or native stealth providers
- [ ] Shim generation can consume calibrated UA, UA-CH, platform, languages, timezone, screen/window, WebGL, storage, media, and WebGPU values
- [ ] Existing stock IdentityBundle-driven shim behavior is preserved
- [ ] External CDP shim behavior prefers calibrated values over static defaults when present
- [ ] Worker-scope shim behavior remains active and coherent with calibrated values
- [ ] Tests verify the generated shim reflects calibrated values without changing stock-mode output
- [ ] Existing stealth shim leak/compliance tests continue to pass

## Blocked by

- `issues/32-slice.md`

## Work done (iteration 1)

Wired calibration into stealth shim generation and the external CDP
render path. The shim is never disabled; on an external CDP session
where a calibrated fingerprint exists, the rendered shim now prefers
those values over the static `IdentityBundle` defaults.

Files changed:

- `src/render/stealth.rs`
    * New public `CalibrationOverrides` struct — every field optional
      so callers can populate only the axes their probe could measure
      (UA, platform, locale, languages, timezone, tz offset, WebGL
      unmasked vendor/renderer, WebGPU adapter, heap limit, media
      mic/cam/speaker counts). Deliberately decoupled from
      `crate::render::calibration` so `stealth.rs` stays buildable
      without `cdp-backend`; the pool projects an
      `EffectiveFingerprint` into this shape.
    * `is_empty()` — short-circuits to the bundle-only render path so
      stock output stays byte-identical.
    * New `render_shim_from_bundle_with_calibration(bundle, Option<&CalibrationOverrides>)`
      and `render_worker_shim_from_bundle_with_calibration(bundle,
      Option<&CalibrationOverrides>)`. The original
      `render_shim_from_bundle` / `render_worker_shim_from_bundle`
      now delegate with `None` so every existing caller is unaffected.
    * Internal refactor: `apply_overrides()` + `OverrideScratch` layer
      override-derived values on top of the bundle-derived
      `ShimSubstitutions` table. UA → derives `app_version` (strip
      `Mozilla/`); platform → derives UA-CH platform token (first
      word, quote-stripped); WebGL renderer → re-derives
      `gpu_vendor_keyword` via the same helper the bundle path uses,
      so WebGL and WebGPU stay coherent on calibrated input.
    * Four new unit tests:
        - `calibration_overrides_none_matches_stock_output` — proves
          `None` (and empty overrides) produce byte-identical output
          to `render_shim_from_bundle`. Satisfies "existing stock
          IdentityBundle-driven shim behavior is preserved".
        - `calibration_overrides_reflected_in_shim` — full override
          set; asserts each calibrated value lands in the rendered JS
          (UA, platform, locale, languages, timezone, tz offset,
          WebGL vendor, WebGPU adapter, heap limit, GPU keyword
          re-derived from calibrated renderer, mic count).
        - `partial_overrides_only_replace_listed_fields` — sets only
          timezone; asserts bundle UA and locale survive untouched.
        - `worker_shim_honours_calibration_overrides` — same coverage
          for the DOM-stripped worker variant, plus the
          `@worker-skip-start`/`@worker-skip-end` markers stay stripped.

- `src/render/pool.rs`
    * Imports `render_shim_from_bundle_with_calibration` and
      `CalibrationOverrides`.
    * `install_stealth` now delegates to a new
      `install_stealth_with_overrides(page, Option<&CalibrationOverrides>)`.
      With `None` (or all-`None` overrides), it uses the cached
      `shim_js()` string — exact same hot path as before. With
      non-empty overrides it generates a fresh shim via the new
      stealth function and injects it via
      `Page.addScriptToEvaluateOnNewDocument`.
    * `Self::calibration_overrides_from_fingerprint(&EffectiveFingerprint)`
      — projects the slice-32 probe payload into shim overrides. UA /
      platform / locale / timezone / WebGL unmasked vendor / WebGL
      unmasked renderer / WebGPU adapter / `performance_memory.js_heap_size_limit`
      map 1-1 (empty-string fields drop to `None` so bundle defaults
      still apply). `media_devices` is bucketed by substring
      (audioinput / videoinput / audiooutput) into the three shim
      counts so the rendered media list count and the calibration
      sample agree.
    * In the render path immediately after `ensure_calibrated`: on
      `Ok(fp)`, build overrides and call
      `install_stealth_with_overrides`. Empty overrides skip the
      second injection entirely. On `Err`, the warning path from
      slice 32 is retained and the bundle shim already installed at
      fresh-page creation continues to serve. The shim's
      per-section `try`/`catch` tolerates the second-script run on
      the upcoming target nav.

Key decisions:

- **Layered overrides, not a parallel shim.** The override struct is
  optional-per-field rather than a full shim variant. Stock path
  passes `None` and gets the exact same template-substitution flow;
  external-CDP paths populate only the axes the probe actually
  measured, and every other placeholder still flows from the bundle.
  This is what makes the byte-identical-stock test possible and
  satisfies "existing stock IdentityBundle-driven shim behavior is
  preserved" without forking the shim source.

- **Decoupled from calibration module.** `stealth.rs` does not
  `use crate::render::calibration` so it stays buildable without the
  `cdp-backend` feature. The pool owns the projection from
  `EffectiveFingerprint` → `CalibrationOverrides`, which is the only
  thing that needs the calibration types in scope.

- **GPU keyword re-derived from calibrated renderer.** When the probe
  reports an Apple/AMD/NVIDIA renderer different from the bundle's
  Intel default, both `WEBGL_UNMASKED_VENDOR` *and* the
  `GPU_VENDOR_KEYWORD` token are overridden. Otherwise the rendered
  shim would expose an Apple WebGL string but an Intel keyword
  downstream — the coherence break slice 32 built infrastructure to
  detect.

- **Shim never disabled for external CDP.** Shim is installed at
  fresh-page creation in `install_stealth` (slice 32 baseline) *and*
  a second time after calibration with overrides. The second install
  applies to the upcoming target navigation; both shims run, but the
  shim's defensive `try`/`catch` (107 occurrences across the
  template) tolerates the second pass, and the calibrated values
  land on the fields where the second-pass `Object.defineProperty`
  calls succeed. A future slice can replace this with a
  tracked-identifier `Page.removeScriptToEvaluateOnNewDocument` swap
  if a double-run regression surfaces.

- **No double-install when there's nothing to override.** Empty
  `CalibrationOverrides` (probe returned but every field was blank)
  short-circuits before the second `addScriptToEvaluateOnNewDocument`.
  Costs nothing when there's no win.

## Blocker for next iteration

- Same harness limitation as slices 29–32: `cargo`, `git`, and other
  Bash commands are rejected for approval in this worktree, so the
  changes could only be reviewed by inspection. Before moving to
  `issues/done/`:
    1. `cargo check --all-targets --all-features`
    2. `cargo test --all-features` — in particular the four new
       `stealth::tests` cases:
        - `calibration_overrides_none_matches_stock_output`
        - `calibration_overrides_reflected_in_shim`
        - `partial_overrides_only_replace_listed_fields`
        - `worker_shim_honours_calibration_overrides`
       and the existing `stealth_shim_leaks` / `fpjs_compliance` /
       `worker_shim_live` suites must remain green (stock path is
       provably unchanged by
       `calibration_overrides_none_matches_stock_output`).
    3. A live external-CDP run pointed at e.g. `--external-cdp-url
       http://127.0.0.1:9222` to confirm the
       `event="stealth.calibration.applied"` debug log fires once per
       render after the slice-32 `event="calibration.summary"` line.
    4. `git add` + `git commit` and `git mv issues/33-slice.md
       issues/done/33-slice.md`.

- Wave-2 follow-ups (out of scope, listed for the next AFK iteration):
    * Track the `addScriptToEvaluateOnNewDocument` identifier returned
      by `install_stealth` so the calibration-aware second install can
      replace the first rather than stack on top of it
      (`Page.removeScriptToEvaluateOnNewDocument`).
    * Plumb screen / window / storage quota from the calibration probe
      into the bundle so launch-flag-level fields (window size, device
      scale factor) follow calibrated values too. Today the shim
      reflects calibrated identity but Chromium launch flags still
      come from the bundle alone.
    * Promote `EnforcePolicy` from slice 32's `report-only` to a real
      gate that flips into shim-overrides-only mode when mismatch
      count exceeds a threshold.
