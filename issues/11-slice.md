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
