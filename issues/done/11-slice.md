# Slice 11: Auto-selector generation [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Given an `ElementHandle`, generate a robust CSS or XPath selector string that uniquely identifies it within its tree. Prefer stable attributes (id, data-*, ARIA) and avoid brittle nth-child paths when avoidable. Exposed as `element.generateSelector({ kind: 'css' | 'xpath' })`.

## Acceptance criteria

- [ ] Generates a selector that, when re-queried on the same tree, returns the original element
- [ ] Prefers `id`, `data-testid`, ARIA roles, semantic tags over positional paths
- [ ] Falls back to nth-of-type only when no stable anchor exists
- [ ] Available in Rust core and Node SDK
- [ ] Tests cover: id-bearing elements, ARIA-anchored elements, deeply nested anonymous divs

## Blocked by

- Slice 9 (selector engine)

## Progress notes (autonomous run 2026-05-14)

Implemented in `src/parser/selectors.rs`:

- `pub enum SelectorKind { Css, Xpath }` re-exported from `parser` mod.
- `ElementHandle::generate_selector(SelectorKind) -> String`.
- CSS generator:
  - Fast paths: unique `#id` or `[data-testid="..."]` / `data-test-id`
    / `data-test` / `data-qa` / `data-cy` if it disambiguates from root.
  - Otherwise walks ancestors bottom-up, emitting a segment per level
    with anchor precedence id → stable data-* → aria-label → role →
    `:nth-of-type(N)`. Stops walking once
    `root.select(chain).count() == 1 && hit == target`.
  - Ids with non-ident characters fall back to `[id="..."]`.
- XPath generator:
  - Fast paths: `//*[@id='x']` or `//*[@data-testid='x']` when unique.
  - Otherwise absolute path `/seg1/seg2/...` so `[N]` predicates
    evaluate against single-context lists at each step (the relative
    `//tag[N]` form flattens across siblings — slice 9 progress note).
- Unit tests cover: id-anchored elements, data-testid, aria-label,
  deeply-nested anonymous divs (round-trip via `:nth-of-type` fallback),
  ambiguous classes (positional path), id with special chars
  (attribute-form fallback), and round-trip for both CSS + XPath.

SDK type stub added to `sdk/index.d.ts`: `SelectorKind`, `ElementHandle`
with `generateSelector({ kind })`. Runtime binding deferred until the
rust parser surface lands on a release (same pattern as slices 8 + 9).

### Verification status

`cargo check` / `cargo test parser::selectors --all-features` blocked
by the autonomous-run sandbox (cargo not on the approved command list).
Next operator: run

    cargo check --all-targets --all-features
    cargo test parser::selectors --all-features

and address any compile drift before promoting the issue further.

### Commit blocker

`git add` / `git commit` / `git mv` were all denied by the sandbox in
this run. Files are written but uncommitted on `ralph/slice-11`:

- src/parser/selectors.rs    (new SelectorKind + generate_selector + tests)
- src/parser/mod.rs           (re-export SelectorKind)
- sdk/index.d.ts              (forward-declared ElementHandle / SelectorKind)
- issues/11-slice.md          (this progress note — should move to issues/done/)

Operator: stage the four files, commit, then `git mv issues/11-slice.md
issues/done/11-slice.md` and commit that as a follow-up (or amend) before
running `scripts/merge-ralph-worktree.sh`.
