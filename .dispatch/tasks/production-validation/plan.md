# Production Validation & Honest Gap Closure

Meta: fechar as 7 dúvidas legítimas do audit do super browser. Objetivo não é "mais features" — é **provar** o que já existe funciona em produção real + fechar os deferrals que ficaram mentindo (flaky markers, ponta frouxa).

## Princípio

Cada item tem:
- **Claim atual** (o que a gente diz que funciona)
- **Proof gap** (o que NÃO testamos)
- **Validação** (comando/teste que fecha a lacuna)
- **Critério de sucesso** (pass/fail mensurável)

Se a validação falhar, reportar honesto — sem "mark as ignore" pra encobrir.

---

## Bloco A — Validação real-world (prioritário)

### A.1 Real-world antibot validation suite

**Claim:** crawlex tem detection de 10 vendors (Cloudflare, Akamai, DataDome, PerimeterX, etc) + Stealth v3 + motion engine.
**Gap:** só testamos detection contra fixtures HTML. Zero prova de bypass em site real protegido.

**Entrega:**
- `tests/real_world_antibot_live.rs` `#[ignore]` — suite contra sites com antibot real:
  - `nowsecure.nl` — Cloudflare JS challenge known
  - `datadome.co/what-is-a-bot/` ou demo site DataDome
  - `bot.sannysoft.com` — FPJS-style detection + webdriver/chrome checks
  - `arh.antoinevastel.com/bots/areyouheadless` — headless-specific checks
  - `pixelscan.net/fingerprint-check` — canvas/WebGL/fonts coherence
  - `browserleaks.com/webrtc` + `/client-rects` + `/canvas` — fingerprint detail
  - `abrahamjuliot.github.io/creepjs/` — CreepJS FP score

Cada URL:
- Render via `crawlex crawl --method render --motion-profile balanced`
- Capturar HTML + screenshot final + challenge detection output
- Assert per-site criteria:
  - nowsecure.nl: `page.challenge.is_none()` OU contenteúdo target chega (sem `cf-chl-bypass`)
  - browserleaks: extract WebGL hash, canvas hash — assert diferentes de known-bot values
  - creepjs: extract score final — assert bot_score < 0.5 (ou outro threshold baseline)

**Critério:** report markdown `production-validation/real_world_report.md` com pass/fail + screenshot por site. Target razoável: **6 de 7 sites passam**. Sites que falham viram entrada priorizada no backlog.

### A.2 FingerprintJS bundle offline validation

**Claim:** stealth shim v3 passa FPJS checks.
**Gap:** nunca rodamos o bundle FPJS real.

**Entrega:**
- `tests/fpjs_compliance_live.rs` `#[ignore]` — serve localmente `fingerprint.js` bundle (via wiremock) + página que chama `FingerprintJS.load().get()` + extract result JSON
- Assert campos-chave:
  - `components.canvas.value.geometry` não match known Chrome-headless values
  - `components.audio.value` não é 0 (headless often is)
  - `components.platform.value` === `Linux x86_64` (ou do perfil)
  - `components.webGL.value.vendor` === `Google Inc.`

**Critério:** pelo menos 20 componentes no FPJS "humano" shape. Bot detection score < 3 em 5 runs consecutivos.

---

## Bloco B — Deferrals que estão mentindo

### B.1 Fix `spa_render_live` e `spa_lua_flow_live`

**Claim atual:** marked `#[ignore = "known-flaky wiremock+Chromium timing"]`.
**Problema:** isso é marker-de-fuga. Se wiremock+Chromium é flaky, quebra também em produção quando o site parece lento. Deve investigar.

**Entrega:**
- Root-cause **real** do timeout (trace CDP handler, ver onde wait_for_navigation trava)
- Fix (possivelmente add small retry wrapper, wait_for_dom_ready, ou patch chromiumoxide handler)
- Remover `#[ignore]` — testes rodam em CI

**Critério:** ambos testes passam 5/5 runs sucessivos.

### B.2 P0-4 Runtime.Enable leak — port rebrowser-patches

**Claim atual:** deferido P1 — "risco de quebrar handler patched".
**Problema:** é leak ativo. brotector + DataDome detectam Runtime.Enable via timing em main frame. Todo stealth fica asterisco até fechar.

