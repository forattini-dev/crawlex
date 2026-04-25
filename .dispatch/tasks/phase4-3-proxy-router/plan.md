# Fase 4.3 — Proxy router EWMA + affinity

Substituir `Vec<banned: bool>` atual por `ProxyRouter` com score EWMA, quarentena, afinidade persistida.

## Checklist

- [ ] **Investigar estado atual**: `src/proxy/rotator.rs` — entender estrutura, callers (`rg "ProxyPool|ProxyRotator" src/`), integrações em `src/crawler.rs` e `src/render/`. Listar todos os sites a migrar.

- [ ] **Criar `src/proxy/router.rs`** com:
  ```rust
  pub struct ProxyScore {
      pub success: u32,
      pub timeouts: u32,
      pub resets: u32,
      pub status_4xx: u32,
      pub status_5xx: u32,
      pub challenge_hits: u32,
      pub latency_p50_ms: Option<f64>,
      pub latency_p95_ms: Option<f64>,
      pub last_success_at: Option<std::time::Instant>,
      pub quarantine_until: Option<std::time::Instant>,
  }
  pub enum ProxyOutcome {
      Success { latency_ms: f64 },
      Timeout,
      Reset,
      Status(u16),
      ChallengeHit,
      ConnectFailed,
  }
  pub struct ProxyRouter { ... }
  impl ProxyRouter {
      pub fn new(proxies: Vec<Url>, strategy: RotationStrategy, thresholds: PolicyThresholds) -> Self;
      pub fn pick(&self, host: &str, bundle_id: u64) -> Option<Url>;
      pub fn record_outcome(&self, proxy: &Url, outcome: ProxyOutcome);
      pub fn evict(&self, proxy: &Url);
      pub fn scores_snapshot(&self) -> Vec<(Url, ProxyScore)>;
  }
  ```
  EWMA α=0.2 pra latency. Quarentena: N falhas consecutivas (N=3 default) ou score_floor — defina pequeno struct `PolicyThresholds { max_consecutive_failures: u32, min_success_rate: f64, challenge_quarantine_secs: u64 }`. Affinity interno `DashMap<(String, u64), Url>`.

- [ ] **Schema SQLite**: em `src/storage/sqlite.rs`, novas tabelas:
  ```sql
  CREATE TABLE IF NOT EXISTS proxy_scores (
    url TEXT PRIMARY KEY,
    success INTEGER DEFAULT 0,
    timeouts INTEGER DEFAULT 0,
    resets INTEGER DEFAULT 0,
    status_4xx INTEGER DEFAULT 0,
    status_5xx INTEGER DEFAULT 0,
    challenge_hits INTEGER DEFAULT 0,
    latency_p50_ms REAL,
    latency_p95_ms REAL,
    last_success_at INTEGER,
    quarantine_until INTEGER,
    updated_at INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS host_affinity (
    host TEXT NOT NULL,
    bundle_id INTEGER NOT NULL,
    proxy_url TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (host, bundle_id)
  );
  ```
  Adicionar na migration list. API: `Storage::load_proxy_scores()`, `save_proxy_scores(snapshot)`, `load_host_affinity()`, `save_host_affinity(host, bundle_id, proxy)`.

- [ ] **Persistence throttled**: `ProxyRouter` acumula mudanças em memória, flush a cada N (≤16) mudanças OU 5s. Use `tokio::task` spawned ou `parking_lot::Mutex<Vec<Pending>>` + drain no flush. Startup: `load_proxy_scores()` → popula estado inicial.

- [ ] **Wire no HTTP path**: `src/http/*` ou `src/fetch/*` — onde quer que pool HTTP resolve proxy. Trocar `rotator.next()` por `router.pick(host, bundle_id)`. Pós-request: `router.record_outcome(proxy, Outcome::...)` com latência e status.

- [ ] **Wire no Render path**: `src/render/pool.rs` — proxy per-browser. `router.pick` no preflight. CDP timing events podem alimentar latency (ou usar start→nav_complete manual).

- [ ] **Policy engine hook**: `src/policy/engine.rs` — em `Decision::SwitchProxy`, consultar `router.pick` pra escolher substituto baseado em score, não random.

- [ ] **Feature-gate mini build**: router não depende de chromiumoxide. Garante `cargo build --no-default-features --features cli,sqlite` passa.

- [ ] **Deprecar `rotator.rs`**: depois de todos callers migrarem, `rm src/proxy/rotator.rs` + remover do `mod.rs`. Se algum teste depende dele, atualizar.

- [ ] **Testes**: `tests/proxy_router.rs` não-ignore, 3 cenários:
  - happy path: 10 success em sequência → p50 convergindo, quarantine=None
  - degradation: 5 success depois 3 timeouts → quarantine_until setado
  - recovery: após quarentena expirar, volta ao pool
  - affinity: `pick(host, bundle)` retorna mesmo proxy em chamadas subsequentes

- [ ] **Verify**: `cargo build --all-features`, `cargo build --no-default-features --features cli,sqlite`, `cargo clippy --all-features --all-targets -- -D warnings`, `cargo test --all-features`, live HN (`live_news_navigation`) precisa continuar PASS.

- [ ] **Output**: `.dispatch/tasks/phase4-3-proxy-router/output.md`.

## Restrições

- Não tocar 4.2, 4.4, 4.5 nem Lua.
- Não regredir live HN test.
- `Decision::SwitchProxy` já existe em policy/engine.rs — só estender.
- Challenge detection ainda não existe; deixar `ChallengeHit` wire-only (será conectado em 4.2).
- Sem commits.
- Mini build verde obrigatório.
