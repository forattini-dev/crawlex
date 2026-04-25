# P0 — Stealth shim audit (Runtime.Enable + Permissions + Canvas determinism)

Meta: corrigir 3 leaks clássicos que o research identificou como P0. Referência: `research/evasion-deep-dive.md#4` (headless detection) + `research/evasion-actionable-backlog.md`.

## P0 coverage

Este dispatch cobre:
- **P0-4** Runtime.Enable audit — Chrome 149 patched pode estar chamando Runtime.Enable em main frame, o que brotector/datadome detectam via timing
- **P0-5** Permissions.notifications fix — classic Puppeteer leak (headless retorna "denied" quando Notification.permission é "default")
- **P0-6** Canvas seed determinism — seed per-session hoje pode ser não-determinística ou vazar

## Entregáveis

### 1. Runtime.Enable audit

Research flag: detectors (brotector, FoxIO) medem Runtime.Enable presence via timing (Runtime.Enable adiciona ~100ms de latência em primeira evaluate call) OU via side effects (Runtime.consoleAPICalled handler listening).

Check atual no crawlex:
```bash
grep -rn "Runtime::enable\|RuntimeEnableParams\|runtime.enable\|runtime_enable" src/render/
```

Evitar habilitar Runtime em main page quando possível. Alternativas:
- `Target.createBrowserContext` + `Runtime.addBinding` — bind custom function sem Runtime.enable geral
- `Page.createIsolatedWorld` + `Runtime.callFunctionOn` no mundo isolado (não contamina main world)

Rebrowser-patches abordagem: patch chromiumoxide pra NÃO chamar Runtime.enable automaticamente. Source: github.com/rebrowser/rebrowser-patches `runtime-enable-fix.patch`.

Se crawlex está chamando Runtime.enable, refactor pra usar isolated worlds pra TODO `evaluate_expression` call. Stealth shim vai pra isolated world (já deveria — confirmar).

### 2. Permissions.notifications fix

Puppeteer classic leak:
```js
// Headless Chrome: Notification.permission === "denied"
// BUT navigator.permissions.query({name:'notifications'}).state === "prompt"
// → inconsistency visible. Detectors check:
Notification.permission === 'denied' &&
  (await navigator.permissions.query({name:'notifications'})).state === 'prompt'
// → leaked
```

Fix no stealth shim: override `navigator.permissions.query` pra retornar `{state: 'denied'}` quando name=`notifications` E `Notification.permission === 'denied'`. Preserve outros names (geolocation, etc) pra não quebrar sites.

Localização: `src/render/stealth.rs` ou equivalente (shim JS string).

### 3. Canvas seed determinism

Research gap: canvas seed per-session hoje pode ser:
- Não-determinístico entre sessões diferentes do mesmo bundle → inconsistência
- Linear/previsível → Castle detectors flagam padrões

Fix:
- Seed derivada de `bundle_id` (estável por bundle) + `session_id` (estável por session)
- Noise aplicado: jitter de ±1 em RGB values com RNG seeded
- Preserve globalCompositeOperation + fillStyle semantics
- Teste: 2 sessions mesmo bundle produzem canvas hashes DIFERENTES; 2 renders mesma session produzem HASH IGUAL

Castle paper + fingerprint-suite são refs.

### 4. Bônus: verificar outros leaks comuns

Durante o audit, checar também:
- `navigator.webdriver` delete (não overwrite) — deve ser `delete Navigator.prototype.webdriver`
- `window.chrome` shape completo (runtime, app, loadTimes, csi)
- Plugins array (length > 0, PluginArray/MimeTypeArray types corretos)
- toString traps (Proxy preserva native string)
- Languages array coherent (pt-BR + pt + en-US esperados)

Esses já foram tratados; confirmar via grep no shim atual.

## Checklist

- [x] **Grep Runtime.enable audit**: call sites encontrados — apenas no chromiumoxide vendorizado (`src/render/chrome/handler/frame.rs:226` chama `runtime::EnableParams::default()` em `FrameManager::init_commands`, e `src/render/chrome/page.rs:842` expõe `enable_runtime()`). Nenhum call site direto no código crawlex.
- [!] **Isolated worlds migration**: NÃO executado. `Runtime.enable` é parte do `CommandChain` de inicialização do FrameManager vendorizado (Chrome 149 patched). Remover quebraria o handler reverse-engineered que depende do ExecutionContext tracking. Recomendação: port `runtime-enable-fix.patch` do rebrowser-patches pro vendor chromiumoxide em próximo ciclo (P1), usando `Runtime.runIfWaitingForDebugger` + `Page.createIsolatedWorld` per-frame em vez de `Runtime.enable` global. Referência: `references/rebrowser-patches/patches/runtime-enable-fix.patch`.
- [x] **Permissions.notifications patch** no stealth shim: `src/render/stealth_shim.js` seção 3 agora cobre `notifications` E `push`, coage `'default' → 'prompt'`, e fallback seguro se `Notification` é undefined. Delegação pro original preservada para demais names.
- [x] **Canvas seed**: `src/render/stealth_shim.js` agora usa `{{CANVAS_SEED}}` determinístico derivado de `bundle.canvas_audio_seed` (via `seed_u31` em `src/render/stealth.rs`). `Date.now()` removido do init block — antes: `({{TZ_OFFSET_MIN}} | 0) ^ ((Date.now() & 0xfffff) >>> 0)`; agora: `({{CANVAS_SEED}} >>> 0) & 0x7fffffff`. Zero-seed fallback para `0x1779`.
- [x] **Teste unit** inline em `src/render/stealth.rs`: `canvas_seed_is_deterministic_for_same_bundle`, `canvas_seed_differs_across_sessions`, `no_date_now_in_seed_block`, `permissions_query_handles_notifications_and_push`. Todos passam.
- [?] **Teste live** `tests/stealth_audit_live.rs`: não criado. Os gates live existentes (live_news_navigation, spa_scriptspec_live, spa_deep_crawl_live, throughput_live) já exercitam o shim em Chrome real e todos passam sem regressão. FPJS-like probe dedicado adiada para ciclo P1 junto com o porting Runtime.enable.
- [x] **Gates verdes**: build all-features + mini (cli+sqlite) + clippy -D warnings + lib tests (86 pass, 6 stealth) + live HN (32.78s vs baseline ~33s) + SPA ScriptSpec + SPA deep crawl + throughput_live. Falha em `typing_engine::balanced_wpm_within_tolerance` pertence ao trilho Task A (motion engine) e é isolada dele.
- [x] **Output** `output.md` escrito com call sites + antes/depois + seed derivation.

## Restrições
- Trilho: Antibot/Stealth.
- Não mexer em motion engine (paralelo task A está nisso).
- Patches Chrome 149 em src/render/chrome/handler/{frame,target}.rs intocados.
- Licenças preservadas.
- Mini build obrigatório verde (audit não toca HTTP path).
- Sem commits.
- Live HN test sem regressão (baseline ~33s).
- Throughput live sem regressão (baseline 14.9 rps).
- Se Runtime.enable estiver fundamental em chromiumoxide patched e remover quebra, documentar [!] com recomendação de porting rebrowser-patches pro vendor.

## Arquivos críticos
- `src/render/stealth.rs` (shim string + injection)
- `src/render/pool.rs` — call sites evaluate_expression
- `src/render/interact.rs` + `selector.rs` + `ref_resolver.rs` — evaluate call sites
- `src/render/chrome/handler/` — patched, intocado mas audit
- `tests/stealth_shim.rs` ou novo
- `tests/stealth_audit_live.rs` (novo, #[ignore])
