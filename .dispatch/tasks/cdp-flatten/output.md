# cdp-flatten — output

## Árvore antes (src/)

```
src/
  cdp/
    mod.rs
    client/    (Browser, Page, handler/, browser/, ...)
    types/     (mod.rs — wire envelope)
    protocol/  (cdp.rs ~111k linhas + mod.rs + revision.rs)
    fetcher/   (mod.rs, fetcher/, runtime/zip/, version/, ...)
    LICENSES/  (APACHE, MIT, NOTICE)
  render/
    (outros módulos)
```

## Árvore depois (src/)

```
src/
  render/
    chrome/              ← era src/cdp/client/
    chrome_protocol/     ← era src/cdp/protocol/ (dir mantida — ver nota)
    chrome_wire.rs       ← era src/cdp/types/mod.rs
    chrome_fetcher/      ← era src/cdp/fetcher/ (dir mantida — ver nota)
    LICENSES/            ← era src/cdp/LICENSES/
    (outros módulos)
```

`src/cdp/` não existe mais. `ls src/ | grep cdp` retorna vazio.

## Decisões sobre diretórios vs arquivos únicos

- **`chrome_protocol/` ficou como diretório** (plano permite essa fallback).
  Razões: `cdp.rs` sozinho já tem ~111k linhas; `mod.rs` (o orquestrador do
  subcrate) importa `cdp::browser_protocol::...` e `revision::Revision`,
  tem impls de conversão cruzada, `impl Display` para `ExceptionDetails`/
  `StackTrace`, e o módulo `revision` separado. Achatar tudo num único
  `chrome_protocol.rs` exigiria dobra de `mod cdp { ... }` + `mod revision
  { ... }` inline, aumentando complexidade sem ganho — a árvore só
  carrega 3 arquivos (`cdp.rs`, `mod.rs`, `revision.rs`), já é bem enxuta.
- **`chrome_fetcher/` ficou como diretório**. Razões: tinha subárvore
  própria (`fetcher/`, `runtime/zip/`, `version/`) com ~14 arquivos.
  Achatar exigiria `mod xxx { ... }` inline para cada subdir, degradando
  leitura.
- **`chrome_wire.rs` virou arquivo único** (era um só `mod.rs` de 335
  linhas — trivial).
- **`chrome/` ficou como diretório** (é o cliente, tem handler/ e
  browser/ subdirs — plano já previa).

## Imports

Zero `crate::cdp` em `src/` ou `tests/`:

```
$ grep -rln 'crate::cdp' src/ tests/
(vazio)
```

Mapeamento aplicado:

- `crate::cdp::client::` → `crate::render::chrome::`
- `crate::cdp::protocol::` → `crate::render::chrome_protocol::`
- `crate::cdp::types::` → `crate::render::chrome_wire::`
- `crate::cdp::fetcher::` → `crate::render::chrome_fetcher::`
- `crate::cdp::cdp::` → `crate::render::chrome_protocol::cdp::`
- `crate::cdp::{page,browser,error,element,layout,conn,handler,js,...}::`
  → `crate::render::chrome::{...}::`
- `crate::cdp::Browser`/`Page` → `crate::render::chrome::Browser`/`Page`

Docstrings dentro de `render/chrome/page.rs` e `element.rs` também
foram reescritas (são `ignore`d mas consistência importa).

## Feature flag

Mantida como `cdp-backend` (decisão: rename é cosmético, evitar risco).
Toda declaração de módulo em `src/render/mod.rs` continua sob
`#[cfg(feature = "cdp-backend")]`.

## Checks (todos verdes)

- `cargo build --all-features` — OK (3m 48s cold)
- `cargo build --no-default-features --features cli,sqlite` — OK (5.5s)
- `cargo clippy --all-features --all-targets -- -D warnings` — OK (sem warnings)
- `cargo test --all-features` — OK (todos os `test result: ok`)
- `cargo test --all-features --test live_news_navigation -- --ignored --nocapture` —
  `test result: ok. 1 passed; 0 failed ... finished in 33.50s` ✓
  (front-page PNG 237447 bytes; story PNG 37360 bytes)

## Patches Chrome 149

Permanecem aplicados:
- `src/render/chrome_protocol/cdp.rs`: `ClientSecurityState` com campos
  `Option<...>` (init/handshake vs full).
- `src/render/chrome/handler/frame.rs` + `handler/target.rs`: lifecycle
  handlers adicionados.

Não foram tocados nesta operação.

## Licenças

`src/render/LICENSES/` contém APACHE, MIT, NOTICE (mesmos arquivos).
Conformidade legal preservada.

## Arquivos que ainda mencionam "cdp" como palavra

Esperado: `cdp` é o nome oficial do protocolo (Chrome DevTools
Protocol) e aparece em:
- `src/render/chrome_protocol/` (path do protocol gerado)
- `src/render/chrome/mod.rs` (`pub use crate::render::chrome_protocol as cdp;`)
- docstrings técnicas ("CDP protocol types", "the CDP client")
- alvo de tracing: `target: "crate::cdp::conn::raw_ws::..."` (log tag)
- feature flag `cdp-backend`
- `tests/render_pool_backend.rs` comentários

Nada disso é referência a `crate::cdp::` (path de módulo) — todos são
textos/docs.
