# Wave 1 — Runtime.Enable port (rebrowser-patches)

Meta: fechar P0-4 leak de verdade. Port do `runtime-enable-fix.patch` do rebrowser-patches pro handler Chrome 149 vendorizado. Owner SOLO: `src/render/chrome/handler/frame.rs` + relacionados.

## Contexto
brotector, DataDome medem Runtime.Enable presence via timing em main frame. Todo stealth fica com asterisco até fechar.

rebrowser-patches approach:
- NÃO chamar `Runtime.enable` no main page world
- Usar `Page.createIsolatedWorld` per-frame → `Runtime.runIfWaitingForDebugger`
- Stealth shim roda em isolated world, não main
- ExecutionContext tracking alternativo via `Page.frameAttached` + `Page.frameNavigated`

Patches Chrome 149 já dependem de ExecutionContextCreated/Destroyed — **reescrever esse tracking** pra não depender de Runtime.enable.

## Arquivos alvo
- `src/render/chrome/handler/frame.rs` (FrameManager init_commands)
- `src/render/chrome/page.rs` (enable_runtime API)
- `src/render/stealth.rs` (shim injection via isolated world)
- `src/render/pool.rs` (browser setup — remove Runtime.enable triggers)
- `tests/runtime_enable_audit.rs` (#[ignore])

## Checklist
- [ ] Auditar todos call sites `Runtime.enable` / `RuntimeEnableParams::default()` / `enable_runtime()` em todo repo
- [ ] Adaptar `references/rebrowser-patches/patches/runtime-enable-fix.patch` pro FrameManager
  - Remove `Runtime.enable` do `init_commands`
  - Add `Page.enable` + `Page.setLifecycleEventsEnabled{ enabled: true }` + setup frameAttached/frameNavigated listeners
  - Track frames via DOM.getDocument em vez de ExecutionContextCreated
- [ ] Stealth shim inject: trocar `Page.addScriptToEvaluateOnNewDocument` (que roda em main world) por:
  - `Page.createIsolatedWorld({ frameId, worldName: '__crawlex_stealth__' })` → guardar contextId
  - `Runtime.callFunctionOn({ executionContextId, functionDeclaration: shim_src })` per novo frame
- [ ] Per-frame isolation: cada frame ganha isolated world próprio ao attach; shim injetado nele
- [ ] Validar que evaluate_expression calls pro crawlex (selector resolver, ax snapshot, observer) continuam funcionais usando isolated world
- [ ] `tests/runtime_enable_audit.rs` `#[ignore]`: inject brotector-like probe (clock timing on Runtime evaluate) → assert absence of Runtime.Enable signals
- [ ] Live HN continua PASS ~33s (não pode regredir)
- [ ] Live SPA ScriptSpec + SPA Deep Crawl + Throughput continuam PASS
- [ ] Gates: build all + mini + clippy + test
- [ ] Output + `.done`

## Restrições SPECIAL
- Este worker MEXE em Chrome 149 handler patches (é o ponto). Preservar comportamento funcional. Se quebrar, reverter e marcar `[!]` com diagnóstico — NÃO submeter quebrado.
- SOLO run — outros workers não tocam em handler/
- Licenças preservadas
- Sem commits
- Se a adaptação do patch rebrowser for maior que esperado (>5h), marcar `[?]` e pedir IPC com análise

## Fallback
Se adaptação completa quebra muita coisa:
- Alternativa conservadora: manter Runtime.enable MAS criar isolated world pro shim (reduz leak — shim não vaza em main world)
- Marcar como "partial mitigation" no output
