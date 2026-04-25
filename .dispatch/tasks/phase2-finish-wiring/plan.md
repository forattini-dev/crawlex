# Fase 2 follow-up — Fechar wiring ScriptSpec runner

Worker anterior entregou `src/script/runner.rs` standalone com tests + events + ActionPolicy, mas 3 items ficaram `[!]`:
1. `RenderPool::render_with_script` não existe
2. CLI `--script-spec <path>` não existe
3. `Crawler` não passa script pro render path

Sem esses 3, o runner é API interna inacessível. Marco M1 (Stealth Browser Beta: "fluxo controlado sem Lua pra steps básicos") depende disso.

## Escopo: só os 3 items faltando

- [x] **`RenderPool::render_with_script`**: novo método paralelo ao `render()`. Assinatura:
  ```rust
  pub async fn render_with_script(
      &self,
      url: &Url,
      wait: &WaitStrategy,
      script: &ScriptSpec,
      proxy: Option<&Url>,
  ) -> Result<(RenderedPage, RunOutcome)>;
  ```
  Reusa fluxo do `render()` pra setup/navigate/initial-wait/session-state/challenge-detect. Entre settle inicial e screenshot final, roda `ScriptRunner::new(page, spec, plan, ...).run().await`. Se Lua host também presente, roda depois do script. Screenshot final continua rolando conforme `screenshot_mode`.

  Dica de blast radius: extrair o "core render loop" de `render()` pra helper privado `render_inner(page, url, wait, run_steps)` onde `run_steps` é closure `&Page -> Future<Output=Result<()>>`. Daí `render()` passa `|page| actions::execute_with_policy(...)` e `render_with_script()` passa `|page| runner.run()`. Evita duplicar 1900 LoC.

- [x] **CLI `--script-spec <path>`**: `src/cli/args.rs`:
  ```rust
  #[arg(long, value_name = "PATH", conflicts_with = "actions_file")]
  pub script_spec: Option<String>,
  ```
  `src/cli/mod.rs`: loader
  ```rust
  fn load_script_spec(path: &str) -> Result<ScriptSpec> {
      let data = std::fs::read(path).map_err(Error::Io)?;
      ScriptSpec::from_json(&data).map_err(|e| Error::Config(format!("script-spec: {e}")))
  }
  ```
  Popular `Config::script_spec: Option<ScriptSpec>`. Mutex com `--actions-file` via clap `conflicts_with`.

- [x] **Crawler integração**: `src/crawler.rs` no render branch de `process_job` — se `self.config.script_spec.is_some()` → chama `render_with_script`, senão `render`. Idêntico tratamento de `RenderedPage` (challenge detect, session state, etc). `RunOutcome` ignorado por enquanto (ou loggeado em debug) — captures/exports ficam pra fase 4 artifacts.

- [x] **Gates verdes obrigatórios**:
  - `cargo build --all-features`
  - `cargo build --no-default-features --features cli,sqlite` (script_spec em Config precisa ser `#[cfg(feature = "cdp-backend")]` OU wrapper serializable que compila sem backend — escolha menos invasiva)
  - `cargo clippy --all-features --all-targets -- -D warnings`
  - `cargo test --all-features` non-ignored
  - `cargo test --all-features --test live_news_navigation -- --ignored` PASS

- [x] **Live test** `tests/spa_scriptspec_live.rs` `#[ignore]` — ScriptSpec JSON pequeno que navega wiremock SPA, clica botão, tira screenshot element, snapshot AX tree. Preferir system Chrome (`/usr/bin/google-chrome`) como `live_news_navigation` faz.

- [x] **Output** `.dispatch/tasks/phase2-finish-wiring/output.md`.

## Restrições
- Só os 3 items. Não adicionar feature nova.
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build verde obrigatório.
- Live HN test sem regressão.
- Sem commits.
- Lua host continua funcionando — script runner roda ANTES do Lua hook on_after_load se ambos presentes.
