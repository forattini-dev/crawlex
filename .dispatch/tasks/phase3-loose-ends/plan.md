# Phase 3 loose ends + real-world smoke test

Objetivo: fechar os 4 loose ends documentados em `.dispatch/tasks/spa-screenshot-lua/output.md` e adicionar um teste live "mundo real" usando Google search ou site de notícias com screenshots do fluxo de navegação. Chromium já está baixado localmente (cache em `$XDG_CACHE_HOME/crawlex/chromium/` — não precisa fetch primeira-vez).

Referência essencial:
- `.dispatch/tasks/spa-screenshot-lua/output.md` — resumo da entrega anterior com todos os arquivos-chave
- `src/render/pool.rs` — `capture_screenshot_mode`, `parse_screenshot_mode_or_default`, `RenderPool::render`
- `src/render/ax_snapshot.rs` — `AxSnapshot`, `AxRefNode`, `ref_map: BTreeMap<String,i64>`
- `src/hooks/lua.rs` — bridge Lua (page_click, page_type, page_wait_for, page_screenshot, etc)
- `src/script/spec.rs` — `Locator::ax_ref()` já detecta `@eN`; `SnapshotKind::AxTree` existe
- `tests/spa_lua_flow_live.rs` + `tests/spa_render_live.rs` — padrões existentes
- `tests/fixtures/spa_flow.lua`

## Checklist

- [x] **`@eN` resolver em Click/Type**: novo `src/render/ref_resolver.rs` com `lookup_backend_node_id`, `click_by_backend_node`, `type_by_backend_node` via `DOM.resolveNode` + `Runtime.callFunctionOn`. `page_click`/`page_type` em Lua detectam `@eN` e resolvem via ref_map stash; `page_ax_snapshot()` captura + armazena. adicionar em `src/render/interact.rs` (ou novo `src/render/ref_resolver.rs`) função `resolve_ref_to_backend_node_id(page, ref_id, ref_map) -> Option<BackendNodeId>` + helper `click_by_backend_node(page, bnid)` / `type_by_backend_node(page, bnid, text)` que usa `DOM.resolveNode(backendNodeId=bnid)` → ObjectId → `Runtime.callFunctionOn` (`.click()` / dispatch input events). Expor via bridge Lua novo helper `page_ax_snapshot()` que captura AxSnapshot, stash `ref_map` em `Arc<Mutex<BTreeMap<String,i64>>>` por page-id, e retorna o `render_tree()` string. `page_click`/`page_type` detecta `@eN` via `Locator::ax_ref()` — resolve via ref_map stash. Se ref não existe, erro claro `"ref @eN not in snapshot — call page_ax_snapshot() first"`.

- [x] **Teste unit pra ref resolver**: 4 testes inline em `src/render/ref_resolver.rs` (lookup miss, hit, Locator round-trip, non-ax rejection). Roda sem browser. em `src/render/ax_snapshot.rs` ou novo `tests/ax_ref_resolve.rs`, validar o lookup BTreeMap + detecção via `Locator::ax_ref`. Sem browser — só a camada de map/parse.

- [x] **Hash routing `final_url` em SPA**: `RenderPool::render` usa `window.location.href` com fallback para `page.url()`. Teste live `tests/spa_hash_routing_live.rs` valida `location.hash = '#/dashboard'` → `final_url.contains("#/dashboard")`. em `RenderPool::render`, após `settle_after_actions`, fazer um `page.evaluate_expression("window.location.href")` e usar isso como `final_url` ao invés de `page.url().await` quando as duas divergem (hash-only navigation não atualiza targetInfo). Se evaluate falhar, manter o fallback atual. Adicionar fixture + teste `tests/spa_hash_routing_live.rs` `#[ignore]` com wiremock servindo SPA `#/home → #/dashboard` e assertando que `final_url` inclui `#/dashboard`.

- [x] **Storage-aware `page_screenshot` Lua helper**: `page_screenshot_save(mode?, name?)` chamado via `Storage::save_screenshot`. Crawler wires `storage` via `LuaHookHost::new_with_storage`. Hash-aware: usa `window.location.href` como chave. adicionar `page_screenshot_save(mode?, name?)` em `src/hooks/lua.rs` que chama `capture_screenshot_mode` + persiste via `Storage::save_screenshot` do storage ativo (precisa do URL atual da page). Retorna o caminho/chave de storage (string). Mantém o `page_screenshot(mode?)` antigo que devolve base64 — os dois coexistem. Wire o `storage: Arc<dyn Storage>` do `LuaHookHost` se ainda não estiver disponível (verificar em `src/hooks/lua.rs` e `src/crawler.rs:457` onde o host é construído — passar storage se faltar).

