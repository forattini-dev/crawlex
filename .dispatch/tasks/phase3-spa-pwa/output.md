# Fase 3 — SPA/PWA Deep Crawl — Output

## Entregue

### Observer JS
- `src/render/spa_observer.rs` (novo): bundle JS com 4 wrappers
  (`history.pushState`/`replaceState`, `popstate`, `hashchange`,
  `window.fetch`, `XMLHttpRequest.prototype.open/send`) +
  serde types (`RouteObservation`, `NetworkEndpointObservation`,
  `CollectedObservations`) + `collect_expression()` helper.
- Hard cap `OBSERVER_SAMPLE_CAP = 2000` por array pra
  limitar OOM em SPAs patológicas.
- Semântica preservada: `fetch` retorna Promise original, XHR
  open/send identity-forward, erros propagam, `loadend` listener
  registra status/duration no XHR.

### Collectors pós-settle
Em `src/render/pool.rs`:
- `install_spa_observer` + `reinject_spa_observer` (dupla-instalação:
  `Page.addScriptToEvaluateOnNewDocument` registra pra docs futuros,
  e reinjeção post-nav garante captura do primeiro documento —
  na prática o Chrome 149 que eu testei NÃO dispara
  addScriptToEvaluateOnNewDocument pro first-document load, então
  a reinjeção é obrigatória).
- `collect_spa_observations` — lê os globais via
  `Runtime.evaluate` com `return_by_value`.
- `collect_indexeddb_inventory` — via `indexedDB.databases()` +
  `open()` + enumeração de object stores (opt-in).
- `collect_cache_storage_inventory` — via `caches.keys()` +
  `caches.open()` + `keys().slice(0, 500)` (bounded, opt-in).
- `fetch_manifest_json` — download via in-page `fetch()` (reusa
  cache/credenciais do Chrome), parse JSON.
- `persist_spa_artifacts` — emite artifacts só quando não-vazios.

### ArtifactKind (Fase 4)
Adicionados 6 variants em `src/storage/mod.rs`:
- `SnapshotRuntimeRoutes` → `snapshot.runtime_routes` (application/json)
- `SnapshotNetworkEndpoints` → `snapshot.network_endpoints`
- `SnapshotIndexedDb` → `snapshot.indexeddb`
- `SnapshotCacheStorage` → `snapshot.cache_storage`
- `SnapshotManifest` → `snapshot.manifest`
- `SnapshotServiceWorkers` → `snapshot.service_workers`

`wire_str`, `mime`, `extension`, `from_wire` todos
atualizados — backend filesystem e sqlite usam
`wire_str`/`from_wire` transparente, zero mudança.

### RenderedPage
`src/render/mod.rs` ganhou (cfg=`cdp-backend`):
- `runtime_routes: Vec<Url>` — absolutos, http(s) only, dedup
- `network_endpoints: Vec<Url>` — idem
- `is_spa: bool` — `!routes.is_empty() || final_url.fragment() != seed.fragment()`

Ambos são incorporados em `captured_urls` (se config flags on) — o
caminho de frontier do crawler herda sem mudanças.

### Config + CLI
`src/config.rs` ganhou 6 campos (com `#[serde(default)]` pra não
quebrar JSONs antigos):
- `collect_runtime_routes` (true)
- `collect_network_endpoints` (true)
- `collect_indexeddb` (false — pesado)
- `collect_cache_storage` (false — pesado)
- `collect_manifest` (true)
- `collect_service_workers` (true)

CLI flags novas em `src/cli/args.rs`:
- `--no-spa-observer` → desliga routes+endpoints+manifest+SW
- `--collect-indexeddb`, `--collect-cache-storage` (opt-in)
- `--collect-spa-state` → umbrella liga TUDO (inclui os caros)

### Crawler
`src/crawler.rs`: evento `RenderCompleted` agora inclui
`runtime_routes`, `network_endpoints`, `is_spa` pra telemetria.
Frontier pega URLs via `captured_urls` (inalterado).

### Testes
- `tests/spa_observer_js.rs` (novo, não-ignored): valida shape do
  JS (tokens esperados, braces balanceadas) + parse serde dos tipos.
  3 testes, PASS < 1ms.
- `tests/spa_deep_crawl_live.rs` (novo, `#[ignore]`): wiremock SPA
  com `history.pushState` + `fetch('/api/items')`. Assertivas:
  `is_spa=true`, `/dashboard` em runtime_routes, `/api/items` em
  network_endpoints e captured_urls, `SnapshotRuntimeRoutes` +
  `SnapshotNetworkEndpoints` em `list_artifacts`. PASS ~3s com
  system Chrome.

## Gates verdes

- `cargo build --all-features` ✓
- `cargo build --no-default-features --features cli,sqlite` ✓
- `cargo clippy --all-features --all-targets -- -D warnings` ✓
- `cargo test --all-features` (non-ignored) ✓ — 230+ testes
- `cargo test --all-features --test live_news_navigation -- --ignored` ✓ (~33s)
- `cargo test --all-features --test spa_scriptspec_live -- --ignored` ✓ (~4s)
- `cargo test --all-features --test spa_deep_crawl_live -- --ignored` ✓ (~3s)
- Patches Chrome 149 em `src/render/chrome/handler/{frame,target}.rs` intocados
- Licenças em `src/render/LICENSES/` preservadas
- Sem commits

## Findings

- Chrome 149 não executa `Page.addScriptToEvaluateOnNewDocument`
  para o PRIMEIRO documento navegado quando o script é registrado
  DEPOIS do `Target.createTarget` mas ANTES do `Page.navigate`.
  Stealth shim tem esse mesmo gap? — parece funcionar porque o
  shim redefine `navigator.*` e é idempotente, mas pode estar
  silenciosamente faltando no primeiro load também. Investigar
  numa próxima fase (fora de escopo Fase 3).
- Dupla-instalação (addScriptToEvaluateOnNewDocument + reinjeção
  post-nav) resolve pro observer — o guard
  `__crawlex_observer_installed__` evita binding duplo.
- IndexedDB collector depende de `indexedDB.databases()` — método
  que existe em Chrome ≥71 mas não em Firefox; gracefully retorna
  `[]`.
- Cache Storage collector bounded a 500 keys/cache pra não explodir
  payload em SWs offline-first.
