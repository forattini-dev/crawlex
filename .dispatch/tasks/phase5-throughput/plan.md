# Fase 5 — Throughput / Super Crawling no browser

Meta: browser escalar sem explodir RAM/latência. Separar `Browser` / `BrowserContext` (session) / `Page` (tab). Pool de pages por browser/context. Reuso de tab. Scheduler com budgets por host/origin/proxy/session. Métricas reais.

## Estado atual

Hoje:
- `RenderPool` em `src/render/pool.rs` — gerencia browsers + contexts + session states via `DashMap`s
- `render()`/`render_with_script()` cria **Page nova** a cada call (abre tab, fecha no fim)
- `Semaphore` cap em `max_concurrent_render` controla paralelismo
- Browsers cached por `browser_key` (bundle+proxy) em `DashMap<String, Arc<Browser>>`
- BrowserContexts cached por `session_id` em `DashMap<(String, String), BrowserContextId>`
- Cada render = new Page dentro do context existente

Falta:
- **Pool de Pages** reuse — hoje abre/fecha a cada job. Custo: ~300-500ms per page setup (CDP domain enables, shim injection, context wire).
- **Scheduler com budgets** — hoje `Semaphore` global; falta caps por host / origin / proxy / session.
- **Métricas de throughput**: renders/min, tabs ativas, contexts ativos, tempo médio, memória por browser, challenges/proxy.
- **`MAX_BROWSERS` configurável**: hoje implícito no flag `max_concurrent_render`.

## Entregáveis

### 1. Page pool por context

`src/render/page_pool.rs` (novo):
```rust
pub struct PagePool {
    idle: Vec<PooledPage>,
    in_flight: usize,
    max_per_context: usize,
    last_cleaned: Instant,
}

pub struct PooledPage {
    page: chromiumoxide::Page,
    created_at: Instant,
    last_used: Instant,
    uses: u32,
    context_id: BrowserContextId,
}
```

API:
- `acquire(context) -> PooledPage` — retorna idle se existe, senão cria nova
- `release(page)` — devolve pro idle pool. Se `page.uses >= max_uses_per_page` OU `created_at > ttl` → close em vez de devolver
- `cleanup()` — background task evicta pages stale (`idle > 30s`)

Limites:
- `max_pages_per_context: 4` (default)
- `max_uses_per_page: 100` (rotação pra evitar leaks)
- `page_ttl: 300s`

### 2. RenderPool integra PagePool

`RenderPool::render_inner` (pós-Fase 2 helper) agora:
- Em vez de `context.new_page(...)` + `page.close()` → `page_pool.acquire(context).await` + `page_pool.release(page).await`
- Reset state entre uses: clear cookies opt-in, `page.goto("about:blank")` antes de release (ou no acquire se vier sujo)
- Se challenge detectado OU erro fatal → `close` em vez de release (page contaminada)

### 3. Scheduler com budgets

`src/scheduler.rs` (novo ou append em `crawler.rs`):
```rust
pub struct RenderBudgets {
    pub per_host_inflight: HashMap<String, AtomicUsize>,
    pub per_origin_inflight: HashMap<String, AtomicUsize>,
    pub per_proxy_inflight: HashMap<Url, AtomicUsize>,
    pub per_session_inflight: HashMap<String, AtomicUsize>,
    pub limits: BudgetLimits,
}

pub struct BudgetLimits {
    pub max_per_host: usize,      // default 4
    pub max_per_origin: usize,    // default 2
    pub max_per_proxy: usize,     // default 8
    pub max_per_session: usize,   // default 1 (SPA stateful)
}
```

`Crawler::process_job` render branch: antes de render, `budgets.try_acquire(host, origin, proxy, session)?`. Se algum budget estouro → reenqueue job com delay pequeno + emit `decision.made why=budget:<kind>`. Em `Drop` do guard, decrementa.

### 4. `MAX_BROWSERS` configurável

Hoje implícito. Tornar explícito:
- `Config::max_browsers: usize` default `4` (browsers simultâneos vivos)
- `RenderPool` respeita — se `browsers.len() >= max_browsers` e precisa de uma nova `browser_key`, evicta o browser menos-usado (LRU sobre `last_used`).

### 5. Métricas reais

