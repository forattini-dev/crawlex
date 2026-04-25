# Fase 3 — SPA/PWA Deep Crawl

Meta: parar de pegar só snapshot final; rastrear app state e rotas úteis em runtime. Capturar manifest, service workers, IndexedDB, Cache Storage, rotas observadas via History API, fetch/XHR endpoints vistos durante navegação.

## Estado atual

Já existe:
- `captured_urls` no `RenderedPage` (via CDP Network events pra resources não-document)
- `final_url` reflete pushState (via `window.location.href`)
- `SessionStateBlob` captura cookies + localStorage em `capture_session_state` (`src/render/pool.rs`)
- `service_worker_urls` + `manifest_url` em `RenderedPage` (via CDP + origin state)
- `src/discovery/pwa.rs` — seed/probe via HTTP (manifest + SW lookup standalone)
- `tests/spa_scriptspec_live.rs` com wiremock SPA + pushState

Falta:
- **History API observer** injetado — capturar `pushState`/`replaceState`/`popstate`/`hashchange` em tempo de execução, acumular rotas
- **Fetch/XHR observer** injetado — acumular endpoints chamados em runtime (além dos Network events CDP)
- **IndexedDB inventory** — listar DB names + object stores
- **Cache Storage keys** — listar caches + keys
- **Artifacts estruturados** usando `save_artifact` da Fase 4:
  - `snapshot.runtime_routes` — JSON com rotas SPA observadas
  - `snapshot.network_endpoints` — JSON com fetch/XHR capturados
  - `snapshot.indexeddb` — JSON com inventory
  - `snapshot.cache_storage` — JSON com keys
  - `snapshot.manifest` — JSON do webmanifest
  - `snapshot.service_workers` — JSON com regs + script URLs

## Entregáveis

### 1. Observer JS injection

Novo bundle em `src/render/spa_observer.rs` (ou append ao stealth shim):
- `history.pushState` / `replaceState` override que loga em `window.__crawlex_runtime_routes__`
- `window.addEventListener('popstate'/'hashchange')` logger
- `window.fetch` wrapper (preserva semântica) que loga `{url, method, started_at}` em `window.__crawlex_network_endpoints__`
- `XMLHttpRequest.prototype.open` wrapper com mesmo log

Injetado via `Page.addScriptToEvaluateOnNewDocument` logo após stealth shim.

### 2. Collectors pós-settle

Em `src/render/pool.rs` (junto com `capture_session_state`):
```rust
pub struct RuntimeObservations {
    pub routes: Vec<RouteObservation>,     // pushState/replace/popstate/hashchange
    pub endpoints: Vec<NetworkEndpoint>,   // fetch/XHR
    pub indexeddb: Vec<IndexedDbInventory>,
    pub cache_storage: Vec<CacheStorageInventory>,
    pub manifest_data: Option<serde_json::Value>,
    pub service_workers: Vec<ServiceWorkerObservation>,
}
```

Collect via `Runtime.evaluate` lendo `window.__crawlex_runtime_routes__` e `__crawlex_network_endpoints__`. IndexedDB via CDP `IndexedDB.requestDatabaseNames` + `IndexedDB.requestDatabase`. Cache Storage via CDP `CacheStorage.requestCacheNames` + `requestEntries`.

### 3. Artifact emission

Cada collector produz JSON → `save_artifact` com kind apropriado:
- `ArtifactKind::SnapshotRuntimeRoutes`
- `ArtifactKind::SnapshotNetworkEndpoints`
- `ArtifactKind::SnapshotIndexedDb`
- `ArtifactKind::SnapshotCacheStorage`
- `ArtifactKind::SnapshotManifest`
- `ArtifactKind::SnapshotServiceWorkers`

Adicionar esses 6 variants em `ArtifactKind` enum (Fase 4 já tem infra — expandir enum + wire_str + mime).

### 4. Discovery integration

`RenderedPage` ganha:
```rust
pub struct RenderedPage {
    // existing fields...
    pub runtime_routes: Vec<Url>,       // absolute URLs derivadas de routes
    pub network_endpoints: Vec<Url>,    // endpoints unique
}
```

Crawler incorpora `runtime_routes` + `network_endpoints` ao conjunto de URLs descobertas (push no frontier via dedupe).

### 5. SPA detection heuristic

Flag simples: se `routes.len() > 0` após settle OU `final_url.fragment()` difere do initial → marca `RenderedPage::is_spa: bool`. Usado pra telemetria + opcional policy futura.

### 6. Config flags (não-breaking)

