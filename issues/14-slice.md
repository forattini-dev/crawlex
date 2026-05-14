# Slice 14: Adaptive relocation in selector calls [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Extend the selector API to accept an `identifier` parameter. On first run, the matched element's fingerprint is saved to the adaptive store. On subsequent runs, if the selector returns nothing (or returns elements scoring below threshold against the stored fingerprint), walk the DOM, score candidates, and return the highest-scoring match above the configured threshold. Confidence score is exposed on the returned handle for auditing.

## Acceptance criteria

- [ ] `.css(selector, { identifier, threshold? })` and `.xpath(...)` accept the new params
- [ ] First-run with new identifier saves the fingerprint
- [ ] Selector failure on subsequent run triggers DOM walk + scoring
- [ ] Returned element exposes `.adaptiveConfidence` when relocated
- [ ] Threshold defaults to 0.2 (matches Scrapling); per-call override accepted
- [ ] Integration test: train on fixture A, query on mutated fixture B, assert correct relocation
- [ ] Logs adaptive match events at info level

## Blocked by

- Slice 13 (adaptive store), Slice 10 (selector helpers)
