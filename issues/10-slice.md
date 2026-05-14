# Slice 10: Selector helpers — findByText, findByRegex, filter [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Add the content-driven query helpers to the selector engine: `findByText(text, opts)`, `findByRegex(pattern)`, and `filter(predicate)`. These let users target elements when attributes are unstable. Available on both the tree handle and any `ElementHandle`-collection.

## Acceptance criteria

- [ ] `findByText` supports exact, contains, case-insensitive, and trim options
- [ ] `findByRegex` accepts a compiled regex and matches against element text
- [ ] `filter` accepts a predicate closure (Rust) / function (Node) and returns a filtered collection
- [ ] Helpers compose with `.css()`/`.xpath()` results
- [ ] Tests cover text matching across nested elements, unicode, and regex captures

## Blocked by

- Slice 9 (selector engine)
