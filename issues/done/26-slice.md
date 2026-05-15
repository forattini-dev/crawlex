# Slice 26: Feature parity matrix doc vs Scrapling [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

Publish a feature parity matrix at `docs/comparisons/scrapling.md` listing every Scrapling claim and crawlex's status (parity, superior, intentionally absent). AFK note: was originally HITL because it's a public marketing artifact — final wording requires human approval and we want to ensure no overclaim.

## Acceptance criteria

- [ ] `docs/comparisons/scrapling.md` published, linked from README
- [ ] Matrix covers: spider DSL, fetchers (HTTP/dynamic/stealth), session mgmt, proxy rotation, ad-block, DoH, adaptive parser, selectors, MCP, shell, dev-replay, robots.txt
- [ ] Each row cites a crawlex slice or existing module as evidence
- [ ] "Intentionally absent" rows (Python SDK, ML embeddings, distributed) call out the reason
- [ ] No public benchmark numbers (per PRD out-of-scope)
- [ ] Reviewed by human before publish

## Blocked by

- Slice 25 (v1 removal must land so claims reflect shipped surface)
