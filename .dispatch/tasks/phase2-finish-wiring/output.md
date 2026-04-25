# phase2-finish-wiring — output

All three outstanding wiring items closed. ScriptSpec runner is now reachable end-to-end:
CLI → Config → Crawler → RenderPool → ScriptRunner → artifacts.

## Changes

### 1. `RenderPool::render_with_script`
`src/render/pool.rs` — refactored the 380-LoC `render()` body into a shared
`render_core<F>` helper parameterised on a closure `F: for<'p> FnOnce(&'p Page)
-> BoxFuture<'p, Result<bool>>`. The closure slots in between wait-strategy
settle and the Lua `on_after_load` hook, returning `mutated_after_load` so the
existing settle path is preserved.

- `Renderer::render(...)` delegates with a closure that runs
  `actions::execute_with_policy` (legacy Actions path).
- `RenderPool::render_with_script(url, wait, script, proxy)` delegates with a
  closure that builds a `Plan`, spins up `ScriptRunner`, runs it, and stashes
  the `RunOutcome` via an `Arc<Mutex<Option<RunOutcome>>>`. Returns
  `(RenderedPage, RunOutcome)`.

Ordering preserved: ScriptSpec runner → Lua `on_after_load` / `on_after_idle`
→ settle → screenshot.

### 2. CLI `--script-spec <path>`
`src/cli/args.rs` — added `script_spec: Option<String>` with
`conflicts_with = "actions_file"`. `src/cli/mod.rs` — added `load_script_spec`
loader (cdp-gated) and populated `Config::script_spec`. Mini-build guard
(`reject_browser_only_flags`) also rejects `--script-spec` for parity.

### 3. Config + Crawler integration
`src/config.rs` — added `#[cfg(feature = "cdp-backend")] #[serde(skip)] pub
script_spec: Option<crate::script::ScriptSpec>`. Mirrors the existing
`actions` field pattern so no new cross-build breakage.

`src/crawler.rs` — `process_job` render branch now dispatches to
`render_pool().render_with_script(...)` when `config.script_spec.is_some()`;
falls back to `render(...)` otherwise. `RunOutcome` logged at `debug`
(`target: "crawlex::script"` with step/capture/export counts) — artifact
persistence deferred to the phase-4 artifacts work.

### 4. Live test
`tests/spa_scriptspec_live.rs` — `#[ignore]`-gated, prefers
`/usr/bin/google-chrome`, wiremock fixture served with
`set_body_raw(..., "text/html")` so Chrome parses it instead of wrapping in
`<pre>` (the gotcha that tripped the first run). Inline ScriptSpec JSON
exercises: `wait_for` → `click` → `wait_for` → `screenshot(element)` →
`snapshot(ax_tree)`. Asserts all 5 steps pass, artifact kinds match, PNG
bytes >64, sha256 is 64-char hex.

## Gates

| Gate | Result |
|------|--------|
| `cargo build --all-features` | OK (31s) |
| `cargo build --no-default-features --features cli,sqlite` | OK (10s) |
| `cargo clippy --all-features --all-targets -- -D warnings` | OK |
| `cargo test --all-features` (non-ignored) | 53 lib + all integration PASS |
| `cargo test --all-features --test live_news_navigation -- --ignored` | PASS (32.75s) |
| `cargo test --all-features --test spa_scriptspec_live -- --ignored` | PASS (~1s after navigate) |

## Non-touched

- Patches Chrome 149 in `src/render/chrome/handler/{frame,target}.rs`: intact.
- `src/render/LICENSES/`: preserved.
- No commits created.
- Lua hook path (`on_after_load` / `on_after_idle`) still fires after the
  ScriptSpec runner.

## Key files

- `src/render/pool.rs` — `render_core`, `render_with_script`
- `src/config.rs` — `script_spec` field
- `src/cli/args.rs` — `--script-spec` flag
- `src/cli/mod.rs` — `load_script_spec` + mini-build guard
- `src/crawler.rs` — dispatch in `process_job`
- `tests/spa_scriptspec_live.rs` — new live end-to-end test
