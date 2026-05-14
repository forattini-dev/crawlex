# Slice 22: `crawlex shell` Rust REPL [AFK]

## Parent

`issues/prd-v2-scraping-framework.md`

## What to build

`crawlex shell` drops the user into an interactive Rust REPL (rustyline) with crawlex helpers preloaded. Commands: `.fetch <url>`, `.css <selector>`, `.xpath <expr>`, `.findByText <text>`, `.save <identifier>` (writes adaptive fingerprint), `.open` (open the last response in a browser). State persists across commands within the session.

## Acceptance criteria

- [ ] `crawlex shell` enters an interactive prompt
- [ ] `.fetch <url>` issues a request (HTTP backend by default; `--stealth` flag uses stealth backend)
- [ ] `.css` / `.xpath` query the last response and pretty-print results
- [ ] `.findByText` and `.findByRegex` work against the last response
- [ ] `.save <identifier>` stores the currently selected element to the adaptive store
- [ ] `.open` opens the last fetched HTML in the system browser
- [ ] Readline history persists across sessions

## Blocked by

- Slice 10 (selectors must exist for shell to query them)