**Entrega:**
- Ler `references/rebrowser-patches/patches/runtime-enable-fix.patch`
- Adaptar pro nosso `src/render/chrome/handler/frame.rs` — substituir `Runtime.enable` por `Page.createIsolatedWorld` per-frame + `Runtime.runIfWaitingForDebugger`
- Garantir ExecutionContext tracking continua funcional (nosso patch Chrome 149 depende disso)
- Stealth shim roda em isolated world
- Teste novo `tests/runtime_enable_audit.rs` `#[ignore]` — brotector-like probe via detected CDP timing

**Critério:** brotector `runtimeEnableLeak` returns false. Live HN + SPA tests continuam passando.

### B.3 P0-7b h2 pseudo-header fork

**Claim atual:** deferido — `h2` crate hard-codes order.
**Problema:** Akamai H2 fingerprint hash contém pseudo-header order. Sites usando Akamai Bot Manager detectam `m,s,a,p` vs Chrome `m,a,s,p` imediatamente.

**Entrega:**
- Fork local do `h2` crate (já vendored via git dep ou path dep) — apenas 4 linhas em `frame/headers.rs::Iter::next`: trocar branch order `authority` vs `scheme`
- `[patch.crates-io]` em workspace root Cargo.toml
- Teste `tests/h2_pseudo_header_order_live.rs` — local raw server assert `:method, :authority, :scheme, :path`

**Critério:** pseudo-header byte-exact Chrome. Build + clippy + tests verdes.

---

## Bloco C — Validação profunda (próprio código)

### C.1 Motion engine vs detector ML real

**Claim:** motion engine passa detectores ML.
**Gap:** tests são shape-only (variance, MT). Nenhum classifier real rodado.

**Entrega:**
- Usar detector open-source como referência: `bot-detector-js` OU implementar baseline simples (velocity entropy, inter-event timing variance, coordinate digit distribution)
- Teste `tests/motion_classifier.rs`: generate 1000 trajectories balanced profile → classifier score > 0.7 "human" (vs bot baseline < 0.3 com trajetória linear)
- Sample keystroke timing: WPM variance + hold/flight ratio match paper benchmarks (PMC8606350)

**Critério:** motion profile `human` score > 0.7 "human", `paranoid` > 0.85. `fast` pode ser baixo (é pra dev).

### C.2 Vendor telemetry classifier vs payload real

**Claim:** classifier reconhece Akamai sensor_data v1.7/v2, PerimeterX PX signals.
**Gap:** fixtures small. Samples reais ofuscados têm 50-200KB.

**Entrega:**
- Baixar samples reais (via manual browsing de site protegido + DevTools export — legal/ético pois é tráfego do próprio navegador)
- `tests/antibot_fixtures/vendor_payloads/`:
  - `akamai_sensor_v1.7_real.txt` (sample ~50KB)
  - `akamai_sbsd_v2_real.txt`
  - `perimeterx_collector.json` (com event IDs visíveis)
  - `datadome_report.txt`
  - `turnstile_result.json`
- Assert classifier identifica corretamente version + ≥80% das keys esperadas

**Critério:** classifier não regride; se falhar em payload real, document gap.

### C.3 Render path `record_outcome` audit

**Claim:** render path `record_outcome` completo após Fase 5.
**Gap:** atravessou 3 workers, pode ter pontas frouxas (timeouts, erros específicos, retry chains).

**Entrega:**
- Audit manual: `rg "record_outcome" src/crawler.rs src/render/` — listar todos call sites
- Tabela: cada branch de error/success no render path → qual `ProxyOutcome` dispara
- Fix qualquer branch que não chama record_outcome (silent score drift)
- Teste `tests/proxy_outcome_coverage.rs` non-ignored: simular 6 outcomes (Success, Timeout, ConnectFailed, ChallengeHit, Status, Reset) → verify counter no router incrementou

**Critério:** 100% das branches cobertas. Router score reflete realidade.

---

## Bloco D — Performance real (não wiremock)

### D.1 Throughput real-world benchmark

**Claim:** 14.9 rps com PagePool.
**Gap:** wiremock local = network ~0ms. Produção tem 100-500ms por request.

