# Slice 9: Selector engine — CSS + XPath + navigation [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Build the `selectors` module on top of the parser tree. Expose CSS and XPath query methods plus navigation accessors (`parent`, `siblings`, `children`) on the element handle. CSS pseudo-element semantics match Scrapy/Parsel so users porting selectors don't rewrite. Node SDK mirrors the API.

## Acceptance criteria

- [ ] `TreeHandle::css(&str) -> Vec<ElementHandle>` and `xpath(&str) -> Vec<ElementHandle>` in Rust
- [ ] `ElementHandle` exposes `parent()`, `siblings()`, `children()`, attribute access, text content
- [ ] CSS pseudo-elements `::text` and `::attr(name)` supported per Scrapy/Parsel semantics
- [ ] Node SDK exposes `.css()`, `.xpath()`, `.parent`, `.siblings`, `.children` on the handle
- [ ] Tests cover Scrapy parity selectors against fixture pages
- [ ] XPath axis support (`ancestor::`, `following-sibling::`) verified by tests

## Blocked by

- Slice 8 (parser foundation)
