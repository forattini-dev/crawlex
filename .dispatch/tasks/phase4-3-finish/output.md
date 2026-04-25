# Fase 4.3 finish — output summary

## Resumo

Removidos resíduos da task 4.4 download (fora dos 5 trilhos) e completado o wiring do ProxyRouter (score-driven, EWMA, quarentena, affinity persistida em SQLite). `rotator.rs` deprecated. Todos os critérios de build / clippy / test verdes.

## Mudanças

### Removido (resíduo da 4.4 download)
- `src/download/` (mod.rs + pipeline.rs) — deletado
- `src/storage/mod.rs` — `Storage::save_asset` trait method removido
- `src/storage/sqlite.rs` — `Op::SaveAsset`, tabela `assets`, impl `save_asset` removidos
- `src/storage/filesystem.rs` — impl `save_asset` removido
- `src/config.rs` — `Config.download_policy` field + default removido
- `src/cli/args.rs` — flags `--download-kinds` + `--max-asset-size-mb` removidas
- `src/cli/mod.rs` — parsing de `DownloadPolicy::from_spec` removido
- `src/crawler.rs` — seção de classify + should_store + save_asset + `DecisionMade size-exceeded` removida (~65 linhas)
- `src/lib.rs` — `pub mod download;` removido

EventKind `ArtifactSaved` preservado (é usado por scripts/captures/screenshots, trilho "artifacts").

### ProxyRouter wiring
- `src/proxy/rotator.rs` — deletado. `RotationStrategy` movido pra `router.rs` (mesma API).
- `src/proxy/mod.rs` — export enxuto só do router.
- `src/proxy/router.rs`:
  - `pending_dirty_len()` helper
  - `pack_score_rows(drained)` — converte `Vec<(Url, ProxyScore)>` em rows SQLite (Instant → unix seconds via anchor).
  - `hydrate_from_storage(router, storage)` — lê `proxy_scores` + `host_affinity` e repopula router.
  - `start_flush_loop(router, storage, interval, batch_threshold)` — helper standalone (não usado diretamente pelo crawler porque o crawler usa o path Arc<dyn Storage> + downcast; mantido como helper público pra outros callers).
- `src/proxy/health.rs` — reescrito. Canary probe → `ProxyOutcome::Success { latency_ms }` / `Timeout` / `ConnectFailed` → `router.record_outcome`.
- `src/storage/sqlite.rs`:
  - Métodos inerentes `save_proxy_scores`, `save_host_affinity`, `load_proxy_scores`, `load_host_affinity` (reads via read-only spawn_blocking; writes via Op no writer thread).
  - `Storage::as_any_ref` impl pra habilitar downcast do dyn handle.
- `src/crawler.rs`:
  - `proxy_pool: Arc<ProxyPool>` → `proxy_router: Arc<ProxyRouter>`.
  - `ctx.proxy = proxy_router.pick(&host, 0)` (bundle_id=0 por ora; stub pra multi-identity futuro).
  - HTTP fetch: grava `ProxyOutcome::Success { latency_ms }` / `Status(code)` no path de sucesso e `ConnectFailed` / `Reset` no path de erro.
  - `PolicyContext.proxy_score` populado via `proxy_router.score_for(&proxy)` — habilita `Decision::SwitchProxy` no engine.
  - Hydrate from storage + flush loop spawned (gated `cfg(feature = "sqlite")`). Flush tick 5s drena pending e persiste via writer thread.
  - Health spawn recebe `Arc<ProxyRouter>`.

### Testes
- `tests/proxy_router.rs` — 5 integration tests (sqlite-gated):
  - ewma_converges_over_sequence
  - consecutive_failures_trigger_quarantine
  - quarantine_recovery_picks_proxy_again
  - eviction_removes_from_rotation
  - affinity_round_trip_via_sqlite (save → new storage handle → hydrate → pick retorna mesmo proxy + score histórico preservado)
- Unit tests inline em `router.rs` (9) preservados — todos verdes.

## Verificação (executada)

```
cargo build --all-features                                              → OK (1m01s)
cargo build --no-default-features --features cli,sqlite                 → OK (10s)
cargo clippy --all-features --all-targets -- -D warnings                → OK (1m03s)
cargo test --all-features                                               → OK, 0 failed
cargo test --all-features --test proxy_router                           → 5 passed
cargo test --all-features --test live_news_navigation -- --ignored      → 1 passed (32.78s)
```

## Caveats / notas

- Render path não grava `ProxyOutcome` por ora (plan item marcado `[!]`). O `RenderPool::render` só recebe `Option<&Url>` e não retorna signal estruturado de lifecycle; `record_outcome` do lado render deve entrar quando Fase 1 antibot landar (challenge detection + structured lifecycle result).
- `bundle_id` hardcoded em 0 porque não há multi-identity per run ainda. O campo na API já existe pra quando identity bundles forem per-job.
- Flush loop usa downcast do `Arc<dyn Storage>` pra `SqliteStorage` por tick. Alternativa seria guardar `Option<Arc<SqliteStorage>>` no Crawler, mas isso vaza o concrete type — ficou com downcast por ora.
- Clippy `--no-default-features --features cli,sqlite` tem um erro preexistente em `cli/mod.rs:621` (manual_map em cfg block) — fora do escopo, não bloqueia nenhum dos critérios que o plan pediu.

## Arquivos chave tocados

- `src/proxy/router.rs` (+168 lines — hydrate, pack, flush helper)
- `src/proxy/mod.rs` (simplified, re-exports)
- `src/proxy/health.rs` (rewritten para router outcomes)
- `src/proxy/rotator.rs` (deleted)
- `src/storage/sqlite.rs` (+inherent load/save methods + as_any_ref; -SaveAsset path)
- `src/storage/filesystem.rs` (-save_asset impl)
- `src/storage/mod.rs` (-save_asset trait method)
- `src/crawler.rs` (router wiring, hydrate, flush, outcome recording, proxy_score pra policy; -download section)
- `src/config.rs` (-download_policy)
- `src/cli/args.rs` (-download CLI flags)
- `src/cli/mod.rs` (-download_policy parse)
- `src/lib.rs` (-pub mod download)
- `src/download/` (deleted recursively)
- `tests/proxy_router.rs` (new)
