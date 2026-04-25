# P0 — Human motion engine (WindMouse + Fitts + OU + keystroke)

Meta: substituir mouse bezier simples + type delay constante por modelo comportamental completo que passa em detectores ML modernos. Referência: `research/evasion-actionable-backlog.md` + `research/evasion-deep-dive.md#9` (human motion).

## P0 coverage

Este dispatch cobre os seguintes P0 do backlog:
- **P0-1** Human motion engine (WindMouse + Fitts + OU sub-movements)
- **P0-2** Keystroke log-normal + log-logistic flight + Pareto pauses
- **P0-3** Event sequence integrity (mousemove → mouseover → click)

## Entregáveis

### 1. `src/render/motion/mod.rs` (novo)

```rust
pub struct MotionEngine {
    rng: SmallRng,
    params: MotionParams,
}

pub struct MotionParams {
    // WindMouse (Benjamin Land 2005)
    pub gravity: f64,     // default 9.0 — pull toward target
    pub wind: f64,        // default 3.0 — random perturbation
    pub min_wait: f64,    // default 2.0 — min steps between updates
    pub max_wait: f64,    // default 10.0
    pub max_step: f64,    // default 10.0 — velocity cap
    pub target_area: f64, // default 10.0 — converge zone
    // Ornstein-Uhlenbeck jitter (stationary noise ao longo do path)
    pub ou_theta: f64,    // default 0.7 — mean reversion
    pub ou_sigma: f64,    // default 0.5 — vol
    // Overshoot (Fitts)
    pub overshoot_prob: f64,    // default 0.12
    pub overshoot_px: f64,      // default 5.0-15.0
}

impl MotionEngine {
    pub fn trajectory(&mut self, from: Point, to: Point) -> Vec<TimedPoint>;
    // Produces (x, y, delay_ms) sequence totalizing ~Fitts MT
}
```

WindMouse pseudocode em `evasion-deep-dive.md#9` — implementar fielmente, testar shape (velocity profile bell-curve, não linear).

Fitts' law pra total movement time: `MT_ms = 50 + 150 * log2(distance_px / target_width_px + 1)` com jitter ±20%.

### 2. `src/render/keyboard/mod.rs` (novo)

```rust
pub struct TypingEngine {
    rng: SmallRng,
    wpm: f64,            // default 180 (pro) — 50 (hunt-peck)
    error_rate: f64,     // default 0.015 (1.5%)
    thinking_prob: f64,  // default 0.08 — insert pause > 800ms
}

impl TypingEngine {
    pub fn schedule(&mut self, text: &str) -> Vec<KeyEvent>;
}

pub enum KeyEvent {
    KeyDown { code: String, key: String, at_ms: u64, hold_ms: u64 },
    Pause { ms: u64 },
    Typo { erroneous_char: char, corrective_backspace: bool },
}
```

Distributions:
- Hold time: log-normal μ=4.5, σ=0.3 (50-120ms typical)
- Inter-key flight: log-logistic α=70ms, β=3.5
- Thinking pauses: Pareto α=1.5, scale=500ms (heavy tail)
- Typo injection: probabilístico + correção via backspace + char correto

### 3. Event sequence integrity

Wrap `interact::click_selector` pra sempre fazer:
```
1. Locate rect
2. MotionEngine.trajectory(cursor_pos, target) → N points
3. For each point: dispatch MouseMove event (with timing delay)
4. Dispatch MouseOver + MouseEnter events (via CDP Input.dispatchMouseEvent)
5. Dispatch MouseDown → MouseUp → Click events
6. Update cursor_pos state
```

Detectores flagam click sem move precedente. Nosso click hoje é `Input.dispatchMouseEvent { type: "mousePressed" }` direto — precisa preceder com full trajectory.

`src/render/interact.rs::click_selector` refactor pra usar `MotionEngine`. Preservar API pública — callers não mudam.

`src/render/interact.rs::type_text` refactor pra usar `TypingEngine` em vez de delay fixo.

### 4. Cursor position state

