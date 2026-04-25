# Wave 2 — Fechar pendências honestas da Wave 1

Meta: atacar os deferrals que ficaram `[!]`/`[?]`/`#[ignore]` na Wave 1 + fechar o ciclo com validação real-world.

## Pendências cobertas

### 1. OCSP `status_request` ext 5 (TLS)
Test `status_request_extension_is_present` em `tests/tls_extension_order.rs` marcado `#[ignore]`. Chrome emite `status_request` (ext 5) no ClientHello; nosso BoringSSL config não.

**Fix:** em `src/impersonate/tls.rs`, habilitar OCSP stapling request no builder:
- Para SSL_CTX: `SSL_CTX_set_tlsext_status_type(ctx, TLSEXT_STATUSTYPE_ocsp)` ou equivalente da boring crate
- Chrome 131+ emite tanto `status_request` (5) quanto `signed_certificate_timestamp` (18)
- Remover `#[ignore]` do test + confirmar ext 5 aparece

### 2. CLI wire-up dos scaffolds de infra (Wave 1 `[?]`)
Worker de infra-scaffold marcou 3 CLI flags `[?]` pra evitar conflito com outras waves:
- `--residential-provider <brightdata|oxylabs|iproyal|none>`
- `--captcha-solver <2captcha|anticaptcha|vlm|none>`
- `--mobile-profile android` (+ presets Pixel/Galaxy)

**Fix:** em `src/cli/args.rs` + `src/cli/mod.rs`, wire esses flags nas structs + load factories em startup (`ResidentialProviderKind::from_str` + `build_provider()`, `SolverKind::from_str` + `build_solver()`, `parse_mobile_profile`). Defaults continuam `none`. Handoff `CRAWLEX_HANDOFF` env var já wireada.

### 3. `Decision::HumanHandoff` enum variant (Wave 1 `[!]`)
Worker infra-scaffold criou `HandoffDecision` local porque não queria editar `src/policy/engine.rs` durante wave paralela. Agora sem concorrência, unificar.

**Fix:** adicionar `Decision::HumanHandoff { reason, vendor, url, screenshot_path }` em `src/policy/engine.rs`. Integrar em `decide_post_challenge` — quando `SessionAction::GiveUp` + `BypassLevel::Interactive` (ou equivalente), retornar `HumanHandoff` em vez de `GiveUp`. Remover `HandoffDecision` local em `src/render/handoff.rs` ou alias pra `Decision::HumanHandoff`.

### 4. IndexedDB transaction order audit (Wave 1 scope-out `#26`)
Worker crawl-pattern marcou `[~]` escopo fora. Observer vive em `src/render/spa_observer.rs`.

**Fix:** em `src/render/spa_observer.rs` inject JS pra validar: após N writes IDB, re-read e comparar ordem. Se divergir de Chromium canonical impl, emit `VendorTelemetryObserved` evento. Log-only (não bloqueia crawl).

### 5. render path `record_outcome` threading (pendência pré-existente)
Fase 5 throughput fechou metade: PagePool registra success/timeout pro ProxyRouter. Mas render_core não thread `duration_ms` + `challenge` outcome estruturado. `record_outcome(ChallengeHit)` só dispara via hook de challenge detection — nenhum Success/Timeout render path.

**Fix:** em `src/render/pool.rs::render_core`, medir wall-clock navigate→settle + passar pro crawler emitir `ProxyOutcome::Success { latency_ms }` ou `Timeout` quando render_core termina. Wire semelhante ao que HTTP path já faz em `src/crawler.rs`.

### 6. A.1 real-world retry (validação)
Plan anterior previa retry após ciclo stealth. Baseline: **2/8 pass + 4 unreachable + 1 partial + 1 fail**. Target: **6-7/8 pass**.

