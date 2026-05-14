# Slice 15: findSimilar via adaptive engine [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Add `element.findSimilar(opts?)` that builds a fingerprint from the given element and returns other elements in the same tree whose score exceeds the threshold. Useful for "anchor one row, get all rows" patterns. Reuses the similarity engine from slice 12 without touching the adaptive store.

## Acceptance criteria

- [ ] `ElementHandle::findSimilar(threshold?) -> Vec<ElementHandle>` in Rust
- [ ] Node SDK exposes `.findSimilar()` on element handles
- [ ] Default threshold is configurable, defaults to 0.2
- [ ] Does NOT touch the adaptive store (pure in-tree scan)
- [ ] Tests: table-row fixture returns all sibling rows; unrelated decorative elements excluded

## Blocked by

- Slice 14 (relocation logic and threshold plumbing)

## Progress note (2026-05-14)

Implementation written but NOT committed — bash perms blocked all
`cargo`/`git` commands in this loop, so feedback loops did not run.

What landed in the working tree (unstaged):

- `src/parser/adaptive.rs`: `ElementHandle::find_similar(threshold: Option<f32>) -> Vec<ElementHandle>`
  - builds fingerprint from anchor
  - walks topmost ancestor's descendants
  - excludes anchor by NodeId
  - filters by `threshold` (default `DEFAULT_THRESHOLD` = 0.2)
  - sorts hits by descending score
  - 6 new unit tests covering: table-row recall, anchor-exclusion,
    high-threshold filtering, descending-score order, no-store contract,
    empty-result.
- `sdk/index.d.ts`: declared `ElementHandle.findSimilar(opts?: { threshold?: number }): ElementHandle[]`.

Next iteration must:

1. Run `cargo test --all-features` + `cargo check --all-targets --all-features`
   to confirm the new code compiles and tests pass. Most likely-fail point:
   `let mut top = *self.inner();` deref of `ElementRef` to `NodeRef` —
   pattern already used in `relocate()` so should work.
2. Stage `src/parser/adaptive.rs` + `sdk/index.d.ts`, commit, and
   `git mv issues/15-slice.md issues/done/15-slice.md`.
