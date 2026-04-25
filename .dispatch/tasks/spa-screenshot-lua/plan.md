# SPA/PWA screenshot pós-JS + Lua script CLI — fechar Fase 3

Objetivo: um operador roda `crawlex crawl --seed <SPA>... --method render --screenshot --hook-script flow.lua` e recebe:
1) screenshot do DOM **depois** do JS + actions terem rodado (não viewport inicial),
2) `flow.lua` executa de verdade com acesso à `Page` e seus helpers.

Estado atual (verificar antes de mexer):
- `RenderPool::capture_screenshot_mode` já tem Viewport/FullPage/Element implementados (`src/render/pool.rs`).
- `RenderPool::render` chama `capture_screenshot(page)` que delega pra FullPage.
- `settle_after_actions` já reroda wait pós-action incluindo re-probe de Selector (`src/render/pool.rs`).
- `--hook-script` já é flag CLI (`src/cli/args.rs:114`).
- `lua-hooks` feature existe; `hooks::HostConfig`/`fire_with_page` já chama eventos no render path (`src/render/pool.rs` no bloco `#[cfg(feature = "lua-hooks")]`).
- Teste `tests/spa_render_live.rs` `#[ignore]` já valida pushState+selector reaparece.

Confirmar o que está quebrado/faltando e só então escrever código. Leia `src/hooks/lua.rs` e `src/render/pool.rs` antes de cada item.

## Checklist

- [x] **Auditar o fluxo screenshot atual no render path**: confirmado — `settle_after_actions` em pool.rs:1753 roda antes de `capture_screenshot` (pool.rs:1773); `save_screenshot` + `write_screenshot_output` em crawler.rs:633-635. Ordem correta, nada a corrigir.

- [x] **Expor `--screenshot-mode viewport|fullpage|element:<selector>` na CLI**: adicionado `screenshot_mode: Option<String>` em `OutputConfig`, flag `--screenshot-mode` em args.rs, parse/validate em cli/mod.rs via novo `parse_screenshot_mode`. Passagem via `Config` (sem mudança no trait). Default = FullPage.

- [x] **Wire o modo no `RenderPool::render`**: substituído por `capture_screenshot_mode(&page, parse_screenshot_mode_or_default(self.config.output.screenshot_mode.as_deref()))`. `Element` seletor inexistente já retornava `None` via `debug!` paths em `capture_screenshot_mode` (pool.rs:1435-1446 para QuerySelector/GetBoxModel).

- [x] **Verificar Lua script loading**: já wired — `cli/mod.rs:209-216` converte `c.hook_script` em `PathBuf`s e chama `crawler.set_lua_scripts(scripts)`; `Crawler::set_lua_scripts` em `crawler.rs:457` instancia `LuaHookHost::new(scripts)` que lê e executa cada script (lua.rs:69-76). Registra o host no `RenderPool` via `set_lua_host`. Nada a adicionar.

- [x] **Validar helpers Page expostos a Lua**: existentes — `page_click`, `page_type`, `page_wait`, `page_eval`, `page_scroll`. Adicionados — `page_wait_for` (alias Playwright-style), `page_content`, `page_goto`, `page_screenshot(mode?)` (retorna base64 PNG; usa `parse_screenshot_mode_or_default` + `RenderPool::capture_screenshot_mode`).

- [x] **Teste live SPA + Lua end-to-end** (`#[ignore]`): criado `tests/spa_lua_flow_live.rs` + `tests/fixtures/spa_flow.lua`. Cobre o stack: Lua `on_after_load` dirige `page_wait_for`/`page_click`, config usa `screenshot_mode = element:#dashboard`, assert PNG magic + html contém "Dashboard".

- [x] **Teste unit pra screenshot-mode parser na CLI** (não-ignore): `tests/screenshot_mode_parse.rs` — cobre viewport, fullpage (4 aliases), element com seletores simples/complexos, `element:` sem seletor (erro), modo desconhecido (erro lista formas válidas), string vazia (erro).

- [x] **`cargo build --all-features` + `cargo clippy --all-features -- -D warnings` + `cargo test --all-features` verdes**. Build no-default-features também verde. Todos os testes non-ignored passam; 3 testes live `#[ignore]` ficam fora do gate.

- [x] **Escrever resumo em `.dispatch/tasks/spa-screenshot-lua/output.md`**: feito — inclui arquivos + linhas, comando exato do teste live, exemplo CLI operator-facing, e lista do que ainda falta (final_url hash routing, @eN resolver, helper storage-aware para `page_screenshot`).

## Restrições

- Não criar arquivos `.md` de doc fora de `.dispatch/tasks/spa-screenshot-lua/`.
- Não mexer em coisas orthogonais (policy, TLS, proxy). Foco render + Lua.
- Se `ScreenshotCaptureMode::Element` com selector não resolver, retornar `None` e logar em `debug!` — não propagar erro fatal (screenshot é best-effort).
- `capture_screenshot_mode` já existe; não refazer.
- Feature-gate correto: tudo Lua fica em `#[cfg(feature = "lua-hooks")]`, tudo render em `#[cfg(feature = "chromiumoxide-backend")]`.
- Se precisar mudar `Renderer::render` trait, atualizar TODOS os impls + chamadas no `crawler.rs`.
- Commits só se o usuário pedir.
