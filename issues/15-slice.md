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
