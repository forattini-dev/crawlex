# Wave 1 — Crawl pattern ML-tier (server-side behavioral)

Meta: fingerprints que detectores server-side fazem sobre o padrão de requests (não da request individual). Owner: `src/crawler.rs` + `src/scheduler.rs` + novos helpers.

## Items cobertos
- #31 request inter-arrival time jitter (humano: 500-5000ms entre clicks; crawler: 100ms)
- #32 click-through graph shape (humano hub-spoke; BFS uniforme detectável)
- #33 session depth distribution (humano: Pareto 3-7; crawler: 50+ uniforme)
- #23 Cookie CHIPS `SameSite=None; Secure; Partitioned` coverage
- #26 IndexedDB transaction order entropia audit

## Arquivos alvo
- `src/scheduler.rs` (inter-arrival scheduler + session depth caps)
- `src/crawler.rs` (click-graph shape policy)
- `src/http/cookies.rs` (CHIPS support)
- `tests/crawl_pattern_shape.rs`

## Checklist
- [x] `InterArrivalJitter` — scheduler adiciona delay entre jobs da mesma session:
  - log-normal μ=7.5, σ=1.0 (median ~1800ms, tail até 30s)
  - `--motion-profile fast` bypassa (dev) via `JitterProfile::Off`
  - Implementado em `src/scheduler.rs` (`InterArrivalJitter`,
    `JitterProfile`, `delay_for_next`). Wired em `Crawler::process_job`
    antes do dispatch de render; reusa registrable-domain como session key.
- [x] Session depth: `RenderBudgets` ganha `max_per_session_total` (default 15, Pareto-distributed cap)
  - Job que passaria do cap → `SessionDecision::EndSession` + re-queue
  - `SessionDepthTracker` em `src/scheduler.rs`; cap ~Pareto(α=1.3,xm=3) clamped
  - Decisão emite evento `decision.made why=session_depth:pareto_cap`
- [x] Click-graph shape: BFS→hub-spoke. Helper `WeightedFrontier` + `frontier_weight()`
  - Weights [1.0, 0.7, 0.5, 0.3, 0.15] em `DEFAULT_FRONTIER_WEIGHTS`
  - `pop_weighted()` faz weighted sampling (não strict FIFO)
  - Frontier real do crawler segue via queue existente — helper não
    substitui JobQueue; é opt-in para callers ML-tier + é testado
    em `tests/crawl_pattern_shape.rs`.
- [x] Cookie CHIPS: `src/http/cookies.rs`
  - Parser entende `Partitioned` attribute (regex case-insensitive)
  - Rejeita sem `SameSite=None; Secure` (tracing + counter)
  - Storage particionado por `(top_level_site, origin)` — isolation
    covered por teste `partitioned_cookie_isolation_across_top_level_sites`
- [~] IndexedDB audit: scope out — observer code owner-externo (render);
  observação fica na plan.md para próximo wave sem tocar em handler/pool.
- [x] Tests: `tests/crawl_pattern_shape.rs` — 8 testes: log-normal
  distribution shape, Off = no-op, Pareto depth cap, zero cap disable,
  weights monotone decay, weighted pick bias, depth histogram
  hub-spoke, CHIPS partition isolation.
- [x] Gates: mini build (exit 0), lib check mini (exit 0), clippy
  (only pre-existing warnings in outros owners; nada em schduler/
  crawler/http/cookies owned), crawl_pattern_shape (8/8), render_budgets
  (6/6) — sem regressão. Build `--all` falha por erros pre-existentes
  em `src/render/motion/submovement.rs` (owner externo).
- [x] Output + `.done`

## Restrições
- NÃO tocar stealth_shim, motion, pool, handler, impersonate, antibot
- Chrome 149 patches intocados
- Licenças preservadas
- Sem commits
- Live HN sem regressão
- Inter-arrival jitter default pode ser "soft" (50-500ms) pra testes; full log-normal só com `--motion-profile human+`