**Fix:** re-rodar `tests/real_world_antibot_live.rs` com as Wave 1 + S-tier fixes integrados. Gerar novo `production-validation/real_world_report_wave2.md` + update `summary.md` com row `A.1 retry`. Sites pra confirmar:
- `nowsecure.nl` — Cloudflare Turnstile (esperado fail sem solver ainda, mas agora Ext 5 + h2 fix podem não bastar; documentar)
- `antoinevastel.com` — esperado continuar pass
- `arh.antoinevastel.com/areyouheadless` — esperado pass, stealth audit fechado
- `bot.sannysoft.com` — Wave 1 permissions + Notification fixes devem destravar
- `abrahamjuliot.github.io/creepjs/` — com coherence matrix A.1 pode baixar score
- `browserleaks.com/canvas` — com canvas determinismo A.5 + seed derivado
- `browserleaks.com/webrtc` — S.3 fix deve bloquear local IP
- `pixelscan.net` — coherence A.1 + A.3 WebGL

### 7. h2 pseudo-header order (#8 backlog DEFERRED)
No backlog task list #8 marcado `DEFERRED`, mas S.1 já entregou via fork vendor. Confirmar + marcar completo.

## Checklist

- [ ] **OCSP ext 5** em `src/impersonate/tls.rs` — habilitar status_request no SslContextBuilder. Remover `#[ignore]` do test. Confirmar ext 5 no ClientHello parse.
- [ ] **CLI wire residential** em `args.rs`/`mod.rs` — flag + factory call. Default `none`.
- [ ] **CLI wire captcha-solver** — idem.
- [ ] **CLI wire mobile-profile** — flag Android preset pick.
- [ ] **`Decision::HumanHandoff`** em `policy/engine.rs` — variant + integration em `decide_post_challenge`.
- [ ] **Alias/remove `HandoffDecision`** em `src/render/handoff.rs` pra apontar pro Decision unificado.
- [ ] **IndexedDB audit observer** em `src/render/spa_observer.rs` — JS probe + event emit.
- [ ] **Render `record_outcome` timing** em `src/render/pool.rs` — wall-clock + wire crawler.rs passar pro `ProxyRouter`.
- [ ] **Retry A.1 real-world** — executar `cargo test --test real_world_antibot_live -- --ignored --nocapture`. Gerar `production-validation/real_world_report_wave2.md`. Updatear `summary.md` row A.1 retry.
- [ ] **Task #8 backlog** — marcar `completed` no TaskList (via TaskUpdate).
- [ ] **Gates verdes** obrigatórios:
  - `cargo build --all-features`
  - `cargo build --no-default-features --features cli,sqlite`
  - `cargo clippy --all-features --all-targets -- -D warnings`
  - `cargo test --all-features` (597 passed continua)
  - `cargo test --test live_news_navigation -- --ignored` PASS ~33s
  - `cargo test --test h2_fingerprint_live -- --ignored` PASS
- [ ] **Output** `.dispatch/tasks/wave2-closeout/output.md` com antes/depois real-world results + 10 items status.
- [ ] **`.done` marker** em `.dispatch/tasks/wave2-closeout/ipc/.done`.

## Restrições

- Patches Chrome 149 em `src/render/chrome/handler/{frame,target}.rs` intocados.
- Licenças `src/render/LICENSES/` preservadas.
- Feature flag `cdp-backend` respeitado.
- Mini build sempre verde.
- Live HN baseline ~33s sem regressão.
- Throughput 14.9 rps sem regressão.
- Sem commits.
- CAPTCHA solver externo continua fora de escopo (só wire-up scaffold + stubs).
- Real-world tests `#[ignore]`, rate-limit 1 req/site/run.
- Single dispatch — sem paralelo. Este worker é sequencial porque mexe em `args.rs`/`mod.rs`/`policy/engine.rs`/`pool.rs` — arquivos core que conflitam se paralelizar.

## Critério de sucesso

1. `tls_extension_order::status_request_extension_is_present` SEM `#[ignore]` e **passing**
2. CLI `--residential-provider`, `--captcha-solver`, `--mobile-profile` aceitos via clap parser
3. `Decision::HumanHandoff` variant existe + referenciável em policy paths
4. Real-world retry atingir ≥ 5/8 pass (melhoria ≥ 3 vs baseline 2/8)
5. `summary.md` atualizado honestamente com verdict por site
6. Gates finais todos verdes
