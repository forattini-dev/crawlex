# Bump chromiumoxide → git master (fix CDP protocol drift)

Objetivo: substituir `chromiumoxide = "0.9.1"` + `chromiumoxide_fetcher = "0.9.1"` por git deps pinnadas em `mattsse/chromiumoxide` master (rev `afcc3a4` de 2026-04-03) pra destravar live tests que falham com `WS Invalid message: data did not match any variant of untagged enum Message`. Depois rodar live tests reais (HN + spa_lua_flow_live) e provar que funcionam.

Contexto: `cargo test --all-features --test live_news_navigation -- --ignored` atualmente dá `render: navigate: Request timed out` após 30s. Root cause identificado pelo dispatch anterior: drift de CDP entre 0.9.1 e Chrome recente. Crates.io não tem versão mais nova; master tem commits pós-0.9.1 provavelmente com fixes.

## Checklist

- [x] **Bump Cargo.toml**: trocar linhas 100-101 em `Cargo.toml`:
  ```
  chromiumoxide = { git = "https://github.com/mattsse/chromiumoxide", rev = "afcc3a4313f2087249b4490d94e54bf8e3bfaccf", default-features = false, features = ["bytes"], optional = true }
  chromiumoxide_fetcher = { git = "https://github.com/mattsse/chromiumoxide", rev = "afcc3a4313f2087249b4490d94e54bf8e3bfaccf", default-features = false, features = ["rustls", "zip8"], optional = true }
  ```
  Manter features, optional, e default-features idênticos ao atual. Confirmar que a rev é o último HEAD de `main` — se commit não existir mais (force push), usar o penúltimo SHA visível.

- [x] **`cargo build --all-features`**: no API breakage; clean build in 1m 01s. esperar falhas de API. Catalogar cada erro com arquivo+linha. Provável: variantes CDP renomeadas, novos campos obrigatórios em params builder, mudança de `BackendNodeId::inner()`, `AxNode` fields, etc. Fix cada erro item a item — mantendo comportamento; não refatorar.

- [x] **`cargo build --no-default-features --features cli,sqlite`**: clean. mini build precisa continuar verde (não deve tocar render, mas validar).

- [x] **`cargo clippy --all-features --all-targets -- -D warnings`**: clean. resolver qualquer lint novo introduzido pela nova API. Não demotar warnings.

- [x] **`cargo test --all-features`**: all non-ignored green. (todos non-ignored): tudo que passava antes precisa continuar passando. Se algum unit test de `ax_snapshot`/`ref_resolver`/`pool` quebrar por API shift, ajustar teste — não comportamento.

- [!] **Rodar live test real**: Master (afcc3a4) STILL has the same WS Invalid message bug. Live test fails identically: `render: navigate: Request timed out` after 30s. Stopped per plan restriction (não tentar hack local). See details below.

  Root cause identified with RUST_LOG=chromiumoxide=trace: Chrome 149 emits `Network.requestWillBeSentExtraInfo` events that include a new field `clientSecurityState.localNetworkAccessRequestPolicy: "PermissionBlock"` (and `siteHasCookieInOtherPartition`). The CDP bindings in master still don't know those fields, so serde's untagged enum fails to match ANY variant — the event is dropped with `WS Invalid message: data did not match any variant of untagged enum Message`. Once enough of those fire during page load, the Page.navigate command never sees its matching response/lifecycle events and times out.

  Sample failing raw msg (captured with `RUST_LOG=chromiumoxide=trace`):
  ```
  Failed to parse raw WS message ... msg="{\"method\":\"Network.requestWillBeSentExtraInfo\",\"params\":{\"requestId\":\"2075836.2\",\"associatedCookies\":[],\"headers\":{...},\"connectTiming\":{\"requestTime\":617712.619625},\"clientSecurityState\":{\"initiatorIsSecureContext\":true,\"initiatorIPAddressSpace\":\"Public\",\"localNetworkAccessRequestPolicy\":\"PermissionBlock\"},\"siteHasCookieInOtherPartition\":false},\"sessionId\":\"...\"}"
  ```

  Confirmed: grep for `localNetworkAccessRequestPolicy` in `~/.cargo/git/checkouts/chromiumoxide-*/afcc3a4/` returns zero hits. Master git log shows no PDL/protocol regen commits since 0.9.1 — most recent commits are Element Clone, zip8, dep bumps.

  Conclusion: Protocol drift has NOT been fixed upstream. Plan B (local fork with `#[serde(other)]` fallback or regenerated CDP bindings) is required. Per plan restrictions, not attempting local hack here — next dispatch. `cargo test --all-features --test live_news_navigation -- --ignored --nocapture`. Esperar sucesso desta vez (o bug WS Invalid message deve ter sumido). Se ainda falhar, capturar stderr completo + primeiros 50 linhas do handler trace. Não marcar [x] até ter saída `test result: ok`.

- [ ] **Rodar spa_lua_flow_live** também pra confirmar que nada regrediu no path wiremock/localhost: `cargo test --all-features --test spa_lua_flow_live -- --ignored --nocapture`.

- [ ] **Escrever resumo em `.dispatch/tasks/chromiumoxide-git/output.md`**: diff do Cargo.toml, lista de API changes encontradas (nome antigo → novo com arquivo:linha), resultado dos live tests (PASS + duração + screenshots confirmados), e se há lockfile considerations (Cargo.lock vai ter novo commit pin — `cargo update -p chromiumoxide` se precisar de upgrade futuro).

## Restrições

- Só mexer no que for necessário pra destravar o build + live tests. Não refatorar código correto.
- Não mudar features disponíveis (bytes, rustls, zip8).
- Não trocar pra `chromiumoxide_fork` sob nenhuma hipótese — é fork não-oficial.
- Se o master também tiver o mesmo bug (live test continua falhando com WS Invalid message), marcar `[!]` com o stderr completo e parar — vamos precisar abrir issue upstream ou partir pro plano B (fork local com `#[serde(other)]`).
- Não commitar.
- Clippy `-D warnings` mantido.
- Live tests precisam passar DE VERDADE — não marcar [x] sem ver "test result: ok" na stdout.
- Se aparecer flake de rede (timeout em HN), retry até 3 vezes antes de desistir; só então marcar `[!]` com nota de "possível flake, re-run".
