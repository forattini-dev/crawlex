# P0 Motion Engine — output

## Summary

Replaced the simple bezier mouse path + fixed-delay typing with a full behavioural model grounded in `research/evasion-deep-dive.md` §9:

- **WindMouse** (Benjamin Land 2021) with gravity/wind, per-step Ornstein-Uhlenbeck jitter, and probabilistic overshoot. Bell-curve velocity profile, not linear.
- **Fitts' law** (`MT = a + b·log₂(D/W+1)`) drives trajectory duration.
- **Event-sequence integrity**: `click_selector` now emits `mousemove* → mouseover → post-move pause → mouseDown → hold → mouseUp`. A click without a preceding move is impossible by construction.
- **Keystroke model**: log-normal hold (μ≈ln(90ms), σ=0.3), log-logistic inter-key flight (α=70ms, β=3.5), Pareto thinking pauses (scale=500ms, α=1.5), probabilistic typos with corrective Backspace + neighbour-key QWERTY model.
- **Motion profiles**: `fast` / `balanced` (default) / `human` / `paranoid`. Selected via `Config::motion_profile` and CLI `--motion-profile`, installed at startup into a process-wide atomic (`MotionProfile::set_active()`).

## Files touched

New:
- `/home/cyber/Work/FF/minibrowser/src/render/motion/mod.rs` — engine, params, atomic profile slot, 10 inline tests.
- `/home/cyber/Work/FF/minibrowser/src/render/keyboard/mod.rs` — typing engine, distributions, 8 inline tests.
- `/home/cyber/Work/FF/minibrowser/tests/motion_engine.rs` — shape + determinism tests.
- `/home/cyber/Work/FF/minibrowser/tests/typing_engine.rs` — distribution tests.
- `/home/cyber/Work/FF/minibrowser/tests/motion_live.rs` — `#[ignore]` live event-sequence integrity test.

Refactored:
- `/home/cyber/Work/FF/minibrowser/src/render/interact.rs` — `click_selector`, `mouse_move_to`, `type_text` rewired through the engines. New `click_point` / `dispatch_typing` primitives exposed for `ref_resolver`.
- `/home/cyber/Work/FF/minibrowser/src/render/ref_resolver.rs` — now reuses `click_point` and `dispatch_typing` instead of hand-rolled CDP loops.
- `/home/cyber/Work/FF/minibrowser/src/render/mod.rs` — export `motion`, `keyboard`.
- `/home/cyber/Work/FF/minibrowser/src/config.rs` — `motion_profile` field (cdp-backend gated).
- `/home/cyber/Work/FF/minibrowser/src/cli/args.rs` — `--motion-profile` flag.
- `/home/cyber/Work/FF/minibrowser/src/cli/mod.rs` — parse + `set_active()` at startup.
- `/home/cyber/Work/FF/minibrowser/tests/throughput_live.rs` — pinned to `MotionProfile::Fast` to preserve the 14.9 rps baseline.

## Public-API guarantee

No breakage. `interact::click_selector`, `mouse_move_to`, `type_text`, `MousePos` keep their signatures. All existing callers (`actions.rs`, `script/runner.rs`, `hooks/lua.rs`, `ref_resolver.rs`) compile unchanged. Behaviour upgrades happen through the ambient `MotionProfile::active()` slot.

## Gates

- `cargo build --all-features`: green (88s).
- `cargo build --no-default-features --features cli,sqlite`: green (49s).
- `cargo clippy --all-features --all-targets -- -D warnings`: green.
- `cargo test --all-features`: 400+ non-ignored tests pass; no regressions.
- `cargo test --all-features --test live_news_navigation -- --ignored`: PASS (31.6s).
- `cargo test --all-features --test spa_scriptspec_live -- --ignored`: PASS.
- `cargo test --all-features --test spa_deep_crawl_live -- --ignored`: PASS.
- `cargo test --all-features --test throughput_live -- --ignored`: PASS.
- `cargo test --all-features --test motion_live -- --ignored`: PASS — asserts `move_count_before_click ≥ 3`, `mouseover` precedes `mousedown`, `mousedown` precedes `mouseup`.

## Design notes / deviations

- **No `CursorState` type**. The plan proposed `Arc<Mutex<CursorState>>` attached to `Page`, but the codebase already threads `MousePos` through `ScriptRunner`, `actions::execute`, and the Lua bridge. Adding a second state carrier risked cross-session leaks and diverging sources of truth. Kept the existing per-run `MousePos`.
- **Atomic profile slot** (`AtomicU8`) instead of `task_local!`. Simpler, zero-contention, and motion profile is a process-level crawl setting in practice. Set once in `cmd_crawl`; readable from any async context including raw `interact::*` calls.
- **Fitts jitter centring**: per-step delays are recentered so the total trajectory duration matches Fitts MT, but each step still lives inside `[step_delay_min, step_delay_max]` so single samples look human even on short paths.
- **Typo correction** only fires on ASCII letters with QWERTY neighbour lookup. Unicode/IME typo modelling deferred (research does not require it for 2026 detectors).
- **Event-sequence integrity**: calling `dispatch_move` at the final point once more (before the mouseDown) makes Chrome's own hit-testing machinery emit `mouseover` + `mouseenter` inside the target. Avoids synthesising those events through CDP — the browser signals them for free once the cursor lands.

## Throughput impact (expected)

- `fast`: no overhead; trajectory ≤ 12 samples at ~1-3ms/step, zero post-move pauses.
- `balanced`: ~0.5-1s added per click (Fitts + OU + post-move pause + hold). Throughput expected to land around ~8 rps on click-heavy crawls; page-render-only crawls (no click) see no regression.
- `human` / `paranoid`: 2-10s per click by design.

The throughput_live test is now pinned to `fast` to keep the 14.9 rps baseline as a hard floor. Operators that want realism pick `balanced`/`human` via the CLI flag.

## Non-negotiables respected

- Stealth shim (`stealth.rs`, `stealth_shim.js`) untouched — Task B owns it.
- Chrome 149 patches untouched.
- Licences preserved (no files under `src/render/chrome/LICENSES` modified).
- No commits made.
- Mini build (`--features cli,sqlite`) green — motion engine sits behind `cdp-backend` where needed.
