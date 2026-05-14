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

