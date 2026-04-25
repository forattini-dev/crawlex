# S.1 — `h2` pseudo-header order fork

Meta: patch local do `h2` crate pra emitir pseudo-header order `m,a,s,p` (Chrome) em vez de `m,s,a,p` (atual). Unlocks Akamai H2 fingerprint match.

## Entrega

- [ ] Clone `h2` v0.4.13 (ou versão em uso) pra `vendor/h2/`
- [ ] Patch `vendor/h2/src/frame/headers.rs::Iter::next` — swap branches `scheme` ↔ `authority` (4 linhas)
- [ ] `[patch.crates-io]` em `Cargo.toml` raiz apontando pra `vendor/h2`
- [ ] `cargo build --all-features` limpo
- [ ] `cargo build --no-default-features --features cli,sqlite` limpo
- [ ] `cargo clippy --all-features --all-targets -- -D warnings`
- [ ] `cargo test --all-features` non-ignored
- [ ] `cargo test --all-features --test h2_fingerprint_live -- --ignored --nocapture` — assertion de pseudo-header order `:method, :authority, :scheme, :path` passa byte-exact
- [ ] `cargo test --all-features --test live_news_navigation -- --ignored` PASS ~33s
- [ ] Output `.dispatch/tasks/s1-h2-pseudo-header-fork/output.md`
- [ ] `.done` marker

## Restrições

- Não mexer em outras partes do `h2` — só a ordem pseudo-header
- Preserve MIT/Apache license do `h2` em `vendor/h2/LICENSE*`
- Sem commits
- Live HN sem regressão
- Patches Chrome 149 intocados
