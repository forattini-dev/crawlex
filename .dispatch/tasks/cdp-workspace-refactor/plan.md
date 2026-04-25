# Modularizar chromiumoxide vendored → workspace interno `crawlex-cdp`

Objetivo: transformar `vendor/chromiumoxide/` de "cópia pura do upstream" em um conjunto de crates modulares do workspace do crawlex, com nomes próprios, estrutura integrada e atribuição legal correta. Depois do refactor, o código vendorado deve parecer parte natural do repo — não um fork óbvio.

**Estado atual**: `vendor/chromiumoxide/` tem subcrates `chromiumoxide`, `chromiumoxide_types`, `chromiumoxide_cdp`, `chromiumoxide_fetcher`, `chromiumoxide_pdl` + `examples/`, `tests/`, `README.md`, `CHANGELOG.md`. Cargo.toml raiz usa `path = "vendor/chromiumoxide/chromiumoxide"` etc. Imports em `src/`/`tests/` usam `use chromiumoxide::...`.

**Alvo**: cargo workspace com `crates/` layout:
```
crates/
  cdp-client/         (was chromiumoxide)       → package: crawlex-cdp
  cdp-types/          (was chromiumoxide_types) → package: crawlex-cdp-types
  cdp-protocol/       (was chromiumoxide_cdp)   → package: crawlex-cdp-protocol
  cdp-fetcher/        (was chromiumoxide_fetcher)→ package: crawlex-cdp-fetcher
NOTICE                (atribuição Apache-2.0/MIT ao projeto original)
```

Imports: `use chromiumoxide::` → `use crawlex_cdp::` em todo o `src/`, `tests/`, e internamente entre os crates.

## Checklist

- [x] **Converter raiz em cargo workspace**: no `Cargo.toml` raiz, adicionar seção:
  ```toml
  [workspace]
  members = [".", "crates/cdp-client", "crates/cdp-types", "crates/cdp-protocol", "crates/cdp-fetcher"]
  resolver = "2"
  ```
  Manter o `[package]` do crawlex no mesmo arquivo (root crate + workspace combinados). Testar `cargo metadata --format-version 1 --no-deps` produz JSON válido depois da edição.

- [x] **Mover + renomear os 4 crates**:
  - `vendor/chromiumoxide/chromiumoxide/` → `crates/cdp-client/`
  - `vendor/chromiumoxide/chromiumoxide_types/` → `crates/cdp-types/`
  - `vendor/chromiumoxide/chromiumoxide_cdp/` → `crates/cdp-protocol/`
  - `vendor/chromiumoxide/chromiumoxide_fetcher/` → `crates/cdp-fetcher/`
  - DROP: `vendor/chromiumoxide/chromiumoxide_pdl/` (gerador de tipos — roda manualmente offline; não precisa no workspace), `vendor/chromiumoxide/examples/`, `vendor/chromiumoxide/tests/`, `vendor/chromiumoxide/README.md`, `vendor/chromiumoxide/CHANGELOG.md`, `vendor/chromiumoxide/Cargo.toml` (root — era workspace deles, não serve).
  - Usar `git mv` se o diretório já está tracked; caso contrário `mv` puro.
  - Depois, `rm -rf vendor/chromiumoxide/` (se ficar vazio) OU `rm -rf vendor/` se não tiver mais nada.

- [x] **Renomear pacotes**:
  - `crates/cdp-client/Cargo.toml`: `name = "crawlex-cdp"`, `description = "Internal CDP client used by crawlex"`.
  - `crates/cdp-types/Cargo.toml`: `name = "crawlex-cdp-types"`.
  - `crates/cdp-protocol/Cargo.toml`: `name = "crawlex-cdp-protocol"`.
  - `crates/cdp-fetcher/Cargo.toml`: `name = "crawlex-cdp-fetcher"`.
  - Em cada Cargo.toml, ajustar deps internas: `chromiumoxide_types = { path = "../chromiumoxide_types" }` → `crawlex-cdp-types = { path = "../cdp-types" }`. Manter renaming explícito (`package = "..."`) só se algum lugar importa pelo nome antigo.
  - Remover metadados que denunciam origem: `authors`, `repository`, `homepage`, `keywords` do upstream. Deixar `license = "MIT OR Apache-2.0"` (legal) e adicionar `authors = ["Crawlex Contributors"]`.

