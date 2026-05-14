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

## Implementation notes (2026-05-14)

Landed `crawlex shell` as `Command::Shell(ShellArgs)`:

- `src/shell/mod.rs` — `ShellState`, `Fetcher` trait, `ImpersonateFetcher`,
  `dispatch()` (unit-test seam), `run_interactive()` (REPL loop). Verbs:
  `.fetch`, `.css`, `.xpath`, `.findByText`, `.findByRegex`, `.save`,
  `.open`, `.help`, `.exit`. Selection is auto-captured from the first
  match returned by any selector verb; `.save <id>` writes that
  fingerprint into `AdaptiveStore` keyed by URL host.
- `src/cli/args.rs` + `src/cli/mod.rs` — new subcommand + async
  dispatcher (`cmd_shell`).
- `src/lib.rs` — `pub mod shell;` under the `cli` feature.

History persistence is an append-only text file (one line per input) at
`$XDG_DATA_HOME/crawlex/shell_history` by default; arrow-key recall
would need `rustyline`, which we could not vendor in this sandbox.
Acceptance criterion only requires persistence.

Tests in `src/shell/mod.rs` (`#[cfg(test)]`) cover fetch state mutation,
each selector verb, `.save` happy + error paths, unknown commands, help,
and history round-trip. They drive `dispatch()` with a `StubFetcher`
holding canned HTML so no network is required.

**Blocker for next iteration:** the AFK harness denied every
`cargo`/`git` invocation in this session, so the changes are on disk but
unverified by `cargo check` / `cargo test --all-features` and
uncommitted. First action next iteration: run the feedback loops and
land the commit.