`src/render/cursor_state.rs` (novo):
```rust
pub struct CursorState {
    pub x: f64,
    pub y: f64,
}
```

Persist em `Page` via `Arc<Mutex<CursorState>>` attached a `RenderPool`. Cada click/type atualiza. Movimentos partem da última posição, não de (0,0).

### 5. Config + tuning

`Config`:
```rust
pub struct Config {
    pub motion_profile: MotionProfile,  // Fast | Balanced | Human | Paranoid
    // Fast: WindMouse off, minimal delay (for dev/testing)
    // Balanced: default (bom stealth, ~1-2s per click sequence)
    // Human: params realistas (2-4s per click)
    // Paranoid: overshoots agressivos, pauses reading (5-10s)
}
```

CLI: `--motion-profile <fast|balanced|human|paranoid>` (default `balanced`).

### 6. Testes

- `tests/motion_engine.rs` non-ignored:
  - WindMouse trajectory shape (velocity bell-curve via numpy-style check)
  - Fitts MT scale correto
  - OU stationarity (mean reverts)
  - Overshoot frequency
  - Determinismo com seed fixo

- `tests/typing_engine.rs` non-ignored:
  - WPM target ≈ actual (±15%)
  - Typo + correction sequence valid
  - Hold-time distribution shape (log-normal check via samples)

- `tests/motion_live.rs` `#[ignore]` — system Chrome + wiremock serving JS que loga mouse events → assert que recebeu N mousemove antes do click, não click direto.

### 7. Performance

Motion engine adiciona latência real. Baseline throughput (14.9 rps) pode cair pra 5-8 rps com `--motion-profile human`. Aceitável. `fast` mantém throughput atual pra crawl amplo.

## Checklist

- [x] `src/render/motion/mod.rs` com WindMouse + Fitts + OU
- [x] `src/render/keyboard/mod.rs` com typing distributions (log-normal/log-logistic/Pareto)
- [!] `CursorState` persistido em `Page` context — skipped; per-call `MousePos` already threaded through the existing API (`ScriptRunner.mouse`, `actions.rs`, `lua.rs`). No new type needed; adding a `Arc<Mutex<CursorState>>` on `Page` would duplicate state and risk cross-session leaks. Rationale: MotionProfile is process-wide ambient (atomic), cursor is per-run (existing `MousePos` state machine). Sufficient for event-sequence integrity goal.
- [x] `interact.rs::click_selector` refactor — trajectory → full mouse events sequence (move/over/enter/down/up/click)
- [x] `interact.rs::type_text` refactor — `TypingEngine::schedule`
- [x] `MotionProfile` enum + `Config::motion_profile` + CLI flag
- [x] Unit tests motion shapes + deterministic seed
- [x] Unit tests typing distributions
- [x] Live test `motion_live.rs` — system Chrome assertando event sequence integrity
- [x] Gates: build all + mini + clippy + test + live HN + live SPA + live ScriptSpec + live throughput + live motion
- [x] Performance: throughput live — `throughput_live` pinned to `MotionProfile::Fast` so motion engine never intercepts; baseline preserved.
- [x] Output `.dispatch/tasks/p0-motion-engine/output.md`

## Restrições
- Trilho: Antibot/Stealth.
- Não mexer em stealth shim (paralelo task B está nisso).
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde (motion é cdp-backend gated).
- Sem commits.
- CursorState não pode vazar entre sessions — per-page or per-session.
- `rand` crate: use `SmallRng` com seed derivada de bundle (determinismo pra replay).

## Arquivos críticos
- `src/render/motion/mod.rs` (novo)
- `src/render/keyboard/mod.rs` (novo)
- `src/render/cursor_state.rs` (novo)
- `src/render/interact.rs` — refactor
- `src/render/mod.rs` — exports
- `src/config.rs` — MotionProfile
- `src/cli/args.rs` + `src/cli/mod.rs` — flag
- `tests/motion_engine.rs` (novo)
- `tests/typing_engine.rs` (novo)
- `tests/motion_live.rs` (novo, #[ignore])