`Config`:
- `collect_runtime_routes: bool` (default true quando render)
- `collect_network_endpoints: bool` (default true)
- `collect_indexeddb: bool` (default false — caro)
- `collect_cache_storage: bool` (default false — caro)

CLI:
- `--collect-spa-state` (on/off conjunto) ou `--no-spa-observer` pra desabilitar batch.

### 7. Integração com Fase 4 artifacts

Reusa `save_artifact` + `list_artifacts`. Artifacts consultáveis via SQLite por kind. Events `ArtifactSaved` já enriched.

## Checklist

- [x] **Observer JS bundle**: `src/render/spa_observer.rs` com 4 wrappers (history/popstate/fetch/XHR). Injectado via `Page.addScriptToEvaluateOnNewDocument` no setup da Page.

- [x] **Collectors pós-settle**: `collect_runtime_observations(page)` em `src/render/pool.rs` lê globals JS + chama CDP IndexedDB/CacheStorage/manifest fetcher. Retorna `RuntimeObservations`.

- [x] **Expand `ArtifactKind`**: adicionar 6 novos variants + atualizar `wire_str`, `mime`, `extension`.

- [x] **Wire no render path**: após `settle_after_actions` + challenge detect, se `config.collect_*` flags ligadas, `save_artifact` pra cada observation (skip vazios).

- [x] **RenderedPage extension**: `runtime_routes: Vec<Url>`, `network_endpoints: Vec<Url>`, `is_spa: bool`.

- [x] **Crawler frontier integration**: `process_job` render branch incorpora `runtime_routes` + `network_endpoints` no set de URLs descobertas (via dedupe path existente).

- [x] **Config + CLI flags**: campos em `Config` + flags CLI. Default: routes/endpoints on, indexeddb/cache_storage off.

- [x] **IndexedDB via CDP**: `IndexedDB.enable` + `requestDatabaseNames` + `requestDatabase` — estrutura: `{db_name, version, stores: [{name, key_path, indexes}]}`.

- [x] **Cache Storage via CDP**: `CacheStorage.requestCacheNames` + `requestEntries` (first N keys). Estrutura: `{cache_name, keys: [url]}`.

- [x] **Manifest JSON fetch**: se `manifest_url` existe, baixar body + JSON-parse + emitir `SnapshotManifest`.

- [x] **Service workers JSON**: já temos `service_worker_urls` — estruturar como `{reg_scope, script_url, state}`.

- [x] **Testes unit** `tests/spa_observer_js.rs`: validar JS injection syntactical + globals shape (teste pure sem browser via chumbo do JS em string e parse-check).

- [x] **Testes live** estender `spa_scriptspec_live` ou criar `spa_deep_crawl_live.rs` `#[ignore]`:
  - SPA wiremock com pushState no click + fetch call no botão
  - Após render, verificar `runtime_routes` contém route nova
  - `network_endpoints` contém URL do fetch
  - `list_artifacts(session_id)` retorna `SnapshotRuntimeRoutes` + `SnapshotNetworkEndpoints`
  - System Chrome preferido.

- [x] **Gates**: build all + mini + clippy + test + live HN + live SPA suites verdes.

- [x] **Output** `.dispatch/tasks/phase3-spa-pwa/output.md`.

## Restrições

- Trilho: **SPA/PWA Crawl**. Não misturar com Fase 5/6.
- Observer JS injetado como parte do stealth shim — não expor `window.__crawlex_*` em produção (prefixo `__crawlex_` tá OK porque stealth path já neutraliza nomes suspeitos; MAS ver se outros detectores olham window globals).
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde.
- Live HN test sem regressão.
- Sem commits.
- Observer NÃO deve quebrar fetch/XHR legítimos — wrappers precisam preservar semântica (await, error handling, response types).
- IndexedDB/CacheStorage opt-in por default — coleta pesada.
- Performance: collect pós-settle adiciona ~100-200ms por render. Aceitável.

## Arquivos críticos
- `src/render/spa_observer.rs` (novo — ou append stealth shim)
- `src/render/pool.rs` — collectors + wire
- `src/render/mod.rs` — `RenderedPage` expansion
- `src/storage/mod.rs` — `ArtifactKind` expand
- `src/crawler.rs` — frontier integration
- `src/config.rs` — flags
- `src/cli/args.rs` + `src/cli/mod.rs` — CLI flags
- `src/discovery/pwa.rs` — touch mínimo (browser agora é fonte primária)
- `tests/spa_deep_crawl_live.rs` (novo)
