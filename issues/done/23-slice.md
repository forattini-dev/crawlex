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

## Implementation notes (2026-05-14)

Landed `npx crawlex shell` as a JS-side intercept in `runCli`:

- `sdk/shell.js` — `createShellApi` (test seam) + `startRepl` (REPL
  wiring with `node:repl`). Helpers: `crawlex.fetch(url)`, `css`,
  `xpath`, `findByText`, `findByRegex`, `save`. `crawlex.last` and the
  REPL global `last` both point at the most recently fetched `Page`.
- `sdk/crawlex-sdk.js` — `runCli` detects bare `shell` arg and calls
  `require('./shell.js').startRepl()` instead of passing through to the
  native binary. Everything else still passes through unchanged.
- `package.json` — `sdk/shell.js` added to the `files` allow-list so it
  ships in the npm tarball.
- `docs/reference/cli.md` — new "Interactive shells" section documents
  both the Rust and Node shells and the subset semantics.

Selector engine is intentionally minimal (regex-based subset: `tag`,
`#id`, `.class`, `tag#id`, `tag.class`, `//tag`). Anything richer
raises an error pointing back at the Rust shell. `findByText` /
`findByRegex` walk every nested element via a depth-tracking
`elementIter` so matches inside nested same-tag containers (e.g.
`<div>` inside `<div>`) still surface. `save` writes JSON to
`$XDG_DATA_HOME/crawlex/adaptive_store.json` keyed by host +
identifier.

Tests in `sdk/test/shell.test.js` (`node:test`) cover css/xpath/find
helpers, lastSelection updates, save happy path + error paths, helper
guards before any fetch, and a round-trip that confirms the adaptive
store JSON is preserved across sessions. They drive `createShellApi`
with a stubbed fetcher so no network is required.

**Blocker for next iteration:** the AFK harness denied every
`node`/`pnpm`/`git` invocation in this session, so the changes are on
disk but unverified by `pnpm test` and uncommitted. First action next
iteration: run `pnpm test` to land the suite, `git mv
issues/23-slice.md issues/done/23-slice.md`, and commit.