`src/metrics.rs` expande com gauges/counters:
- `crawlex_renders_per_min` (rolling window)
- `crawlex_tabs_active` (pages in_flight)
- `crawlex_contexts_active`
- `crawlex_browsers_active`
- `crawlex_render_latency_ms_p50/p95/p99`
- `crawlex_memory_per_browser_mb` (via `Process.getProcessInfo` CDP se disponível, senão skip)
- `crawlex_challenges_per_proxy_per_min{proxy=...}`
- `crawlex_budget_rejections{kind=host|origin|proxy|session}`

Expostos via Prometheus endpoint existente.

### 6. CLI flags

- `--max-browsers <N>` (default 4)
- `--max-pages-per-context <N>` (default 4)
- `--max-per-host-inflight <N>` (default 4)
- `--max-per-origin-inflight <N>` (default 2)
- `--max-per-proxy-inflight <N>` (default 8)
- `--max-per-session-inflight <N>` (default 1)

### 7. Render outcome completing

Fechar ponta frouxa da Fase 4.3: agora `RenderedPage` carrega timing + challenge — `record_outcome` pro ProxyRouter no render path completo:
```rust
match rendered {
    Ok(page) => {
        if page.challenge.is_some() { router.record_outcome(proxy, ChallengeHit) }
        else { router.record_outcome(proxy, Success { latency_ms: duration.as_millis() }) }
    }
    Err(Error::Render(e)) if e.contains("timeout") => router.record_outcome(proxy, Timeout),
    ...
}
```

## Checklist

- [x] **`PagePool`** novo módulo com acquire/release/cleanup, limites configuráveis.
- [x] **`RenderPool` integra PagePool** — reuse de tabs via `acquire`/`release`. Page contaminada (challenge) → close.
- [x] **Reset state entre uses**: goto `about:blank` + optional cookie flush.
- [x] **Budgets scheduler** — `RenderBudgets` + `try_acquire` guard. Reenqueue com delay quando estourar.
- [x] **`MAX_BROWSERS` LRU**: browsers cache respeita cap + evicta LRU.
- [x] **Métricas novas** em `src/metrics.rs` + expose Prometheus. Background task pra renders_per_min rolling + latency histogram.
- [x] **CLI flags** (6).
- [x] **Render path `record_outcome` completo** — fecha Fase 4.3 ponta frouxa.
- [x] **Testes unit** `tests/page_pool.rs`: acquire/release sem browser (mock via trait); TTL/max-uses eviction; concurrent stress (sem race).
- [x] **Testes unit** `tests/render_budgets.rs`: budget rejection, release decrement, multi-key independence.
- [x] **Testes live** `tests/throughput_live.rs` `#[ignore]`: spawn 2-3 wiremock origins + render 10 URLs em paralelo — assert que budgets respeitaram limits, PagePool reusou tabs, latency_p95 razoável.
- [x] **Gates**: build all + mini + clippy + test + live HN + live SPA + live ScriptSpec + live throughput verdes.
- [x] **Output** `.dispatch/tasks/phase5-throughput/output.md`.

## Restrições
- Trilho: **Browser Scale**. Não tocar Fase 6 (isolation — próxima).
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde.
- Live HN test sem regressão (baseline ~33s).
- Sem commits.
- Sessões stateful (challenge session, SPA com cookies) NÃO podem compartilhar pages — per-session serialization garante isso.
- PagePool não pode introduzir race entre drop/release — use `Arc<Mutex<>>` ou channels se precisar.
- Métricas precisam ser low-overhead (<1% CPU) — atomics, não locks pesados.

## Arquivos críticos
- `src/render/page_pool.rs` (novo)
- `src/render/pool.rs` — integra PagePool, LRU browsers
- `src/scheduler.rs` (novo) OU `src/crawler.rs` — RenderBudgets + try_acquire
- `src/metrics.rs` — gauges/counters novos
- `src/config.rs` — campos novos
- `src/cli/args.rs` + `src/cli/mod.rs` — flags
- `src/crawler.rs` — render outcome record_outcome completo
- `tests/page_pool.rs` (novo)
- `tests/render_budgets.rs` (novo)
- `tests/throughput_live.rs` (novo, #[ignore])
