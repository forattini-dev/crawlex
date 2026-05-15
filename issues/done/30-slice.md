# Slice 30: External CDP render path [AFK]

Status: needs-triage
Labels: needs-triage

## Parent

`issues/prd-native-stealth-browser-providers.md`

## What to build

Connect crawlex to a user-provided CDP endpoint and render a normal target through that external browser. The slice should create a crawlex-owned page or context when possible, preserve session isolation by default, and return explicit errors for unreachable or incompatible endpoints.

## Acceptance criteria

- [ ] `cdp` provider mode connects to a configured external CDP endpoint
- [ ] A simple HTML target can be rendered through the external endpoint end-to-end
- [ ] crawlex creates and cleans up its own page or context where the endpoint supports it
- [ ] Session isolation remains the default for external CDP usage
- [ ] Unreachable, invalid, or incompatible CDP endpoints produce actionable errors before target work continues
- [ ] Provider-selected events distinguish external CDP from stock Chromium
- [ ] Unit/integration coverage exercises a generic local CDP endpoint when available

## Blocked by

- `issues/29-slice.md`

## Work done (iteration 1)

Added an explicit CDP preflight that probes the external endpoint's
`/json/version` before any target work begins, so unreachable /
incompatible endpoints fail fast with an operator-readable message
instead of surfacing as a generic WebSocket transport error on the
first job.

Files changed:

- `src/render/cdp_probe.rs` (new) — `probe(endpoint)` async function.
  Accepts `http(s)://` and `ws(s)://` (rewrites to http(s) for the
  version lookup), strips any path so a session-specific debugger URL
  still hits `/json/version`, applies a 5 s timeout, and distinguishes:
    * empty url → "external CDP url is empty"
    * unknown scheme → "expected scheme http/https/ws/wss"
    * connect failure → "external CDP endpoint unreachable at <url>:
      <io err> — verify the browser is running and accepts remote CDP
      connections"
    * non-2xx → "external CDP endpoint at <url> returned HTTP <code>;
      … check the endpoint URL and that DevTools is exposed on this
      host"
    * non-JSON / missing `webSocketDebuggerUrl` → "endpoint looks
      incompatible with the Chromium DevTools Protocol"
  In-file unit tests cover the URL builder and the cheap probe paths
  (empty, unreachable port 1).
- `src/render/mod.rs` — `pub mod cdp_probe` (gated on `cdp-backend`,
  since reqwest is only a dep under that feature).
- `src/render/pool.rs` — `RenderPool::preflight()` now calls
  `cdp_probe::probe` whenever `external_cdp_url` is configured. On
  success the existing `event="provider.selected"` log entry is
  enriched with the resolved `ws_debugger_url` and the `Browser`
  identity string from the version payload. On failure preflight
  returns `Error::Render("external CDP preflight failed: <msg>")`.
- `src/crawler.rs` — preflight failure remains a warning for the
  stock-Chrome path, but is now **fatal** when `external_cdp_url` is
  set: the crawler returns the preflight error before any render
  worker spins up. Matches the acceptance criterion ("…produce
  actionable errors before target work continues").
- `tests/external_cdp_preflight.rs` (new) — wiremock-backed integration
  coverage of the probe contract: compatible host, ws-scheme rewriting,
  HTTP 500, missing `webSocketDebuggerUrl`, non-JSON body, unreachable
  port, invalid scheme.

Key decisions:

- Used a probe instead of a full WebSocket connect: cheap, deterministic,
  needs no extra dependencies (`reqwest` is already pulled in by the
  `cdp-backend` feature), and produces the most operator-readable
  error messages. The real session connect still goes through
  `Browser::connect_with_config` exactly as before — the probe only
  short-circuits unreachable / non-CDP hosts.
- Kept the "rendered through external endpoint end-to-end" criterion
  satisfied by the **existing** `ensure_browser` path (`Browser::
  connect_with_config` + `SetAutoAttachParams` + child-shim watcher),
  which already creates crawlex-owned pages/contexts via
  `CreateBrowserContextParams` / `CreateTargetParams` under the same
  `external_cdp_url` branch. No new code was needed there for this
  slice — session isolation (separate `BrowserContext` per session)
  is inherited from the stock path.
- Provider-selected event already discriminated `stock` vs `cdp`
  (slice 29); the new `ws_debugger_url` + `browser` fields make the
  emitted log line traceable to a specific external host without
  parsing free text.

## Blocker for next iteration

- Same harness limitation as slice 29: `cargo`, `git`, and other Bash
  commands are rejected for approval in this worktree, so the changes
  could only be reviewed by inspection. Before moving the issue to
  `issues/done/`:
    1. `cargo check --all-targets --all-features`
    2. `cargo test --all-features` — in particular
       `external_cdp_preflight` (new file) and the `cdp_probe::tests`
       module.
    3. A live-Chrome run pointed at `--external-cdp-url
       http://127.0.0.1:9222` to confirm the end-to-end render path,
       which is out of scope for the in-process test fixture.
    4. `git add` + `git commit` and `git mv issues/30-slice.md
       issues/done/30-slice.md`.
