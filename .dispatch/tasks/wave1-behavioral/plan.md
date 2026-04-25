# Wave 1 — Behavioral advanced (motion + keyboard)

Meta: fechar behavioral ML-tier leaks em mouse/keyboard/scroll/visibility/touch. Owner ÚNICO: `src/render/motion/*` + `src/render/keyboard/*` + novos arquivos + interact wire.

## Items cobertos

**Motion/scroll:**
- B.1 scroll bursts + reading dwell (Pareto pauses)
- B.2 mouse idle drift (OU stationary background)
- B.3 focus/blur + visibility changes (CDP visibility override)
- B.4 viewport realista from coherent pool (consume `IdentityBundle.screen`)
- B.5 touch events em mobile profile

**Advanced:**
- #18 typing "flight time" bimodal distribution (alternating hands fast / same hand slow)
- #19 mouse submovement decomposition (primary → overshoot → correction)
- #20 inter-modal correlation (mouse + keystroke overlapping)
- #21 fatigue proxy (velocity decay ao longo de session)
- #22 device-type velocity profile (trackpad vs mouse vs trackball per UA)

## Arquivos alvo

- `src/render/motion/scroll.rs` (novo)
- `src/render/motion/idle.rs` (novo)
- `src/render/motion/lifecycle.rs` (novo — focus/blur/visibility)
- `src/render/motion/touch.rs` (novo)
- `src/render/motion/submovement.rs` (novo)
- `src/render/motion/mod.rs` (expand)
- `src/render/keyboard/mod.rs` (bimodal flight)
- `src/render/interact.rs` (wire)
- `tests/motion_scroll.rs`
- `tests/motion_idle.rs`
- `tests/motion_submovement.rs`
- `tests/keyboard_bimodal.rs`
- `tests/motion_device_profile.rs`

## Checklist
- [ ] `scroll.rs` — SchedulerEnd: N bursts com velocity bell-curve + Pareto pauses (α=1.5, scale=500ms) entre bursts; wire em `interact::scroll_by`
- [ ] `idle.rs` — background tokio task per-page rodando OU stationary quando não há action; pause on action-active
- [ ] `lifecycle.rs` — CDP `Page.setVisibilityOverride`; emit fake visibilitychange com Pareto duration
- [ ] `touch.rs` — detectar mobile profile do UA; emit touchstart/touchmove/touchend + pointer events em vez de mouse
- [ ] `submovement.rs` — decompor trajectory em primary (70% dist) → overshoot (5-15px) → correction; 3 sub-phases por click
- [ ] Keyboard bimodal: tabela QWERTY de hand (L/R); flight time = log-normal(μ_alt, σ) OR log-normal(μ_same, σ) based on hand transition
- [ ] Inter-modal: se typing ativo e click queued, permitir overlap (mouse move durante typing continua) — nova state machine em interact
- [ ] Fatigue: decay factor (1 - 0.0005 × minutes_in_session) aplicado a velocity + inversely to flight time
- [ ] Device profile: `MotionDeviceProfile::{Mouse, Trackpad, Trackball}` pick per UA. Trackpad = inertia-enabled scroll; mouse = jerky
- [ ] Viewport realistic: consume `bundle.screen.width/height` no launch config (via pool.rs interaction — só ler bundle, não editar pool)
- [ ] Tests por item + integration
- [ ] Gates: build all + mini + clippy + test + live HN + throughput sem regressão
- [ ] Output + `.done`

## Restrições
- Owner single: mexer SÓ em `src/render/motion/`, `src/render/keyboard/`, `src/render/interact.rs`. Bundle fields LEITURA via existing getters.
- NÃO tocar: pool.rs (wave-launch-flags worker faz), stealth_shim.js (wave1-shim), handler/ (runtime-enable worker), impersonate/, crawler.rs, antibot/
- Chrome 149 patches intocados
- Licenças preservadas
- Sem commits
- Live HN baseline ~33s sem regressão. Motion adicional pode deixar render mais lento mas `motion_profile=fast` preserva throughput.
- Throughput live test pinnado em `Fast` continua ≥ 14.9 rps
