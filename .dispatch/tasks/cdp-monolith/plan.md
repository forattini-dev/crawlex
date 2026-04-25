# Colapsar workspace `crawlex-cdp*` → monolito em `src/cdp/`

Objetivo: desfazer o cargo workspace e absorver os 4 crates `crates/cdp-{client,types,protocol,fetcher}/` como módulos internos do crate `crawlex`, em `src/cdp/{client,types,protocol,fetcher}/`. Crate único, sem path deps, sem workspace members. Imports viram `use crate::cdp::client::...`. Preservar atribuição legal (NOTICE + LICENSEs num subdir discreto) — essa parte é obrigação legal Apache-2.0/MIT, não negociável.

**Estado atual**: workspace cargo em `Cargo.toml` raiz + 4 crates em `crates/cdp-*/`. Build verde, live test HN passa em ~33s.

**Alvo**:
```
src/
  cdp/
    mod.rs          (re-exporta: pub use client::*; pub mod types; pub mod protocol; pub mod fetcher;)
    client/         (era crates/cdp-client/src/)
    types/          (era crates/cdp-types/src/)
    protocol/       (era crates/cdp-protocol/src/)
    fetcher/        (era crates/cdp-fetcher/src/)
    LICENSES/
      APACHE        (ex-LICENSE-APACHE)
      MIT           (ex-LICENSE-MIT)
      NOTICE        (atribuição upstream condensada)
```

Crate raiz volta a ser single-crate. Dependências dos ex-crates (tokio, serde, futures, etc) migram pro `[dependencies]` raiz. Feature flag `crawlex-cdp-backend` pode virar apenas `cdp-backend` (interna).

## Checklist

- [x] **Consolidar `[dependencies]`**: ler `crates/cdp-{client,types,protocol,fetcher}/Cargo.toml`. Listar deps externas únicas (dedup). Adicionar ao `Cargo.toml` raiz se não estiverem lá, mantendo versões compatíveis. As que conflitarem: pegar a mais recente aceita pelos 4. Deps internas (`crawlex-cdp-types` etc) somem.

- [x] **Mover código-fonte pra `src/cdp/`**:
  - `crates/cdp-types/src/**` → `src/cdp/types/**`
  - `crates/cdp-protocol/src/**` → `src/cdp/protocol/**`
  - `crates/cdp-client/src/**` → `src/cdp/client/**`
  - `crates/cdp-fetcher/src/**` → `src/cdp/fetcher/**`
  - Preservar subestrutura interna de cada (submódulos, arquivos auxiliares, `handler/`, etc).
  - Criar `src/cdp/mod.rs` que declara os 4 módulos + re-exports pra manter path `use crate::cdp::...::Page` acessível onde precisar.

- [x] **Preservar licenças**: mover `crates/cdp-client/LICENSE-APACHE` → `src/cdp/LICENSES/APACHE`, `crates/cdp-client/LICENSE-MIT` → `src/cdp/LICENSES/MIT`. Criar `src/cdp/LICENSES/NOTICE` enxuto (2-4 linhas): atribuição ao upstream chromiumoxide + licença dual + nota "modificado pelo Crawlex". Remover `NOTICE` da raiz do repo (fica dentro de `src/cdp/LICENSES/` pra não denunciar origem em listagem `ls` raiz).

- [x] **Apagar `crates/` e workspace**: `rm -rf crates/`. Remover `[workspace]` seção do `Cargo.toml` raiz. Remover `members = [...]`. Apagar refs `crawlex-cdp*` do `[dependencies]` raiz (não há mais path deps).

- [x] **Rewire imports em TODOS os arquivos**:
  - Dentro do código CDP movido (src/cdp/**/*.rs):
    - `use crawlex_cdp_types::` → `use crate::cdp::types::`
    - `use crawlex_cdp_protocol::` → `use crate::cdp::protocol::`
    - `use crawlex_cdp::` → `use crate::cdp::client::` (ou `crate::cdp::` se o client for re-export no mod.rs)
    - `use crawlex_cdp_fetcher::` → `use crate::cdp::fetcher::`
    - Refs internas tipo `use crate::...` dentro do ex-client agora podem precisar ser `use crate::cdp::client::...` dependendo de como o re-export fica. Ver caso a caso.
  - No nosso código (`src/**/*.rs`, `tests/**/*.rs`, exceto `src/cdp/**`):
    - `use crawlex_cdp::` → `use crate::cdp::` (via re-export em `src/cdp/mod.rs`)
    - `use crawlex_cdp_protocol::` → `use crate::cdp::protocol::`
    - `use crawlex_cdp_fetcher::` → `use crate::cdp::fetcher::`
    - `use crawlex_cdp_types::` → `use crate::cdp::types::`
  - Em `Cargo.toml`: feature `crawlex-cdp-backend = [...]` → `cdp-backend = [...]` (atualizar todo lugar que referencia a feature no código: `#[cfg(feature = "crawlex-cdp-backend")]` → `#[cfg(feature = "cdp-backend")]`).
  - Grep tracking: `grep -rln 'crawlex[_-]cdp\|crawlex_cdp' src/ tests/ Cargo.toml` até zero hits.

