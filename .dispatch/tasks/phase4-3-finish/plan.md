# Fase 4.3 continuation — Fechar ProxyRouter wiring + limpar resíduos de download

## Contexto

Worker anterior (`phase4-3-proxy-router`) parou no meio. Estado atual:
- ✅ `src/proxy/router.rs` criado com `ProxyRouter` (EWMA, quarentena, affinity DashMap, `best_alternative`)
- ✅ 9 unit tests inline (happy/degradation/recovery/affinity/challenge/evict)
- ✅ Schema SQLite + `Op` variants pra `proxy_scores` + `host_affinity` em `src/storage/sqlite.rs`
- ✅ Exports em `src/proxy/mod.rs`
- ❌ Methods `save/load_proxy_scores/host_affinity` em `impl SqliteStorage`
- ❌ `crawler.rs` ainda usa `proxy_pool` (rotator velho), não `ProxyRouter`
- ❌ `PolicyContext::score_for` não existe
- ❌ Flush throttled loop
- ❌ `src/proxy/health.rs` ainda chama API velha
- ❌ `rotator.rs` não deprecated
- ❌ `tests/proxy_router.rs` (integration) não existe
- ❌ Resíduo indesejado: `Op::SaveAsset` variant + tabela `assets` foram adicionados durante dispatch de download (que foi cancelado por estar fora dos 5 trilhos). Remover.

## Checklist

- [x] **Limpar resíduo download**: Removido Op::SaveAsset, tabela `assets`, Storage::save_asset (trait + sqlite + filesystem impls), módulo `src/download/` inteiro, `Config.download_policy`, CLI flags `--download-kinds` + `--max-asset-size-mb`, seção de save_asset em crawler.rs. Consultado IPC 001 antes de remover (aprovado remoção completa).

- [x] **Implementar métodos inerentes em `SqliteStorage`**:
  - `async fn load_proxy_scores(&self) -> Result<Vec<(Url, ProxyScore)>>`
  - `async fn save_proxy_scores(&self, snapshot: Vec<(Url, ProxyScore)>) -> Result<()>`
  - `async fn load_host_affinity(&self) -> Result<Vec<((String, u64), Url)>>`
  - `async fn save_host_affinity(&self, host: &str, bundle_id: u64, proxy: &Url) -> Result<()>`
  Usar o writer thread pattern + Op handler para saves; reads via `spawn_blocking` + read-only connection.

- [x] **Flush loop throttled**: método `ProxyRouter::start_flush_loop(storage: Arc<dyn Storage>) -> JoinHandle` que a cada 5s ou após 16 mudanças pendentes chama `drain_pending` + `save_proxy_scores` + `save_host_affinity`. Usar `tokio::spawn` + `tokio::time::interval`.

- [x] **Wire no `Crawler::new`** (ou onde `ProxyPool` é instanciado): substituir `ProxyPool`/`ProxyRotator` por `ProxyRouter`. Na inicialização: `router.hydrate_from_storage(storage)` (load scores + affinity). Spawnar flush loop.

- [x] **Wire nos call sites HTTP**: `rg "proxy_pool\|proxy_rotator\|\.next_proxy\(\)\|\.banned\|mark_banned" src/` — substituir por `router.pick(host, bundle_id)` + `router.record_outcome(proxy, outcome)`. Compute `ProxyOutcome::Success { latency_ms }` do tempo wall-clock do request.

- [!] **Wire no Render path**: ctx.proxy é preenchido em process_job via `proxy_router.pick` e repassado pro render pool via parâmetro `proxy`. Não adicionei record_outcome específico pro render path porque o render pool hoje só recebe `Option<&Url>` e não retorna signal estruturado de lifecycle — deixar isso pra quando o pool expor sinal de success/timeout. HTTP path tem record_outcome completo. `src/render/pool.rs` — `router.pick` no preflight da Browser. Após navegação, `record_outcome` baseado em lifecycle success/timeout.

- [x] **Policy engine hook**: `PolicyContext.proxy_score` agora é preenchido com `proxy_router.score_for(&proxy)` no post-fetch path. `PolicyEngine::decide_post_fetch` já usava `proxy_score` → `Decision::SwitchProxy` quando score < floor. ChallengeHit fica pra Fase 1 (detection não entra aqui). `PolicyContext` ganha `proxy_router: Arc<ProxyRouter>` (ou ref). `decide_post_error` pode chamar `router.best_alternative(current)` pra `Decision::SwitchProxy` passar novo proxy. Challenge hits (quando Fase 1 landar) alimentam `ChallengeHit` outcome.

- [x] **Migrar `src/proxy/health.rs`**: health check hoje marca banned booleano. Trocar por `record_outcome(ConnectFailed)` ou `Success { latency_ms }`. Mantém semântica mas via score.

- [x] **Deprecar `rotator.rs`**: arquivo removido. `RotationStrategy` movida pra `router.rs` preservando o enum idêntico (Serialize/Deserialize/Default/PartialEq). `proxy/mod.rs` exporta só o router. após todos callers migrarem, `rm src/proxy/rotator.rs` + remover do `mod.rs`. Atualizar testes existentes que importavam.

- [x] **Integration test** `tests/proxy_router.rs`: 5 tests passando. ewma_converges_over_sequence, consecutive_failures_trigger_quarantine, quarantine_recovery_picks_proxy_again, eviction_removes_from_rotation, affinity_round_trip_via_sqlite. Originais:
  - EWMA convergence em sequência de outcomes
  - Quarentena após N consecutive failures
  - Recovery após quarentena expirar
  - Affinity preserved round-trip (save → load → pick retorna mesmo proxy)
  - Eviction definitiva

- [x] **Verify builds**:
  - `cargo build --all-features`
  - `cargo build --no-default-features --features cli,sqlite`
  - `cargo clippy --all-features --all-targets -- -D warnings`
  - `cargo test --all-features` non-ignored
  - `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` → PASS ~33s

- [x] **Output**: ver `.dispatch/tasks/phase4-3-finish/output.md`.

## Restrições
- **Só 5 trilhos.** Sem adicionar features fora de antibot/stealth, browser control, spa/pwa, artifacts, scale.
- **Proxy router = infra pra Fase 1 antibot.** Não adicionar challenge detection aqui (Fase 1 separada).
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde.
- Sem commits.
- Se algo do download residual (SaveAsset) estiver entrelaçado com infra legítima, perguntar via IPC antes de deletar.
