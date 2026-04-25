# Fase 6 — Session Isolation — output

Fase 6 fecha M3 Super Crawl Beta. Trilho: Session Isolation.

## Entregas

### 1. SessionRegistry central (`src/identity/session_registry.rs`)
- `SessionRegistry` DashMap-backed, `Send + Sync`.
- API: `get_or_create`, `touch`, `mark`, `bump_challenge`, `set_bundle`,
  `record_proxy`, `set_ttl_override`, `expired`, `evict`, `contains`,
  `get`, `list`, `len`, `scope_key_for`.
- `SessionEntry` com todos os campos do plan (id, scope, scope_key,
  bundle_id, state, created/last_used instantes + unix, ttl_override,
  urls_visited, challenges_seen, proxy_history).
- `SessionSnapshot` serializável para eventos/CLI.
- `EvictionReason` (Ttl|Blocked|Manual|RunEnded) com `as_str`.
- `SessionDropTarget` trait + `spawn_cleanup_task` (tick loop).
- `SessionArchive` trait + `StorageArchive` adapter para qualquer `Storage`.

### 2. Policy de scope (`src/policy/engine.rs`)
- `ScopeSignal { LoginPageDetected, AntibotHostility(vendor, level),
  HostQuarantined, CrossOriginFetch }`.
- `ScopeDecision { Keep, DemoteTo(s), PromoteTo(s), Force(s) }`.
- `decide_scope(current, &signal)` pure:
  - Login + escopo > Origin → DemoteTo(Origin); senão Keep.
  - HardBlock + escopo > Url → DemoteTo(Url); senão Keep.
  - ChallengePage/WidgetPresent + escopo > Origin → DemoteTo(Origin).
  - Suspected → Keep.
  - HostQuarantined → Force(Url) sempre.
  - CrossOriginFetch → Keep (placeholder para evolução).

### 3. RenderPool — `drop_session` + `SessionDropTarget`
- `RenderPool::drop_session(&self, session_id)` percorre
  `contexts` DashMap, computa `(browser_key, ctx_key, BrowserContextId)`,
  purga PagePool (`drop_context`) e chama CDP
  `Target.disposeBrowserContext`. Limpa `session_states` e atualiza
  counter.
- `impl SessionDropTarget for RenderPool`.

### 4. Crawler wiring
- Campo novo `session_registry: Arc<SessionRegistry>` + `render_scope:
  Arc<RwLock<RenderSessionScope>>`.
- `run()` spawna cleanup task (tick = `session_ttl_secs/4` min 30s)
  quando `max_concurrent_render > 0`. Aborta via `CleanupAbort` RAII.
- `flush_sessions_on_run_end()` arquiva cada entry vivo com
  `EvictionReason::RunEnded`.
- Render OK: `get_or_create` + `record_proxy` + promote Clean→Warm +
  `SessionStateChanged`.
- `handle_challenge` (pós-desafio): registra entry, bump challenge,
  persiste estado e emite `SessionStateChanged`. Se
  `session_scope_auto`, roda `decide_scope` + gravando
  `render_scope.write()`. Se `drop_session_on_block` e state = Blocked,
  chama `evict_session` → dispose BrowserContext + archive +
  `SessionEvicted`.

### 5. SQLite `sessions_archive`
- Schema novo em `src/storage/sqlite.rs`:
  ```sql
  CREATE TABLE sessions_archive (
    id PRIMARY KEY, scope, scope_key, state, bundle_id,
    created_at, ended_at, urls_visited, challenges, final_proxy, reason
  );
  ```
  + índices state e ended_at.
- `Op::ArchiveSession` + upsert.
- `archive_session_row(ArchivedSessionRow)` + `list_archived_sessions`
  inherent methods.
- `impl Storage::archive_session` no SqliteStorage (serializa entry →
  archive row).
- Default no-op em `Storage` trait (memory/filesystem backend).

### 6. Config novos campos
- `session_ttl_secs: u64` (default 3600).
- `drop_session_on_block: bool` (default true).
- `session_scope_auto: bool` (default false).
- `RenderSessionScope` agora PartialEq + Eq.

### 7. CLI flags + subcommand (`src/cli/args.rs` + `src/cli/mod.rs`)
- `--session-ttl-secs <N>`
- `--session-scope-auto`
- `--keep-blocked-sessions` (inverso de drop_session_on_block)
- `crawlex sessions list --storage-path <db> [--state <clean|warm|contaminated|blocked>]`
- `crawlex sessions drop --storage-path <db> --id <session>`
  (grava archive row com reason=manual; não toca pool em processo
  externo.)

### 8. Events
- `EventKind::SessionStateChanged` (`session.state_changed`) —
  payload `{from, to, reason}`.
- `EventKind::SessionEvicted` (`session.evicted`) —
  payload `{reason, state, urls_visited, challenges_seen, scope_key}`.

### 9. Testes
- `tests/session_registry.rs` (12 testes):
  get_or_create idempotência, mark monotonic no consumer,
  expired, list filter, scope_key_for granularity + cleanup task
  end-to-end (NoopDrop + CaptureArchive).
- `tests/session_scope_policy.rs` (6 testes): matrix de
  `decide_scope`.
- Live suite inalterada:
  - HN: 31.94s (baseline 33s).
  - SPA ScriptSpec: pass.
  - SPA Deep Crawl: pass.
  - Throughput: 14.9 rps, p95 612 ms (baseline 14.6 rps, p95 601 ms).

## Gates

| Gate | Status |
|---|---|
| `cargo build --all-features` | ok (1m) |
| `cargo build --no-default-features --features cli,sqlite` | ok (11s) |
| `cargo clippy --all-features --all-targets -- -D warnings` | ok |
| `cargo test --all-features` (non-ignored) | ok — todos verdes |
| `live_news_navigation --ignored` | ok 31.94s |
| `spa_scriptspec_live --ignored` | ok |
| `spa_deep_crawl_live --ignored` | ok |
| `throughput_live --ignored` | ok 14.9 rps, p95 612 ms |

## Restrições cumpridas
- Patches Chrome 149 intocados (`src/render/chrome/handler/frame.rs`,
  `target.rs`).
- Licenças em `src/render/LICENSES/` preservadas.
- Mini build obrigatório verde.
- Sem commits.
- ProxyRouter `host_affinity` inalterado — `sessions_archive` não
  referencia `host_affinity` (soft reference por session_id string, sem
  FK).
- SessionRegistry é `Send + Sync` (DashMap).

## Arquivos tocados
- `src/config.rs` — 3 campos novos + PartialEq/Eq em scope.
- `src/identity/mod.rs` — re-exports.
- `src/identity/session_registry.rs` (novo).
- `src/policy/mod.rs` — re-exports.
- `src/policy/engine.rs` — ScopeSignal/ScopeDecision/decide_scope.
- `src/render/pool.rs` — drop_session + `impl SessionDropTarget`.
- `src/storage/mod.rs` — trait `archive_session` default no-op.
- `src/storage/sqlite.rs` — schema + Op + list + impl.
- `src/events/envelope.rs` — 2 novos EventKind.
- `src/cli/args.rs` — CLI flags + SessionsCmd subcommand.
- `src/cli/mod.rs` — cmd_sessions + config wiring.
- `src/crawler.rs` — registry field, cleanup task, eviction hooks.
- `tests/session_registry.rs` (novo).
- `tests/session_scope_policy.rs` (novo).
