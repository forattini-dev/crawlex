# Fase 6 — Isolamento de Sessão

Meta: policy automática de escopo, marcação de sessão, TTL + descarte explícito. Fecha M3 Super Crawl Beta.

## Estado atual

Hoje:
- `Config::render_session_scope: RenderSessionScope { RegistrableDomain | Origin | Url }` existe
- `SessionIdentity::state: SessionState { Clean | Warm | Contaminated | Blocked }` criado em Fase 1 antibot
- `ChallengeSignal` contamina session via `decide_post_challenge` → `SessionAction`
- `RenderPool` cacheia BrowserContexts por `(browser_key, session_id)` via DashMap
- ProxyRouter afinidade `(host, bundle_id) → proxy`

Falta:
- **Policy automática de scope**: hoje é config-fixo. Permitir promoção/demotion por policy (ex: crawl amplo começa `RegistrableDomain`, encontra login page → demote pra `Origin` pra não vazar cookie)
- **Session TTL** + descarte explícito: hoje session vive até processo morrer. Precisa expiry + cleanup
- **Marking consistente**: SessionState existe mas update points estão dispersos. Centralizar.
- **Session registry queryable**: `list_sessions(state_filter)` pra telemetria e cleanup manual

## Entregáveis

### 1. `SessionRegistry` centralizado

Novo módulo `src/session/mod.rs` (ou `src/identity/session_registry.rs`):
```rust
pub struct SessionRegistry {
    sessions: DashMap<String, SessionEntry>,
    ttl_ms: u64,
    cleanup_task: Option<JoinHandle<()>>,
}

pub struct SessionEntry {
    pub id: String,
    pub scope: RenderSessionScope,
    pub scope_key: String,          // e.g. "example.com" for RegistrableDomain
    pub bundle_id: u64,
    pub state: SessionState,
    pub created_at: Instant,
    pub last_used: Instant,
    pub ttl_override: Option<Duration>,
    pub challenges_seen: Vec<ChallengeSignal>,
    pub urls_visited: u32,
    pub proxy_history: Vec<Url>,
}

impl SessionRegistry {
    pub fn new(ttl_ms: u64) -> Self;
    pub fn get_or_create(&self, scope: RenderSessionScope, url: &Url, bundle_id: u64) -> String;
    pub fn mark(&self, id: &str, state: SessionState);
    pub fn touch(&self, id: &str);  // update last_used
    pub fn expired(&self) -> Vec<String>;  // ids with last_used > ttl
    pub fn drop(&self, id: &str);
    pub fn list(&self, filter: Option<SessionState>) -> Vec<SessionEntry>;
    pub fn start_cleanup_task(&self, storage: Arc<dyn Storage>) -> JoinHandle<()>;
}
```

### 2. Scope policy automática

Novo enum + decisor em `src/policy/engine.rs`:
```rust
pub enum ScopeDecision {
    Keep,
    PromoteTo(RenderSessionScope),  // e.g. Url → Origin
    DemoteTo(RenderSessionScope),   // e.g. RegistrableDomain → Origin
    Force(RenderSessionScope),      // override by operator
}

pub fn decide_scope(
    current: RenderSessionScope,
    signal: ScopeSignal,
) -> ScopeDecision;

pub enum ScopeSignal {
    LoginPageDetected,           // forms with password fields → demote to Origin
    AntibotHostility(ChallengeVendor),  // demote to Url (forensics)
    CrossOriginFetch(Url),       // promote may make sense
    HostQuarantined,             // force new Url scope
}
```

Regras iniciais conservadoras:
- `RegistrableDomain + LoginPageDetected` → `DemoteTo(Origin)` (cookies login não vazam)
- `*+AntibotHostility(HardBlock)` → `DemoteTo(Url)` (forensic contention)
- `Origin + CrossOriginFetch(same_registrable)` → `Keep` (normal)
- `* + HostQuarantined` → `Force(Url)` (evita reuso)

### 3. Session marking hooks

Adicionar call points pra `registry.mark(id, state)` onde hoje só existe info local:
- `src/render/pool.rs` pós-challenge → `Contaminated` or `Blocked` dependendo de level
- `src/render/pool.rs` pós-render OK → `Warm` (se era `Clean`)
- `src/policy/engine.rs::decide_post_challenge::KillContext` → `registry.drop(id)`
- `src/crawler.rs` errors repetidos no mesmo session → `Blocked`

### 4. TTL + cleanup