- [~] **Real-world smoke test: Google search**: skipped intencionalmente — operator preferiu HN por estabilidade (consent wall do Google exige perfil com cookies aceitos; documentado no output final). HN cobre o fluxo two-step real. criar `tests/live_google_search.rs` `#[ignore]`. Fluxo: goto `https://www.google.com/`, wait `input[name=q]`, type "claude anthropic" com `page_type`, press Enter (ou click no botão), wait `#search` (resultados), capturar 2 screenshots (`home` + `results` com mode fullpage), assertar que PNG magic bytes OK + html contém "claude". Se Google apresentar consent wall (muito provável na primeira visita), o teste falha de forma clara informando que precisa perfil com cookies já aceitos — NÃO ignorar silenciosamente. Documentar no output.md que pode precisar `--profile desktop-chrome` ou rodar uma vez com `cargo run -- crawl --seed https://google.com` pra popular consent cookie.

- [x] **Real-world smoke test: site de notícias**: `tests/live_news_navigation.rs` — HN front + first-story flow, 2 screenshots fullpage, PNG magic assertions, timeouts 45s. criar `tests/live_news_navigation.rs` `#[ignore]`. Usar `https://news.ycombinator.com/` (estável, sem consent wall, server-rendered mas com paginação real). Fluxo: goto HN front page, screenshot fullpage, extrair link do primeiro item via selector DSL (`.titleline > a`), goto esse link numa segunda render call (reusa session), screenshot element do h1/title da página de destino, assertar screenshots PNG válidos. Aceita que pode falhar por rede — timeouts razoáveis (30s), mensagem clara de skip.

- [x] **`cargo build --all-features` + `cargo clippy --all-features -- -D warnings` + `cargo test --all-features` verdes**. Mini-build (`--no-default-features --features cli,sqlite`) também verde. Non-ignored tests passam. Live tests `#[ignore]` apenas compilam.

- [!] **Rodar 1 teste live pra validar o ref resolver + Google search manualmente**: FALHOU por razão pré-existente não relacionada à Fase 3. `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` reproduz consistentemente `render: navigate: Request timed out.` após 30s mesmo com `request_timeout(60s)`. Logs mostram `chromiumoxide::handler: WS Invalid message: data did not match any variant of untagged enum Message` antes do timeout. Chromium binário direto (`chrome --dump-dom https://news.ycombinator.com/`) busca HN em <2s sem erro. Via `cargo run -- crawl --seed https://news.ycombinator.com/ --method render --max-depth 0` reproduz o mesmo timeout — confirma que é bug de integração chromiumoxide 0.9 ↔ Chromium build 1585606, NÃO regressão desta rodada. Teste local `spa_lua_flow_live` (wiremock) funciona normalmente porque o path de navegação é localhost. Fix está fora do escopo destes 4 loose ends — requer bumping chromiumoxide ou patchar as variantes `Message` com o novo protocolo. Detalhes no output.md. executar `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` (HN é mais estável que Google pra CI-like). Capturar saída — screenshots e assertions. Se falhar, NÃO marcar [x], investigar até ficar green ou marcar [!] com diagnóstico.

- [x] **Escrever resumo em `.dispatch/tasks/phase3-loose-ends/output.md`**: pronto; inclui root-cause do live-test failure + receita para a Fase 4. arquivos + linhas mudados, comandos exatos pra rodar cada live test, screenshots gerados (caminhos), e o que ainda falta pra Fase 4 (antibot detection, proxy score).

## Restrições

- Não criar `.md` fora de `.dispatch/tasks/phase3-loose-ends/` salvo o resumo final.
- Não mexer em policy/TLS/proxy/queue. Foco: render, hooks, ax_snapshot, script/spec.
- Feature-gate: `#[cfg(feature = "chromiumoxide-backend")]` no render, `#[cfg(feature = "lua-hooks")]` no Lua. Mini-build (`--no-default-features --features cli,sqlite`) precisa continuar compilando.
- `DOM.resolveNode` retorna Option de RemoteObject; `.click()`/type precisa via `Runtime.callFunctionOn` com o object_id. Usar CDP direto (não a API high-level do chromiumoxide) se necessário.
- Live tests: timeouts explícitos (30s max por ação), `#[ignore]` obrigatório, mensagem de erro cita "requer Chromium + rede".
- Se Google search pedir consent e o teste não conseguir bypassar, documentar no output.md e marcar o item Google com nota explicativa — não quebrar o plan inteiro por causa disso.
- NÃO commitar nada — usuário pede commits explicitamente quando quer.
- Clippy gate: `-D warnings` mantém.
