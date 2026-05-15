# Slice 32: Per-session browser fingerprint calibration [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Run a mandatory calibration step for external CDP sessions before navigating to the target. crawlex should serve a local calibration origin, measure the effective browser fingerprint, cache it for the render session, and emit a concise calibration event plus an optional full report.

## Acceptance criteria

- [ ] External CDP sessions navigate to a local `__crawlex_calibrate` HTTP origin before the target
- [ ] Calibration captures core identity, screen/window, locale/timezone, WebGL, canvas/audio sample, storage quota, media, WebRTC, permissions, plugins, `window.chrome`, performance memory, and WebGPU where available
- [ ] Calibration result is represented as an effective browser fingerprint model
- [ ] Calibration is cached per render session and invalidated when endpoint, seed, proxy, locale, timezone, profile, or context identity changes
- [ ] A calibration summary event includes browser product, platform, locale, timezone, WebGL renderer, mismatch count, and policy
- [ ] Full fingerprint report output is available only when explicitly requested
- [ ] Tests cover probe result parsing and cache-key invalidation

## Blocked by

- `issues/30-slice.md`

## Work done (iteration 1)

Added per-session browser fingerprint calibration for the external CDP
provider. The pool now serves a local `__crawlex_calibrate` HTTP origin
on `127.0.0.1:<ephemeral>`, and external-CDP renders navigate the
acquired page to that origin *before* the target, run a calibration
probe, parse the JSON result into an effective fingerprint model, and
cache it per session. Cache key folds in every input that legitimately
changes the live identity, so any of endpoint / seed / proxy / locale /
timezone / profile / context produces a fresh slot.

Files added:

- `src/render/calibration.rs` (new) — full module:
    * `EffectiveFingerprint` (browser product/version, platform, UA,
      locale, timezone, screen, window, WebGL, canvas/audio sample,
      storage quota, media devices, WebRTC, permissions, plugins,
      `window.chrome`, perf-memory, WebGPU adapter, mismatch_count,
      policy) — the data model that satisfies "calibration result is
      represented as an effective browser fingerprint model".
    * `CalibrationKey` (endpoint/seed/proxy/locale/timezone/profile/
      context) + `fingerprint_id()` helper. Field separator (`\x1f`)
      prevents concatenation collisions.
    * `CalibrationCache` — `RwLock<HashMap<CalibrationKey, Arc<…>>>`
      with `get` / `insert` / `format_full_report`. Cache is invalidated
      naturally because a different key hashes to a different slot.
    * `parse_probe(json)` — tolerant deserializer with operator-readable
      errors; defaults `policy="report-only"` when the probe omits it.
    * `count_mismatches(fp, expected_locale, expected_timezone)` —
      case-insensitive locale, strict TZ.
    * `CalibrationSummary` — concise serializable summary (browser
      product, platform, locale, timezone, WebGL renderer, mismatch
      count, policy). Does *not* carry the full fingerprint surface.
    * `serve_calibration_origin()` — minimal tokio-loopback HTTP server
      that answers every request with `CALIBRATION_HTML`. Returns a
      `CalibrationOrigin` whose `_shutdown` `oneshot::Sender` tears the
      listener down on drop.
    * `CALIBRATION_PROBE_JS` (include_str of the JS probe) and
      `CALIBRATION_HTML` / `CALIBRATION_PATH` constants.
    * 14 unit tests covering full + sparse parse, empty/garbage parse,
      `count_mismatches` edge cases, cache get/insert, **cache-key
      invalidation on every one of the seven identity fields**,
      hash-collision-via-concatenation guard, full-report shape, JS
      probe surface coverage (asserts navigator/screen/Intl/WebGL/
      OfflineAudioContext/storage/mediaDevices/RTCPeerConnection/
      permissions/plugins/window.chrome/performance.memory/navigator.gpu
      are all referenced), summary-event shape, and a tokio integration
      test that boots the local origin and `reqwest`s the HTML back.
- `src/render/calibration_probe.js` (new) — best-effort capture of every
  surface required by the acceptance criteria. Each individual capture
  is wrapped in a `safe(fn, fallback)` helper so a stripped browser
  cannot abort the whole probe; the IIFE returns a single
  `JSON.stringify`d payload.

Files changed:

