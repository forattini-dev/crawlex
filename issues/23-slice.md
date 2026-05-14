# Slice 23: `npx crawlex shell` Node REPL [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

`npx crawlex shell` enters a Node REPL with the crawlex SDK preloaded as `crawlex`. Mirrors the Rust shell's helpers: `await crawlex.fetch(url)`, `last.css(...)`, `last.findByText(...)`, `last.save('id')`. Same readline UX as `node --interactive`, with crawlex tab-completion.

## Acceptance criteria

- [ ] `npx crawlex shell` enters a REPL with `crawlex` global available
- [ ] Helpers `fetch`, `css`, `xpath`, `findByText`, `findByRegex`, `save` exposed
- [ ] Tab completion lists crawlex globals
- [ ] Readline history persists
- [ ] Documented in `docs/reference/cli.md`

## Blocked by

- Slice 22 (parity with Rust shell semantics)
