# Fase 5 — Throughput / Super Crawling — Output

## Gates
- `cargo build --all-features` — OK (1m41s)
- `cargo build --no-default-features --features cli,sqlite` — OK (~6s)
- `cargo clippy --all-features --all-targets -- -D warnings` — OK (43s)
- `cargo test --all-features` — 200+ tests, all passing
- `cargo test --all-features --test live_news_navigation -- --ignored` — PASS in 32.1s (baseline ~33s)
- `cargo test --all-features --test spa_scriptspec_live -- --ignored` — PASS in 1.19s
- `cargo test --all-features --test spa_deep_crawl_live -- --ignored` — PASS in 1.17s
- `cargo test --all-features --test throughput_live -- --ignored` — PASS: 10 renders em 0.68s, ~14.6 rps, p95 = 601ms, 2 tabs reused

## Entregas
1. **PagePool** (`src/render/page_pool.rs`):
   - `PagePoolLimits` com defaults do plano (4 pages/context, 100 uses, 300s ttl, 30s idle)
   - `try_acquire`/`register_fresh`/`release`/`release_dirty_key`/`cleanup_idle`/`drop_context`
   - `PageLease` RAII guard: `release_clean` normal, Drop = dirty
   - Counters atômicos por ctx (`in_flight`, `total_created`, `total_reused`)

2. **RenderPool integrado** (`src/render/pool.rs`):
   - `render_core` tenta reusar tab via `try_acquire` → reset `about:blank` → render
   - Miss ou reset-fail → cria fresh + instala stealth + spa_observer
   - Challenge/5xx/status=0 pós-render → `page.close()` + `release_dirty`
   - `MAX_BROWSERS` agora vindo de `config.max_browsers` (default 4, LRU preservado)
   - Eviction de browser LRU também derruba PagePool entries + contexts do key

3. **RenderBudgets scheduler** (`src/scheduler.rs`):
   - 4 dimensões (host/origin/proxy/session) com atomic CAS try_bump
   - `BudgetGuard` RAII decrementa 4 counters on drop
   - Rejection counters por kind
   - Wire no `Crawler::process_job`: requeue com 100ms backoff + `DecisionMade why=budget:<kind>`

4. **Métricas novas** (`src/metrics.rs` + `metrics_server.rs`):
   - Counters: `pages_created`, `pages_reused`, `budget_rejections_{host,origin,proxy,session}`
   - Gauges: `tabs_active`, `contexts_active`, `browsers_active`
   - Rolling window 60s: `renders_per_min`, `render_latency_ms_{p50,p95,p99}`
   - Labelled: `challenges_per_proxy_total{proxy=...}`

5. **CLI flags** (`src/cli/args.rs` + `mod.rs`):
   - `--max-browsers`, `--max-pages-per-context`
   - `--max-per-host-inflight`, `--max-per-origin-inflight`
   - `--max-per-proxy-inflight`, `--max-per-session-inflight`

6. **Render outcome** (`src/crawler.rs`):
   - Success → `ProxyOutcome::Success { latency_ms }`
   - Challenge → `ChallengeHit` (via handle_challenge) + counters.record_challenge
   - 5xx → `Status(s)`; status=0 → `Reset`
   - Err render → `Timeout`/`ConnectFailed`/`Reset` conforme msg
   - Fecha ponta frouxa Fase 4.3

## Testes novos
- `tests/page_pool.rs` — 5 testes unit: defaults, counter balance, drop_context, etc
- `tests/render_budgets.rs` — 6 testes: reject paths, unwind, session serialization, proxy isolation, stress concurrent
- `tests/throughput_live.rs` — 1 ignored: 10 renders paralelos, asserts reuse>0, p95<10s, counter integridade

## Arquivos tocados
- `src/render/page_pool.rs` (novo, ~285 linhas)
- `src/scheduler.rs` (novo, ~218 linhas)
- `src/render/pool.rs` — integração PagePool + counters + LRU config
- `src/render/mod.rs` — `pub mod page_pool`
- `src/lib.rs` — `pub mod scheduler`
- `src/config.rs` — campos max_browsers, max_pages_per_context, render_budgets
- `src/metrics.rs` — Counters expandido, RenderSamples rolling window
- `src/metrics_server.rs` — expose novas counters/gauges
- `src/crawler.rs` — budget guard, record_outcome render completo, render latency sample
- `src/cli/args.rs` — 6 flags novos
- `src/cli/mod.rs` — wire flags → Config

## Observações
- Reinject observer (`reinject_spa_observer`) funciona corretamente em tabs reusados: script está idempotente via `window.__crawlex_observer_installed__`
- PagePool não compartilha entre sessions (ctx_key = `browser_key|session_id`) → per-session serialization preservada
- SPA/challenge stateful tests passam sem regressão
- Throughput live mede ~14 rps num único Chrome com 2 origins wiremock locais

Fase 6 (isolation) é a próxima — não tocado aqui.
