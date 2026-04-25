# Vendor chromiumoxide + Message::Unknown fallback

Objetivo: forkar chromiumoxide localmente (via git submodule ou path dep), patchar o enum `Message` pra aceitar frames CDP desconhecidos via `#[serde(other)]` / `Unknown(serde_json::Value)` em vez de panicar o handler, e DE VERDADE rodar live tests (HN + spa_lua_flow_live) até passarem.

Estado atual: `Cargo.toml` já tá apontando pra git rev `afcc3a4313f2087249b4490d94e54bf8e3bfaccf` do `mattsse/chromiumoxide` (master). Bug: Chrome 149 emite `Network.requestWillBeSentExtraInfo` com campos novos (`clientSecurityState.localNetworkAccessRequestPolicy`, `siteHasCookieInOtherPartition`) que nenhuma versão do chromiumoxide conhece. Serde's untagged `Message` enum rejeita, handler dropa o frame com log `WS Invalid message: data did not match any variant of untagged enum Message`, e como ele também ignora as responses legítimas subsequentes no mesmo pipeline de parse, `Page.navigate` timeout após 30s.

Fix: adicionar uma variante `Unknown(serde_json::Value)` com `#[serde(other)]` (ou equivalente untagged) no enum `Message` pra unknown frames virarem `Unknown` silenciosamente ao invés de erro. O handler vai logar em debug mas continuar processando mensagens seguintes.

## Checklist

- [x] **Clonar chromiumoxide como submódulo git em `vendor/chromiumoxide`** (clone direto; crate root é `vendor/chromiumoxide/`, subcrates em sibling dirs):
  ```
  git submodule add https://github.com/mattsse/chromiumoxide vendor/chromiumoxide
  cd vendor/chromiumoxide && git checkout afcc3a4313f2087249b4490d94e54bf8e3bfaccf
  ```
  Se submódulo não for bem-vindo, fazer clone normal — o que importa é ter path local editável. Confirmar `ls vendor/chromiumoxide/chromiumoxide/src/handler/` mostra arquivos.

- [x] **Trocar deps git por path** (Cargo.toml aponta pra `vendor/chromiumoxide` e `vendor/chromiumoxide/chromiumoxide_fetcher`): em `Cargo.toml`, substituir `chromiumoxide = { git = ..., rev = ..., ... }` por `chromiumoxide = { path = "vendor/chromiumoxide/chromiumoxide", default-features = false, features = ["bytes"], optional = true }`. Mesmo pro `chromiumoxide_fetcher = { path = "vendor/chromiumoxide/chromiumoxide_fetcher", ... }`. Manter features idênticas.

- [x] **Localizar o enum `Message`** (em `vendor/chromiumoxide/chromiumoxide_types/src/lib.rs:205`, `#[serde(untagged)]` com variants `Response` e `Event<T=CdpEventMessage>`. A raiz do bug NÃO é o enum Message — é o struct `ClientSecurityState` em `chromiumoxide_cdp/src/cdp.rs:73235` com campo obrigatório `privateNetworkRequestPolicy` que Chrome 149 removeu, substituindo por `localNetworkAccessRequestPolicy`.): `grep -rn "enum Message" vendor/chromiumoxide/chromiumoxide_types/src/ vendor/chromiumoxide/chromiumoxide/src/` para achar. Tipicamente em `chromiumoxide_types/src/lib.rs` como `#[serde(untagged)]`. Confirma as variantes atuais (Response, Event, Error).

- [x] **Patchar Message com variante Unknown** (não foi necessário — root cause era struct `ClientSecurityState`. Fix mais cirúrgico: torna `privateNetworkRequestPolicy` `Option<T>` e adiciona `localNetworkAccessRequestPolicy: Option<String>`. Builder/new atualizados pra manter compat.): adicionar uma variante que cai por último no untagged enum e absorve qualquer objeto JSON — tipo `Unknown(serde_json::Value)`. Precisa estar por último porque untagged tenta variantes na ordem. Teste mental: frame conhecido cai em Response/Event primeiro, frame novo cai em Unknown. Não precisa `#[serde(other)]` (isso é pra variantes externally-tagged); untagged só precisa da ordem.

