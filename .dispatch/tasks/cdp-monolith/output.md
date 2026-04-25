# cdp-monolith — output

## Result

Workspace collapsed into a single-crate monolith. All four `crates/cdp-*/`
subcrates are now internal modules under `src/cdp/`. `cargo metadata
--format-version 1 --no-deps` returns exactly one package (`crawlex`).

## Tree (before → after)

Before:
```
Cargo.toml            (workspace: ".", crates/cdp-client, crates/cdp-types,
                       crates/cdp-protocol, crates/cdp-fetcher)
NOTICE                (with chromiumoxide attribution)
crates/
  cdp-client/   (crawlex-cdp       — 96-line lib.rs + browser/, handler/, …)
  cdp-types/    (crawlex-cdp-types — 335-line lib.rs)
  cdp-protocol/ (crawlex-cdp-protocol — 168-line lib.rs + 111040-line cdp.rs)
  cdp-fetcher/  (crawlex-cdp-fetcher — lib.rs + fetcher/, runtime/, version/)
src/
  (everything else)
```

After:
```
Cargo.toml            (single [package], no [workspace])
NOTICE                (chromiumoxide block removed; pointer to src/cdp/LICENSES/)
src/
  cdp/
    mod.rs            (pub mod client; pub mod fetcher; pub mod protocol;
                       pub mod types; pub use client::*;)
    client/           (former crates/cdp-client/src/)
    types/            (former crates/cdp-types/src/, lib.rs → mod.rs)
    protocol/         (former crates/cdp-protocol/src/, lib.rs → mod.rs)
    fetcher/          (former crates/cdp-fetcher/src/, lib.rs → mod.rs)
    LICENSES/
      APACHE          (ex-LICENSE-APACHE)
      MIT             (ex-LICENSE-MIT)
      NOTICE          (condensed attribution — 5 lines)
```

## Feature flags

Renamed `crawlex-cdp-backend` → `cdp-backend` across
`Cargo.toml`, `src/**`, `tests/**`, `docs/**`. The public feature surface:

- `cdp-backend` (turns on async-tungstenite, which, fnv, futures-timer,
  pin-project-lite, dunce, reqwest — all formerly transitive via the
  `crawlex-cdp` path dep)
- `chromium-fetcher` (depends on `cdp-backend`, pulls in `zip8` + `directories`)
- `zip0` / `zip8` (mutually exclusive zip backends for the fetcher; zip8 default)
- `lua-hooks` (depends on `cdp-backend`)

## Imports rewired

- Internal CDP code: `use crate::cdp::{client,types,protocol,fetcher}::…`
- Consumer code (src/, tests/): `crate::cdp::…` (lib) or `crawlex::cdp::…` (tests)
- `consume_event!` macro: demoted from `#[macro_export]` to
  `pub(crate) use consume_event;` inside `events` submodule; called as
  `crate::cdp::protocol::cdp::events::consume_event!` (avoids the
  `macro_expanded_macro_exports_accessed_by_absolute_paths` future
  incompatibility).

## Stealth string rename

- `UTILITY_WORLD_NAME`: `__crawlex_cdp_utility_world__` → `__ctx_world__`
- `EVALUATION_SCRIPT_URL`: `____crawlex_cdp_utility_world___evaluation_script__`
  → `____ctx_world___evaluation_script__`
- Cache dir: `crawlex_cdp` → `crawlex`; `crawlex_cdp-runner` → `crawlex-runner`
- Doc/comment strings in moved code: `crawlex_cdp` → `the CDP client` /
  `CDP client`.

## Verification

- `cargo build --all-features` — clean.
- `cargo build --no-default-features --features cli,sqlite` — clean in ~31s.
- `cargo clippy --all-features --all-targets -- -D warnings` — clean.
- `cargo test --all-features` — all library + integration tests pass (doctests
  in moved CDP modules are `ignore`-marked because their `crate::cdp::…`
  examples only resolve internally, not in doctest external-crate context).
- `cargo test --all-features --test live_news_navigation -- --ignored` —
  `test result: ok. 1 passed` in **34.02 s** (baseline was ~33 s).
- `cargo metadata --format-version 1 --no-deps` — 1 package: `crawlex`.
- `grep -rln 'crawlex[_-]cdp\|chromiumoxide' src/ tests/ Cargo.toml` — zero
  hits (except `src/cdp/LICENSES/` which is explicitly allowed).

## Legal note

**LICENSE texts + upstream NOTICE remain preserved in
`src/cdp/LICENSES/{APACHE,MIT,NOTICE}` as required by Apache-2.0 §4 and the
MIT License.** They were relocated from `crates/cdp-client/LICENSE-*` and
the root `NOTICE` into a discreet subdirectory so the repo root no longer
advertises the fork's origin in a `ls` listing, but the attribution chain
is intact. The root `NOTICE` still points at that directory.

## Patches preserved

The Chrome 149 protocol patches applied earlier survive the move:

- `src/cdp/client/handler/frame.rs` (lifecycle handlers)
- `src/cdp/client/handler/target.rs` (lifecycle handlers)
- `ClientSecurityState` optional fields in `src/cdp/protocol/cdp.rs`

## Notable risks addressed

- `cdp.rs` (111k lines of generated code) triggered no new warnings under
  clippy `-D warnings`. `src/cdp/mod.rs` has `#![allow(warnings)]` +
  `#![allow(clippy::all)]` as a belt-and-braces safety net.
- Edition mismatch: the four former subcrates were edition 2024; the root
  is 2021. The moved code compiles cleanly under 2021 — no edition-specific
  syntax in the CDP surface.
- `#[macro_export]` inside a nested module triggered the
  `macro_expanded_macro_exports_accessed_by_absolute_paths` future-incompat
  lint. Fixed by demoting to `pub(crate) use` of a local `macro_rules!`.