Background task periódico (60s):
```rust
async fn cleanup_loop(registry: Arc<SessionRegistry>, pool: Arc<RenderPool>, storage: Arc<dyn Storage>) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        for id in registry.expired() {
            pool.drop_session(&id).await;  // drop BrowserContext + cookies
            storage.archive_session(&id).await.ok();  // opcional
            registry.drop(&id);
        }
    }
}
```

`pool.drop_session(id)` limpa:
- BrowserContext correspondente (CDP `Target.disposeBrowserContext`)
- `session_states` entry em `RenderPool`
- PagePool pra esse context

### 5. CLI flags

- `--session-ttl-secs <N>` (default 3600 = 1h)
- `--session-scope <auto|registrable_domain|origin|url>` (default `auto` — policy decide)
- `--drop-session-on-block` (bool, default true)

### 6. Telemetria

SQLite expansão em `src/storage/sqlite.rs`:
```sql
CREATE TABLE IF NOT EXISTS sessions_archive (
    id TEXT PRIMARY KEY,
    scope TEXT,
    scope_key TEXT,
    state TEXT,
    bundle_id INTEGER,
    created_at INTEGER,
    ended_at INTEGER,
    urls_visited INTEGER,
    challenges INTEGER,
    final_proxy TEXT
);
```

`Storage::archive_session(id)` flush do `SessionEntry` + cleanup.

`list_sessions` CLI command:
```
crawlex sessions list [--state clean|warm|contaminated|blocked]
crawlex sessions drop <id>
```

### 7. Events

`EventKind::SessionStateChanged { session_id, from, to, reason }` — emit em cada mark.

`EventKind::SessionEvicted { session_id, reason: "ttl" | "blocked" | "manual", urls_visited, challenges_hit }` — emit em drop.

## Checklist

- [x] **`SessionRegistry` módulo novo** com `get_or_create`/`mark`/`touch`/`expired`/`drop`/`list`
- [x] **Scope policy automática**: `decide_scope` + `ScopeSignal` em policy engine
- [x] **Session marking hooks** no render pool + policy + crawler (pontos de update centralizados)
- [x] **TTL cleanup task**: spawn background, drop expired contexts + pages, archive
- [x] **`pool.drop_session(id)`**: limpa BrowserContext via CDP + session_states + PagePool
- [x] **CLI flags** (3): --session-ttl-secs, --session-scope, --drop-session-on-block
- [x] **SQLite `sessions_archive` table** + `archive_session` API
- [x] **CLI subcommand** `crawlex sessions list/drop`
- [x] **Events**: `SessionStateChanged` + `SessionEvicted`
- [x] **Testes unit** `tests/session_registry.rs`: get_or_create idempotence, mark transitions, expired detection, scope decisor rules
- [x] **Testes integration** `tests/session_scope_policy.rs`: scope demotion on login-page signal
- [x] **Teste live** estender `spa_deep_crawl_live` ou criar novo `session_lifecycle_live` que valida state transitions end-to-end (wiremock serve login page → scope demota → context reset)
- [x] **Gates verdes**: build all + mini + clippy + test + live HN + live SPA + live ScriptSpec + live throughput (baseline ~14.6 rps)
- [x] **Output** `.dispatch/tasks/phase6-session-isolation/output.md`

## Restrições
- Trilho: **Session Isolation** (parte final do M3). Exclusivo.
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde.
- Live HN test sem regressão (baseline ~33s).
- Throughput live test sem regressão (baseline ~14.6 rps, p95 ~601ms).
- Sem commits.
- ProxyRouter `host_affinity` tabela pode ter reference pra session_id — quando session drop, NÃO quebrar FK (ou cascade, ou mark as "gone" sem delete).
- SessionRegistry precisa ser Send+Sync (DashMap OK).

## Arquivos críticos
- `src/session/mod.rs` ou `src/identity/session_registry.rs` (novo)
- `src/policy/engine.rs` — decide_scope + ScopeSignal
- `src/render/pool.rs` — marking hooks + drop_session
- `src/crawler.rs` — marking em error/challenge paths
- `src/storage/sqlite.rs` — sessions_archive + archive_session
- `src/config.rs` — ttl + scope config
- `src/cli/args.rs` + `src/cli/mod.rs` — flags + subcommand
- `src/events/envelope.rs` — SessionStateChanged + SessionEvicted
- `tests/session_registry.rs` (novo)
- `tests/session_scope_policy.rs` (novo)