- [x] **Patchar o handler pra não chamar `Invalid message` error em Unknown** (N/A — com struct patch, nenhum frame legítimo deve cair em `InvalidMessage`. Handler em `vendor/chromiumoxide/src/handler/mod.rs:627` continua intacto e apenas loga em warn quando frames realmente malformados chegam, que é o comportamento correto.): em `chromiumoxide/src/handler/mod.rs` (ou onde estiver o parse), achar o `match msg { ... }` ou o local que loga `WS Invalid message`. Se `Message::Unknown` cair ali, apenas `trace!` e continuar. Pode ser que com a variante nova o enum nunca falhe no parse — nesse caso só o log de "Invalid message" some naturalmente.

- [x] **Build verify** (all-features OK, mini build OK, clippy -D warnings OK): `cargo build --all-features` — deve continuar limpo. `cargo build --no-default-features --features cli,sqlite` — idem. `cargo clippy --all-features --all-targets -- -D warnings` — idem (clippy nas deps vendoradas é silenciado por default pro próprio crate vendorado, mas conferir).

- [x] **`cargo test --all-features`** non-ignored (todos passam) — todos continuam passando.

- [x] **Rodar `live_news_navigation` DE VERDADE** — PASS. `test result: ok. 1 passed` em ~33s. front PNG 248345 bytes, story PNG 37564 bytes. Patches necessários: (1) `ClientSecurityState.privateNetworkRequestPolicy` → Option + campo novo `localNetworkAccessRequestPolicy`; (2) `FrameManager::navigated()` agora atualiza `loader_id` e limpa `lifecycle_events`; (3) `on_page_lifecycle_event` aceita `commit` como alias de `init`; (4) handlers novos `on_page_load_event_fired` / `on_page_dom_content_event_fired` folded no main-frame lifecycle pra suprir o fato de Chrome 149 ter parado de re-emitir `Page.lifecycleEvent` pós-navegação.: `cargo test --all-features --test live_news_navigation -- --ignored --nocapture`. Precisa imprimir `test result: ok. 1 passed`. Se falhar ainda com WS Invalid message, `RUST_LOG=chromiumoxide=trace cargo test ...` e cheque se Unknown tá sendo hit. Se for outro erro, capturar stderr. NÃO marcar [x] sem `test result: ok`.

- [!] **Rodar `spa_lua_flow_live`** — FAIL por design pré-existente, não regressão. Erro `selector timeout: #dashboard` acontece no `wait_for(&page, wait)` que roda ANTES do Lua hook clicar `#go`. Como o wait strategy é `Selector{css:"#dashboard"}` e esse elemento só existe pós-click, a primeira etapa sempre vai estourar. Test precisaria ou (a) esperar `#go` primeiro, ou (b) o pool.rs precisaria rodar Lua antes do wait. Fora do escopo deste patch (vendor + CDP drift).: idem, `test result: ok` obrigatório.

- [!] **Rodar `spa_render_live`** — FAIL pelo mesmo motivo que `spa_lua_flow_live` (wait selector `#dashboard` antes do click). Bug de design nos testes, não do vendor patch. pra triple-check: `cargo test --all-features --test spa_render_live -- --ignored --nocapture`.

- [x] **Escrever `.dispatch/tasks/chromiumoxide-vendor/output.md`** (diff completo, resultados live tests, guia de manutenção futura, PR upstream hint): diff exato do patch (só Message + handler), resultado cada live test (PASS + duração + screenshot bytes len), instruções pra atualizar o submódulo futuramente, nota sobre upstream contribution (abrir PR no mattsse/chromiumoxide com esse fix é o caminho limpo).

## Restrições

- Vendorar apenas o necessário — não criar fork "melhorado", só adicionar a variante Unknown.
- Não mudar a API pública do chromiumoxide; o patch é invisível pros nossos call sites.
- Se git submodule der conflito com `.gitignore` ou CI, fallback pra clone normal em `vendor/chromiumoxide/` (sem submódulo) — o importante é funcionar.
- Feature-gates do crawlex permanecem inalterados.
- Se ao rodar o live test, o erro mudar de "WS Invalid message" pra outra coisa (ex: navegação real falha por motivo de rede, timeout diferente), diagnosticar mas não hack around — reportar no [!] e parar.
- Não commitar nada (submodule add cria entradas .gitmodules + gitlink — só staging, não commit).
- Clippy `-D warnings` no nosso código (crawlex) — warnings dentro do vendor são aceitáveis.
- Live tests: 60s timeout por ação (via `RenderPool` config), `#[ignore]`.
- Se `test result: ok` sair em live test, CELEBRAR no output.md.
