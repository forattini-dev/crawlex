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

## Progress notes (autonomous run 2026-05-14)

Implemented in `src/parser/selectors.rs` + re-exported from
`src/parser/mod.rs`:

- `TextMatch` struct with `exact`, `case_insensitive`, `trim` flags plus
  builder helpers (`TextMatch::contains()`, `TextMatch::exact()`,
  `.with_case_insensitive(v)`, `.with_trim(v)`).
- `ElementHandle::find_by_text(needle, opts)` — walks descendants,
  filters by element text vs needle per `opts`. Empty needle returns
  empty.
- `ElementHandle::find_by_regex(&Regex)` — walks descendants, returns
  elements whose concatenated text `Regex::is_match`es. Surface is the
  standard `regex::Regex` (caller does captures themselves).
- `TreeHandle::find_by_text` / `find_by_regex` delegating from the
  document root.
- `HandleSliceExt::filter` extension trait impl on `[ElementHandle]` —
  Vec<ElementHandle> auto-derefs to slice, so users write
  `tree.css("li").filter(|h| ...)` after importing the trait. Method
  name `filter` doesn't clash because Iterator::filter doesn't apply
  to a Vec directly.

Tests added (in `parser::selectors::tests`):

1. contains (default)
2. exact + trim
3. case-insensitive
4. unicode (日本 / 日本語)
5. nested text concatenation (`$<b>42</b>` → matches `"$42"`)
6. regex anchored alternation
7. regex captures (`SKU-(\d+)`)
8. filter composes with `.css(...)`
9. filter composes with `.xpath(...)`
10. find_by_text scoped to an element (not whole tree)
11. empty needle returns empty

### Outstanding

- Node SDK binding (`sdk/`): deferred, same cadence as slices 8 and 9.
  Expose `findByText({exact, caseInsensitive, trim})`, `findByRegex`,
  and `filter` on the JS tree/element handle.
- `find_by_text` walks `descendants` — for very large trees the
  allocation could be lazy via a custom iterator. Not a v1 concern.

### Verification status

`cargo check` / `cargo test parser::selectors --all-features` were
blocked by the autonomous-run Bash sandbox (same situation as slice
9 — `cargo` and `git` invocations need operator approval). Code is on
disk, uncommitted. Next operator:

    cargo check --all-targets --all-features
    cargo test parser::selectors --all-features

then `git add -- src/parser/selectors.rs src/parser/mod.rs` and commit,
then move this file to `issues/done/`.
