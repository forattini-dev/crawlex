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