- `src/render/mod.rs` — `pub mod calibration;` (gated on `cdp-backend`).
- `src/render/pool.rs` —
    * New fields `calibration_cache:
      Arc<crate::render::calibration::CalibrationCache>` and
      `calibration_origin:
      tokio::sync::OnceCell<crate::render::calibration::CalibrationOrigin>`.
    * `RenderPool::ensure_calibrated(&self, page, key)` — returns the
      cached fingerprint on hit; on miss lazily binds the local origin,
      navigates, runs `CALIBRATION_PROBE_JS` via `EvaluateParams`
      (`return_by_value=true`, `await_promise=true`), parses the
      payload, fills in `mismatch_count` against
      `config.locale`/`config.timezone`, emits
      `event="calibration.summary"` (browser product, platform, locale,
      timezone, WebGL renderer, mismatch count, policy), and inserts
      into the cache. Full report is **only** logged when the env var
      `CRAWLEX_CALIBRATION_REPORT=full` is set — satisfies "Full
      fingerprint report output is available only when explicitly
      requested".
    * `calibration_key_for(proxy, session_id, ctx_id)` — composes the
      seven-field key from `config.external_cdp_url`,
      `bundle.canvas_audio_seed`, the per-job proxy, `config.locale`,
      `config.timezone`, `bundle.id`, and the
      `<session_id>|<browser_context_id>` pair.
    * Hook in `render(...)` immediately after the page lease is
      acquired and before `restore_session_state`/warmup/target nav:
      when `config.external_cdp_url.is_some()`, build the key and call
      `ensure_calibrated`. Calibration failures are logged but
      non-fatal — the cache stays empty and the next render retries.

Key decisions:

- Calibration runs **inside the existing per-render page lease**
  rather than during `ensure_browser`. Two reasons: (1) the per-session
  context isn't materialised until `ensure_session_context` runs, and
  the context id is one of the seven cache-invalidation inputs, so
  earlier hooks would either probe with a stale key or have to re-probe
  on every session change anyway; (2) it keeps the cdp branch of
  `ensure_browser` (slice 30 + 31) untouched.
- Full fingerprint report opt-in is via env var
  (`CRAWLEX_CALIBRATION_REPORT=full`) rather than a CLI/config flag.
  No new public surface area, matches the slice-31 deliberate-no-flags
  posture, and satisfies "available only when explicitly requested".
  A future slice can promote it to config if operators want per-job
  control.
- Local origin is bound via a hand-rolled tokio TCP loopback server
  rather than pulling in axum/hyper-server. Every request is answered
  with the same minimal HTML; the probe lives in `EvaluateParams`, not
  in the served document, so the origin only exists to give the probe
  a real `http://` origin (otherwise `OfflineAudioContext`,
  `RTCPeerConnection`, `navigator.permissions` and friends behave
  differently than under `data:` / `about:blank`).
- Cache invalidation is implicit: a `CalibrationKey` change hashes
  differently and slots into a fresh entry, so there's no separate
  "invalidate" step to get wrong. The `cache_key_invalidates_on_each_
  identity_field` test asserts this for all seven fields.
- `mismatch_count` is computed host-side from the parsed fingerprint
  vs `config.locale`/`config.timezone`. Locale comparison is
  case-insensitive (browsers normalise `pt-PT` → `pt-PT` but BCP-47
  tags can disagree on case); timezone comparison is strict because
  IANA names are case-sensitive.

## Blocker for next iteration

- Same harness limitation as slices 29/30/31: `cargo`, `git`, and other
  Bash commands are rejected for approval in this worktree, so the
  changes could only be reviewed by inspection. Before moving the
  issue to `issues/done/`:
    1. `cargo check --all-targets --all-features`
    2. `cargo test --all-features` — in particular the
       `calibration::tests` module (14 tests) including the
       `local_origin_serves_calibration_html` tokio test and the
       `cache_key_invalidates_on_each_identity_field` invariant.
    3. A live external-CDP run pointed at e.g.
       `--external-cdp-url http://127.0.0.1:9222` to confirm the
       end-to-end navigate→probe→parse→cache→summary path. Set
       `CRAWLEX_CALIBRATION_REPORT=full` to also see the full report
       event.
    4. `git add` + `git commit` and `git mv issues/32-slice.md
       issues/done/32-slice.md`.
- Wave-2 follow-ups (out of scope for this slice, listed for the next
  AFK iteration to triage):
    * Surface a typed `EnforcePolicy` (today: `report-only` only) so
      mismatches can rotate identity/proxy instead of just being
      counted.
    * Plumb `CalibrationKey.seed` from a future first-class
      `--identity-seed` so cache hits survive across `bundle.canvas_
      audio_seed` regeneration.
    * Promote `CRAWLEX_CALIBRATION_REPORT` to a typed config field
      once a second consumer materialises.
