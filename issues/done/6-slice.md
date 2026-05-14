# Slice 5: Resource-type blocking in render pool [AFK]

## Parent

#1

## What to build

Add `reject_resource_types` to the render pool so heavy assets are blocked at CDP level before the network request fires. Allowed values mirror Cloudflare: `image`, `media`, `font`, `stylesheet`. Auto-disabled when a job requests screenshots so visual fidelity is preserved.

## Acceptance criteria

- [ ] Config + CLI knob `reject_resource_types: Vec<ResourceType>` with the four canonical values
- [ ] `render::pool` configures CDP (`Fetch.enable` + matching `requestPattern`s, or `Network.setBlockedURLs`) so blocked types never hit the network
- [ ] Screenshot-bearing jobs override the setting (with a warn-level log) regardless of config
- [ ] Integration test against a synthetic page asserts blocked types are absent from the network event stream while allowed types still arrive
- [ ] Default (`[]`) preserves today's bandwidth behavior
- [ ] Documented in `docs/reference/config.md` and the render feature page

## Blocked by

None - can start immediately

## Progress (2026-05-14)

- [x] `config::RejectResourceType` enum (`image`/`media`/`font`/`stylesheet`) with
      `FromStr`, serde `rename_all = "lowercase"`, and an `url_patterns()`
      helper that returns the canonical wildcard set per category.
- [x] `Config::reject_resource_types: Vec<RejectResourceType>` field added with
      `#[serde(default)]` so existing configs deserialise unchanged.
- [x] CLI: `--reject-resource-type <value>` repeatable + comma-split shorthand;
      wired through `cli::mod` into `Config::reject_resource_types`.
- [x] `render::pool` consolidates legacy `block_resources` and typed
      `reject_resource_types` into a single `Network.setBlockedURLs` call.
      Refactored the inline match into a module-level
      `collect_block_resource_patterns` helper to keep the two paths in lock-step.
- [x] Screenshot override: when the job requests a screenshot, the typed
      `reject_resource_types` list is dropped from the block set and a
      `tracing::warn!` log records the categories that were ignored.
- [x] Unit tests:
      - `config::tests::reject_resource_type_*` cover default, `FromStr`,
        per-variant pattern non-emptiness, and serde lowercase round-trip.
      - `render::pool::reject_resource_type_wiring` asserts byte-identical
        patterns between the string and typed paths and that
        `RejectResourceType::Image` covers every required extension.
- [x] Docs: `docs/reference/config.md` Example JSON now includes
      `reject_resource_types`, a "Resource-type blocking" section explains
      the two knobs and the screenshot override, and the "Recent fields"
      table lists the new field with its default.

### Deferred

- [ ] End-to-end integration test against a synthetic page (real Chrome,
      EventLoadingFailed assertion) — the unit-level "patterns match" tests
      pin the wiring, but a real-browser test would need a fixture and is
      best landed alongside the next render-pool integration sweep.
- [ ] `cargo test`/`cargo check` not executed locally this iteration
      (sandbox blocked cargo invocations); CI will be the verification gate.