- [x] **Renomear imports em TODOS os arquivos**:
  - Dentro dos 4 crates (arquivos `.rs` em `crates/cdp-*/src/**`): `use chromiumoxide_types::` → `use crawlex_cdp_types::`, `use chromiumoxide_cdp::` → `use crawlex_cdp_protocol::`, `use chromiumoxide::` → `use crawlex_cdp::` (dentro do cdp-client provavelmente é `crate::`, mas verificar).
  - No nosso código (`src/**/*.rs`, `tests/**/*.rs`): `use chromiumoxide::` → `use crawlex_cdp::`, `use chromiumoxide_cdp::` → `use crawlex_cdp_protocol::`, `use chromiumoxide_fetcher::` → `use crawlex_cdp_fetcher::`.
  - Em `Cargo.toml` (raiz): `chromiumoxide = { path = "vendor/..." }` → `crawlex-cdp = { path = "crates/cdp-client", ... }`. Mesmo para os outros. Manter `optional = true` e `features` intactos.
  - Usar `grep -rln chromiumoxide src/ tests/ crates/ Cargo.toml` iterativamente até zero hits (exceto comentários históricos que podem ficar ou não — prefira limpar).
  - **Importante**: o crate name `crawlex-cdp` traduz em código Rust para `crawlex_cdp` (hífen → underscore). Imports ficam `use crawlex_cdp::`.

- [x] **NOTICE file** (legal obligation Apache-2.0 §4): criar `NOTICE` na raiz:
  ```
  Crawlex — portions derived from "chromiumoxide" (https://github.com/mattsse/chromiumoxide)
  Original work © Matthias Seitz and contributors, dual-licensed MIT / Apache-2.0.
  Substantial modifications by the Crawlex project.

  LICENSE-APACHE and LICENSE-MIT in crates/cdp-client/ preserve the original terms.
  ```
  Manter os dois LICENSE files dentro de `crates/cdp-client/` (a raiz do subcrate que foi mais modificado).

- [x] **Gitignore** (no vendor/ entry — nothing to remove): se o `.gitignore` raiz tem `vendor/` excluído, remover essa entrada (agora está em `crates/`).

- [x] **Build verify** (passed, 1m01s): `cargo build --all-features` precisa continuar verde. Se algum import interno do vendor fez referência circular ou simbólica antiga, resolver.

- [x] **Mini build** (10s): `cargo build --no-default-features --features cli,sqlite`.

- [x] **Clippy** (`-p crawlex --all-features --all-targets -D warnings` green): `cargo clippy --all-features --all-targets -- -D warnings`. Warnings dentro dos crates `cdp-*` podem ficar `warn`; o gate `-D warnings` é só para o crate `crawlex` (nosso). Configurar `[workspace.lints]` se necessário pra separar.

- [x] **Testes non-ignored** (all test suites green): `cargo test --all-features` verde.

- [x] **Live test pra triple-check** (PASS 33.52s): `cargo test --all-features --test live_news_navigation -- --ignored --nocapture`. Precisa continuar PASS em ~30s. Se regredir, o problema é no renaming — revisar.

- [x] **Limpar referências textuais a "chromiumoxide"** (src/tests/docs limpos; histórico .dispatch/ mantido como atribuição) em comentários/docstrings do NOSSO código (`src/**`, `tests/**`, `README.md` se existir, `.dispatch/`). Não precisa obsessivo — tipo "CDP client" substitui bem. Mantém menções onde houver atribuição legítima (NOTICE, LICENSE files, output.md histórico).

- [x] **Escrever `.dispatch/tasks/cdp-workspace-refactor/output.md`**: diff resumido (antes/depois da árvore), lista de arquivos renomeados, zero hits de `chromiumoxide` em `src/`/`tests/`, tempo do live test, e nota final: "vendor agora é workspace interno, indistinguível de uma arquitetura própria".

## Restrições

- **Legal**: LICENSE-APACHE + LICENSE-MIT + NOTICE são obrigatórios (Apache-2.0 §4 exige preservação de notice e cópia da licença). NÃO remover.
- Package names passam a ser `crawlex-cdp*` (com hífen), diretórios `cdp-*` (sem prefixo crawlex — mais limpo em `crates/`).
- Funcionalidade idêntica — os patches do Chrome 149 continuam (ClientSecurityState, lifecycle handlers). Não desfazer patches.
- Não commitar.
- Não mudar API pública dos crates — rename de package, não de tipos/funções.
- Se `cargo build` não encontrar path antigo `vendor/chromiumoxide/...` em algum lugar que você perdeu, grep bruto antes de puxar histórico git.
- Workspace resolver = "2" (edition 2021/2024 compatível).
- Live test precisa imprimir `test result: ok. 1 passed` — não basta compilar.
- Se algo der muito errado no rename mass (ex: macro expansão falha com nomes novos), reportar `[!]` antes de desistir. Alternativa segura é manter package names antigos (chromiumoxide*) mas apenas mover pra `crates/cdp-*/` — só reestrutura de diretórios, menos blast. Só cair pra essa alternativa se o rename completo quebrar macros.
