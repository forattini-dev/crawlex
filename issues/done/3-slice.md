# Slice 2: render_mode flag (auto|always|never) [AFK]

## Parent

#1

## What to build

Promote the existing `fallback_fetch` path to a first-class `render_mode` knob with three values: `auto` (default; today's behavior — impersonate first, fall back to render), `always` (skip impersonation, always render), `never` (skip render, fail-or-degrade if the page needs JS). Surface identically in config TOML, CLI flag, and the Rust + TS SDKs.

## Acceptance criteria

- [ ] `Config.render_mode: RenderMode { Auto, Always, Never }` with `Auto` as default
- [ ] CLI flag `--render-mode <auto|always|never>` wired through `cli/args.rs`
- [ ] Crawler chooses path strictly per `render_mode`; `Never` does not instantiate the render pool
- [ ] Each fetched URL emits an event tagged with which path served it (`impersonate` | `render`)
- [ ] Integration test runs all three modes against the existing `tests/mini_http_only.rs` fixture and asserts the path tag from events
- [ ] `Always` continues to work with proxy rotation
- [ ] Documented in `docs/reference/cli.md`, `docs/reference/config.md`, and the relevant feature page
- [ ] Existing recipes (no `render_mode` set) behave identically to today

## Blocked by

None - can start immediately

## Progress notes (ralph 2026-05-14)

Implemented `RenderMode { Auto, Always, Never }` (default `Auto`) on
`Config`, exposed as `--render-mode` on the crawl CLI, enforced in
`cli::run_crawl` so `Always` bumps `max_concurrent_render` to ≥1 and
forces seed `FetchMethod::Render`, while `Never` slams the pool to
zero and pins seeds to `FetchMethod::HttpSpoof`. The existing
`Decision::Render` arm already short-circuits to "stay on http" when
`max_concurrent_render == 0`, which gives `Never` policy-escalation
refusal for free.

Tagged `FetchCompletedData.path` (`"impersonate"` for HTTP spoof,
`"fallback"` for the external `fallback_fetch` command) and added
`path: "render"` to the `render.completed` json. SDK `index.d.ts`
mirrors the new fields. Integration tests at `tests/render_mode.rs`
exercise `Auto` and `Never` end-to-end against the wiremock fixture
and assert the event tags; `Always` is exercised at the
config-resolution layer because the fixture has no Chrome.

Docs updated: `docs/reference/cli.md`, `docs/reference/config.md`
(JSON example + table row), `docs/reference/events.md`.

**Blocker for next iteration**: `cargo` was not authorised in this
sandbox so `cargo check --all-targets --all-features` and
`cargo test --all-features` could not be run. Re-run them before
moving this slice to `issues/done/`.

