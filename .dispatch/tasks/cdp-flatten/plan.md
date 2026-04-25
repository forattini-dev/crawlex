# Flatten `src/cdp/` dentro de `src/render/`

Objetivo: dissolver o subtree isolado `src/cdp/` dentro dos módulos existentes do crate. Nada mais fica em `cdp/` como casca apartada; tudo vira parte natural da árvore `src/render/` (ou raiz quando fizer sentido). O monolito passa a ter zero subtree "trazido de fora" visível.

**Estado atual**:
```
src/
  cdp/
    mod.rs
    client/    (Browser, Page, handler/, browser_protocol bindings)
    types/     (Method, Response, wire enums)
    protocol/  (code-gen gigantesco ~2M LOC — PDL traduzido)
    fetcher/   (download Chromium binary)
    LICENSES/  (APACHE, MIT, NOTICE)
```

**Alvo**:
```
src/
  render/
    chrome/             (era cdp/client/* — Browser, Page, handler/)
    chrome_protocol.rs  (era cdp/protocol — code-gen 2M linhas em 1 arquivo)
    chrome_wire.rs      (era cdp/types — Method/Response/wire envelope)
    chrome_fetcher.rs   (era cdp/fetcher — downloader + extract)
    LICENSES/           (era cdp/LICENSES — escondido mais fundo ainda)
    ... (resto do render/ existente)
```

O módulo `cdp` some da raiz `src/`. O nome `chrome` combina com o domínio do `render` (que já trata de Chrome). Protocol fica como `chrome_protocol.rs` na mesma pasta, não como subtree. Types + fetcher viram arquivos únicos no render/.

## Checklist

- [x] **Mover `src/cdp/client/` → `src/render/chrome/`** — `mv` direto (não estava tracked no git). Handler/browser subdirs preservados em `src/render/chrome/handler/` e `src/render/chrome/browser/`.

- [x] **Mover `src/cdp/protocol/` → `src/render/chrome_protocol/`** (dir, não arquivo único). Plano permitia essa fallback. Razões: `cdp.rs` sozinho é 111k linhas; `mod.rs` tem impls cross-referenciadas e módulo `revision` separado; achatar em arquivo único exigiria wrapping inline que piora leitura. `#![allow(warnings, clippy::all)]` adicionado no topo do `mod.rs`.

- [x] **Mover `src/cdp/types/mod.rs` → `src/render/chrome_wire.rs`** (arquivo único, 335 linhas).

- [x] **Mover `src/cdp/fetcher/` → `src/render/chrome_fetcher/`** (dir, não arquivo único). Motivo: subdirs `fetcher/`, `runtime/zip/`, `version/` com ~14 arquivos — achatar degradava legibilidade sem ganho.

- [x] **Mover `src/cdp/LICENSES/` → `src/render/LICENSES/`** — APACHE, MIT, NOTICE preservados.

- [x] **Apagar `src/cdp/`** — `rm mod.rs && rmdir src/cdp`. `ls src/ | grep cdp` vazio.

- [x] **Atualizar `src/lib.rs`** — removido `#[cfg(feature = "cdp-backend")] pub mod cdp;`. `render` continua declarado sob a feature.

- [x] **Atualizar `src/render/mod.rs`** — adicionados `pub mod chrome; pub mod chrome_protocol; pub mod chrome_wire; pub mod chrome_fetcher;` sob `#[cfg(feature = "cdp-backend")]`. Re-exports de alto nível (`Browser`, `Page`, etc.) já moram em `render/chrome/mod.rs`, então não precisaram ser duplicados em `render/mod.rs`.

- [x] **Rewire imports GLOBAL** — `grep -rln 'crate::cdp' src/ tests/` = 0. Mapeamento aplicado via `sed` em batch:
  - `crate::cdp::client::` → `crate::render::chrome::`
  - `crate::cdp::protocol::` → `crate::render::chrome_protocol::`
  - `crate::cdp::types::` → `crate::render::chrome_wire::`
  - `crate::cdp::fetcher::` → `crate::render::chrome_fetcher::`
  - `crate::cdp::cdp::` → `crate::render::chrome_protocol::cdp::`
  - `crate::cdp::{page,browser,error,element,layout,conn,handler,js,keys,listeners,cmd,auth,detection,utils,async_process}::` → `crate::render::chrome::{...}::`
  - `crate::cdp::Browser`/`Page` → `crate::render::chrome::Browser`/`Page`
  - Docstrings dentro do ex-client (`/// # use crate::cdp::...`) também reescritos.

- [x] **Feature flag** — mantida como `cdp-backend` (decisão: rename cosmético, evitar risco e churn em Cargo.toml/todos os `cfg`). Documentado na output.

- [x] **Build verify** — `cargo build --all-features` limpo (3m 48s cold).

- [x] **Mini build** — `cargo build --no-default-features --features cli,sqlite` limpo (5.5s).

- [x] **Clippy** — `cargo clippy --all-features --all-targets -- -D warnings` limpo. Os `#![allow(warnings, clippy::all)]` topo de `chrome_protocol/mod.rs` e `chrome/mod.rs` seguraram o code-gen.

- [x] **Test verde** — `cargo test --all-features` todos os "test result: ok".

- [x] **Live test HN** — `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` → `test result: ok. 1 passed; 0 failed ... finished in 33.50s`. Front-page PNG 237447 bytes, story PNG 37360 bytes. Sem regressão.

- [x] **Cleanup** — `ls src/` não mostra `cdp/`; `grep chromiumoxide\|crawlex-cdp src/` = 0; as menções remanescentes de "cdp" são nome técnico do protocolo (docstrings, feature flag, tracing targets), não paths de módulo.

- [x] **Output** — `.dispatch/tasks/cdp-flatten/output.md` com árvore antes/depois, confirmação `src/cdp/` deletado, live test time (33.50s).

## Restrições

- Single crate continua. Não mexer em `Cargo.toml` deps.
- Preservar LICENSES — agora em `src/render/LICENSES/`. Mesma obrigação legal.
- Patches Chrome 149 (ClientSecurityState Option fields, lifecycle handlers em handler/frame.rs/target.rs) permanecem intocados.
- Zero import `crate::cdp` no final.
- Build cargo inteiro não pode ficar mais lento que o estado atual — é só mover código, compilação idêntica.
- Live test precisa imprimir "test result: ok" DE VERDADE.
- Se protocol for muito complicado pra virar arquivo único (macros internas, múltiplos `mod.rs` interdependentes), aceitar `src/render/chrome_protocol/` como dir — documentar a decisão.
- Sem commit.
- Feature rename `cdp-backend` → `chrome-backend` é opcional (cosmético); se der trabalho, manter `cdp-backend` é aceitável.
- Zero menção textual `chromiumoxide` ou `crawlex-cdp` em src/ exceto dentro de LICENSES/NOTICE.
