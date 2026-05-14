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

