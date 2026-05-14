# Slice 12: Element fingerprint + similarity scoring (pure) [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Pure-function `similarity` module. Computes a stable fingerprint for an element (tag, attribute subset split by id/class/href/other, text hash, parent chain, sibling position) and scores two fingerprints into a 0..1 number. Algorithm is a 1:1 port of Scrapling's weighted formula — verified against fixture pairs from Scrapling's own test suite where possible. No persistence in this slice.

## Acceptance criteria

- [ ] `fingerprint(element) -> Fingerprint` (pure, no I/O)
- [ ] `score(&Fingerprint, &Fingerprint) -> f32` in 0..1
- [ ] Tag mismatch caps score; same-tag pairs differentiated by attributes/text/position
- [ ] Property tests confirm `score(a, a) == 1.0` and `score(a, b) == score(b, a)`
- [ ] Recall test on at least 10 real before/after DOM fixture pairs (e.g. e-commerce price labels across redesigns) at threshold 0.2
- [ ] No dependency on storage; pure module

## Blocked by

- Slice 8 (parser foundation provides the element type)

## Status note (2026-05-14)

Implementation drafted in `src/parser/similarity.rs` and registered in
`src/parser/mod.rs`. Includes:

- `Fingerprint` struct (tag, id, classes, href, other_attrs, text_hash,
  text_tokens, parent_chain, sibling_index).
- `fingerprint(&ElementHandle) -> Fingerprint` pure ctor.
- `score(&Fingerprint, &Fingerprint) -> f32` with weighted features
  summing to 1.0 and `TAG_MISMATCH_CAP = 0.15` enforcing tag-mismatch cap.
- Unit + property tests: identity, symmetry, tag-mismatch cap,
  sibling-vs-dissimilar ranking, 100-seed random property loop.
- 10-pair recall test at threshold 0.2 (pair 2 is intentionally
  cross-tag to confirm the cap path).

**Blocker for this iteration**: `cargo` and `git` shell commands are
denied in the current sandbox, so `cargo test --all-features` and the
commit step did not execute. Next Ralph iteration should run the
feedback loops and land the commit if green. Module is self-contained
(only `src/parser/mod.rs` was touched outside the new file).
