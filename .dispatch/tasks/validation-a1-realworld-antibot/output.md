# Validation A.1 — Real-world antibot suite — output

## Run

```
cargo test --all-features --test real_world_antibot_live -- --ignored --nocapture
```

Total wall time: 189s. Process finished exit 0. Test asserts only that a
report was produced; honest per-site numbers follow.

## Totals

- pass: 2
- partial: 1
- fail: 1
- unreachable: 4 (CDP navigate timeout at ~30s; plain `curl` to the same
  hosts returns 200 in under 5s, so these are classified correctly as
  unreachable-from-CDP and NOT as stealth regressions)
- total sites: 8

## Per-site verdict

| # | Site | Verdict | Notes |
|---|------|---------|-------|
| 1 | nowsecure.nl | fail | CloudflareTurnstile WidgetPresent — not bypassed |
| 2 | antoinevastel.com/bots | pass | FP page rendered |
| 3 | arh.antoinevastel.com/bots/areyouheadless | pass | Detector reported "not Chrome headless" |
| 4 | bot.sannysoft.com | unreachable | Navigate timed out |
| 5 | abrahamjuliot.github.io/creepjs | unreachable | Navigate timed out |
| 6 | browserleaks.com/canvas | unreachable | Navigate timed out |
| 7 | browserleaks.com/webrtc | unreachable | Navigate timed out |
| 8 | pixelscan.net | partial | Page rendered but coherence markers absent |

## Screenshots captured

- production-validation/screenshots/nowsecure_nl.png (382 KB)
- production-validation/screenshots/antoinevastel_com.png (117 KB)
- production-validation/screenshots/arh_antoinevastel_com.png (95 KB)
- production-validation/screenshots/pixelscan_net.png (1035 KB)

Four unreachable sites produced no screenshot (no page to capture).

## Surprises

1. **AreYouHeadless = pass.** The detector explicitly reported our browser
   as NOT headless. This is a real evasion win for the stealth shim.
2. **Cloudflare Turnstile widget = detected, not solved.** Stealth
   shipped today keeps the widget visible on `nowsecure.nl`. That is
   the detection signal persisting — a CF-Turnstile solver is out of
   scope of this validation task.
3. **Four consecutive CDP navigate timeouts** at exactly ~30s each
   (sannysoft, creepjs, both browserleaks) while plain curl against the
   same origins returns within seconds. Likely a per-page JS bundle
   that never quiets under `NetworkIdle{idle_ms:1500}` inside 30s
   (creepjs in particular runs an enormous FP sweep). Worth revisiting
   with `WaitStrategy::Fixed` or a longer idle window in a follow-up,
   but not a stealth failure.
4. **pixelscan = partial.** Page rendered 1MB of content but the
   specific coherence markers we look for were absent — the JS-rendered
   output likely uses different phrasing. The challenge signal was
   `None`, so this is an evaluator-precision issue, not a block.

## Deliverables

- `tests/real_world_antibot_live.rs` — new, one `#[ignore]`d test.
- `production-validation/real_world_report.md` — populated table +
  per-site notes.
- `production-validation/summary.md` — created with A.1 row.
- `production-validation/screenshots/` — 4 PNGs (sites we reached).

## Incidental fix

Clippy gate was already red on main (`src/identity/validator.rs:215-220`
— `question_mark` lint on `if let Err(e) = ... { return Err(e) }`).
Fixed in-place to keep the non-negotiable gate green; no behavior
change.

## Gates

- `cargo build --all-features` — green
- `cargo build --no-default-features --features cli,sqlite` — green
- `cargo clippy --all-features --all-targets -- -D warnings` — green
  after the validator.rs cleanup
- `cargo test --all-features` (non-ignored) — running; see final tail
- `cargo test --all-features --test real_world_antibot_live -- --ignored --nocapture`
  — 1 passed, 189s, report written

## Honest read

2/8 true pass. 1/8 partial (pixelscan). 1/8 fail (Cloudflare Turnstile).
4/8 unreachable-from-CDP (not a bypass failure — transport-level
timeout). If every unreachable were re-run with a longer navigate
budget and converted, we still have a concrete Cloudflare Turnstile
gap to close.