**Entrega:**
- `tests/throughput_real_live.rs` `#[ignore]` — 20 URLs reais (HN stories diversos, ou httpbin.org endpoints) rodando paralelo com `--motion-profile fast` e `balanced`
- Report em `production-validation/throughput_real.md`: rps, p50/p95/p99, memory peak, challenge rate
- Baseline esperado: fast 2-5 rps, balanced 1-2 rps em sites reais

**Critério:** numbers publicados honestamente. Se degradação for > 50% do alvo, investigar gargalo.

---

## Checklist execução

### Bloco A (real-world validation) — prioridade máxima
- [ ] A.1 real_world_antibot_live — 7 sites antibot reais, pass/fail report
- [ ] A.2 fpjs_compliance_live — FPJS bundle offline, score < 3

### Bloco B (fix deferrals)
- [ ] B.1 spa_render_live + spa_lua_flow_live root-cause + fix, remover #[ignore]
- [ ] B.2 Runtime.Enable port rebrowser-patches — stealth-grade full
- [ ] B.3 h2 pseudo-header fork — Akamai fingerprint byte-exact

### Bloco C (validation profunda)
- [ ] C.1 motion classifier ML-like scoring
- [ ] C.2 vendor telemetry com samples reais
- [ ] C.3 record_outcome audit + 100% branch coverage

### Bloco D (perf real)
- [ ] D.1 throughput real-world benchmark honest report

### Meta
- [ ] `production-validation/summary.md` — tabela final: claim → status real → evidence
- [ ] Atualizar `research/evasion-gap-analysis.md` com resultados do Bloco A

---

## Sequência recomendada

1. **A.1 primeiro** — mostra o que REALMENTE funciona em produção. Muda prioridade dos blocos seguintes baseado no resultado.
2. B.2 + B.3 em paralelo — ambos são fixes cirúrgicos em deferrals que escondiam leaks reais.
3. B.1 + C.3 em paralelo — testes broken + audit record_outcome.
4. A.2 + C.1 + C.2 em paralelo — validações profundas (stealth, motion, telemetry).
5. D.1 por último — baseline honesto antes de reportar números de produção.

## Restrições

- Real-world live tests `#[ignore]` — não quebram CI.
- Alguns sites (Cloudflare Enterprise, DataDome production) podem ser hostis a crawl repetido. **Rate-limit forte**: 1 request/site/run no test.
- Ético: não fazer > 3 requests na mesma sessão em sites alvo.
- Se site bloquear após 1 run, documentar "blocked on first attempt" é um resultado válido.
- Sem commits automáticos.
- Patches Chrome 149 podem ser tocados em B.2 — esse é o ponto da task. Preservar ExecutionContext tracking.
- Licenças preservadas.
- Mini build continua verde.
- Live HN baseline não regride.
- Se encontrar regressão dura, reportar honesto — sem #[ignore] pra encobrir.

## Critérios de pronto

Esta task fecha quando:
1. Tabela real-world com 7 sites + pass/fail justificado existe
2. Dois leaks P0 deferidos (Runtime.Enable + h2 pseudo-header) fechados ou documentados com blocker upstream claro
3. Testes SPA antes marcados `#[ignore = "flaky"]` rodam sem ignore
4. `record_outcome` audit completo com 100% branch coverage
5. Throughput real-world reportado honestamente (pode ser 2 rps, OK)
6. `production-validation/summary.md` publica "claim vs reality" por feature — nada escondido

## Saída final

`production-validation/summary.md` com estrutura:

```
| Feature | Claim | Evidence | Verdict |
|---------|-------|----------|---------|
| Cloudflare JS challenge bypass | works | 1 run nowsecure.nl screenshot | PASS / PARTIAL / FAIL |
| DataDome bypass | ? | demo site test | ... |
| Motion human-score | > 0.7 | motion_classifier.rs output | ... |
| FPJS score | < 3 | fpjs_compliance_live.rs | ... |
| Throughput produção | 2-5 rps | throughput_real.md | ... |
| Runtime.Enable leak | closed | runtime_enable_audit.rs | ... |
| h2 pseudo-header | Chrome-exact | h2_pseudo_order_live | ... |
```

Quem ler o summary sabe imediatamente **o que funciona de verdade**, **o que ainda não**, e **por quê**.
