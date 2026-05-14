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

## Progress notes (autonomous run 2026-05-14)

- Implemented `src/parser/selectors.rs` with:
  - `ElementHandle` exposing `name`, `attr`, `text`, `html`, `inner_html`,
    `parent`, `siblings`, `children`, plus per-handle `css` / `xpath`.
  - `TreeHandle::css(&str) -> Vec<ElementHandle>` and
    `TreeHandle::xpath(&str) -> Vec<ElementHandle>`.
  - Scrapy/Parsel pseudo-element parity via
    `TreeHandle::css_get` / `css_get_all` honoring `::text` and
    `::attr(name)`.
  - Hand-rolled XPath subset (axes `self`, `child`, `descendant`,
    `descendant-or-self`, `parent`, `ancestor`, `ancestor-or-self`,
    `following-sibling`, `preceding-sibling`; `*` and name tests;
    predicates `[@attr]`, `[@attr='val']`, `[N]`) plus
    `xpath_get_all` for terminal `@attr` / `text()`.
- Added `ego-tree = "0.11"` as a direct dep (was transitive via scraper)
  so the selector engine can hold `NodeRef`s during traversal.
- Unit tests in `parser::selectors::tests` cover Scrapy-parity CSS
  selectors, pseudo-element extraction, navigation, descendant queries,
  predicates, axis traversal (`ancestor::`, `following-sibling::`),
  terminal `@attr` and `text()`, and invalid-selector resilience.

### Outstanding for follow-ups

- Node SDK binding (`sdk/`): deferred until the Rust surface lands on a
  release — same pattern as slice 8. Mirror `css`, `xpath`, navigation
  on the handle, plus `css_get` / `css_get_all` and `xpath_get_all`.
- XPath predicate semantics: `[N]` currently applies across the
  flattened result list rather than per-context. Good enough for
  single-context queries (covered in tests); revisit when nested
  predicates land.
- Predicates do not yet support boolean combinators (`and`/`or`) or
  `text()` / `position()` functions inside `[]`.

### Verification status

Code authored but `cargo check` / `cargo test --all-features` were
blocked by the autonomous-run sandbox (cargo not on the approved
command list). Next operator: run

    cargo check --all-targets --all-features
    cargo test parser::selectors --all-features

and address any compile drift before moving this issue to
`issues/done/`.
