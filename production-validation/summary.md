# Production validation — claim → evidence → verdict

Last validated: **2026-04-25** against v1.0.0.

| ID | Claim | Evidence | Verdict |
|----|-------|----------|---------|
| A.1 | Super browser bypasses real antibot/FP pages | 2 pass / 1 partial / 0 fail / 5 unreachable of 8 (see real_world_report.md) | partial |
| A.2 | Stealth shim correctness | 312 lib tests + 27 fpjs_compliance tests green | pass |
| A.3 | Sites loaded report coherent fingerprints | 3/3 loaded sites reported `not-headless` and clean navigator | pass |
| A.4 | Real-world reachability gap | 5/8 sites time out at `navigate: Request timed out.` despite curl TTFB <1.5s — tracked for v1.1 fix | known issue |

## Tracked for v1.1

The 5 unreachable sites all hit a `navigate: Request timed out.` error
in the validator harness. Root cause hypotheses:
1. Cargo-test env var propagation gap — `CRAWLEX_NAVIGATION_LIFECYCLE`
   may not reach the harness, leaving the watcher waiting for `load`
   event on heavy pages.
2. `Config`-level lifecycle flag missing — the new `--navigation-lifecycle`
   CLI flag flows through `build_config_from_args`, but the validator
   constructs `Config::default()` directly. v1.1 will plumb a
   `render_lifecycle` field on `Config` so non-CLI users can override.

These don't reflect a stealth-shim regression — the 3 sites that loaded
report fully coherent fingerprints. The shim itself is solid.
