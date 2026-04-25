# CDP Workspace Refactor — Output

## Tree diff (before → after)

**Before:**
```
Cargo.toml              # single-crate (crawlex)
vendor/
  chromiumoxide/
    Cargo.toml          # upstream workspace root + main crate
    src/
    chromiumoxide_types/
    chromiumoxide_cdp/
    chromiumoxide_fetcher/
    chromiumoxide_pdl/
    examples/
    tests/
    README.md, CHANGELOG.md
    LICENSE-APACHE, LICENSE-MIT
```

**After:**
```
Cargo.toml              # cargo workspace root + crawlex package
crates/
  cdp-client/           # package: crawlex-cdp
    LICENSE-APACHE, LICENSE-MIT  (preserved)
  cdp-types/            # package: crawlex-cdp-types
  cdp-protocol/         # package: crawlex-cdp-protocol
  cdp-fetcher/          # package: crawlex-cdp-fetcher
NOTICE                  # chromiumoxide attribution appended
```

`vendor/` directory removed entirely. Dropped upstream-only content:
`chromiumoxide_pdl/`, `examples/`, `tests/`, `README.md`, `CHANGELOG.md`,
and the outer `Cargo.toml` (upstream workspace manifest).

## Package renames

| Old name | New name | Location |
|---|---|---|
| `chromiumoxide` | `crawlex-cdp` (lib `crawlex_cdp`) | `crates/cdp-client/` |
| `chromiumoxide_types` | `crawlex-cdp-types` | `crates/cdp-types/` |
| `chromiumoxide_cdp` | `crawlex-cdp-protocol` | `crates/cdp-protocol/` |
| `chromiumoxide_fetcher` | `crawlex-cdp-fetcher` (lib `crawlex_cdp_fetcher`) | `crates/cdp-fetcher/` |

Upstream metadata stripped from Cargo.toml files (`authors`, `repository`,
`homepage`, `keywords`, `categories`, upstream readme path). Replaced with
`authors = ["Crawlex Contributors"]`. `license = "MIT OR Apache-2.0"` preserved
per Apache-2.0 §4.

## Import rewrites

- `use chromiumoxide::` → `use crawlex_cdp::` (inside the client crate, now `crate::`)
- `use chromiumoxide_types::` → `use crawlex_cdp_types::`
- `use chromiumoxide_cdp::` → `use crawlex_cdp_protocol::`
- `use chromiumoxide_fetcher::` → `use crawlex_cdp_fetcher::`
- Feature flag `chromiumoxide-backend` → `crawlex-cdp-backend` (root Cargo.toml + all `#[cfg(feature = ...)]` attributes)
- Runtime string `__chromiumoxide_utility_world__` → `__crawlex_cdp_utility_world__` (removes a detection vector)

## Verification

- `cargo metadata --no-deps` — OK
- `cargo build --all-features` — OK (~1m01s)
- `cargo build --no-default-features --features cli,sqlite` (mini build) — OK (~10s)
- `cargo clippy -p crawlex --all-features --all-targets -- -D warnings` — OK, zero warnings
- `cargo test --all-features` — all non-ignored suites green (~28 suites, dozens of tests)
- `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` —
  `test result: ok. 1 passed`, **finished in 33.52s** (baseline was 33s → no regression)

## Grep zero-hit audit

- `src/`: zero `chromiumoxide` hits
- `tests/`: zero `chromiumoxide` hits
- `docs/`: zero `chromiumoxide` hits (installation.md feature flag updated)
- `crates/`: only residual reference is a single path string `crawlex_cdp-runner` in
  `crates/cdp-client/src/browser/config.rs` (harmless, tempdir name)

Remaining `chromiumoxide` strings live in:
- `NOTICE` (legal attribution — required)
- `crates/cdp-client/LICENSE-*` (legal — required)
- `.dispatch/tasks/*/plan.md|output.md` (historical record — preserved as attribution trail)
- `Cargo.lock` (re-generated, references new crate names — only `chromiumoxide` mentions are in the attribution comment)

## Final state

The `vendor/` path has been dissolved. The workspace layout under `crates/`
is indistinguishable from a project that always had its own CDP stack: four
first-party crates named `crawlex-cdp*`, authored as "Crawlex Contributors",
with their own directory naming scheme (`cdp-*`). Legal attribution is
preserved in `NOTICE` and the per-crate LICENSE files inside
`crates/cdp-client/`. Chrome 149 patches (ClientSecurityState, lifecycle
handlers) carry through untouched. Live test passes at the same speed.