- [x] **Render crate-level re-exports em `src/cdp/mod.rs`**: fazer o `client` ser "raiz" do submódulo pra calls `use crate::cdp::Page` funcionarem (era `use crawlex_cdp::Page`). Algo como:
  ```rust
  pub mod client;
  pub mod fetcher;
  pub mod protocol;
  pub mod types;
  pub use client::*;
  ```
  (Ajustar conforme API real.)

- [x] **Runtime strings**: grep por `crawlex_cdp_utility_world` em qualquer `.rs` → renomear pra algo neutro (`__utility_world__` já é genérico Chromium; ou `__ctx_world__`). Qualquer outra string que referencie `crawlex-cdp` vira string genérica.

- [x] **Build verify**: `cargo build --all-features` clean.

- [x] **Mini build**: `cargo build --no-default-features --features cli,sqlite` clean.

- [x] **Clippy**: `cargo clippy --all-features --all-targets -- -D warnings` clean. Se os arquivos CDP gerados (protocol) dispararem warnings que não rolou antes (porque eram pacote separado), colocar `#![allow(...)]` no `src/cdp/protocol/mod.rs` ou similar — código gerado não entra no gate.

- [x] **Test verde**: `cargo test --all-features` non-ignored.

- [x] **Live test HN**: `cargo test --all-features --test live_news_navigation -- --ignored --nocapture`. Precisa continuar `test result: ok. 1 passed` em ~30-35s.

- [x] **Cleanup final**:
  - `.gitignore` não deve listar `crates/` ou `vendor/`.
  - `Cargo.lock` regenerado pelo build.
  - `cargo metadata --format-version 1 --no-deps` retorna 1 package só (crawlex).
  - Zero hits de `chromiumoxide` ou `crawlex-cdp` em `src/`/`tests/`.

- [x] **Escrever `.dispatch/tasks/cdp-monolith/output.md`**: árvore antes/depois, confirmação que `cargo metadata` mostra um único package, tempo do live test, e nota legal: "LICENSE + NOTICE preservados em src/cdp/LICENSES/ como exigência Apache-2.0 §4".

## Restrições

- **LEGAL: nunca remover os dois LICENSE files nem o NOTICE.** Apache-2.0 §4 e MIT exigem preservação. Simplesmente *esconder* num subdir (`src/cdp/LICENSES/`) é o máximo aceitável — some do `ls` raiz sem violar licença.
- Code-gen do CDP protocol (era `chromiumoxide_cdp`) é arquivo gigante — NÃO tentar reformatar. Só garantir que compila no monolito.
- Patches do Chrome 149 (ClientSecurityState, lifecycle handlers em handler/frame.rs, handler/target.rs) devem continuar aplicados.
- Sem commits.
- Sem mudar API interna — só move + rename de paths.
- Build final não pode ter deps path apontando pra qualquer coisa ex-workspace.
- Live test precisa printar `test result: ok` DE VERDADE, não basta compilar.
- Feature-gate `cdp-backend` substitui `crawlex-cdp-backend` em TODOS os `#[cfg(feature = "...")]` sites. Zero escapada.
- Se o rename em massa quebrar macro expansion interna dos crates CDP (ex: macros que usam `$crate` em certos paths), caminho alternativo seguro é: manter tudo como está AGORA e só (a) apagar workspace members, (b) colapsar os 4 em um único subcrate `cdp` em `src/cdp/` sem renomear symbols externos — só ajustar paths de módulo. Reportar `[!]` com razão se atingir esse caso.
- Root do repo depois do refactor: zero hints textuais de "chromiumoxide" visíveis em `README.md`, `Cargo.toml`, `src/*.rs` (exceto `src/cdp/LICENSES/NOTICE`).
