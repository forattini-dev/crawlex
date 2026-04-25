# P0 Stealth shim audit — output

## P0-4 Runtime.Enable audit

### Call sites (grep)
```
src/render/chrome/page.rs:842   self.execute(js_protocol::runtime::EnableParams::default())
src/render/chrome/handler/frame.rs:226   let enable_runtime = runtime::EnableParams::default();
src/render/chrome_protocol/cdp.rs:17133  pub const IDENTIFIER: &'static str = "Runtime.enable";
```

Nenhum call site direto em código crawlex próprio (pool.rs, interact.rs, selector.rs, ref_resolver.rs). Apenas no chromiumoxide vendorizado.

### Decisão: [!] DEFERRED — não mexido
`Runtime.enable` em `FrameManager::init_commands` é parte do CommandChain vendorizado (Chrome 149 patched). O handler em `src/render/chrome/handler/frame.rs` depende do rastreamento de `ExecutionContextCreated`/`Destroyed` que Runtime.enable produz — remover quebra o mapa `context_ids` (linha 213) e os `isolated_worlds` (linha 214), inviabilizando `evaluate_expression` inteiro.

Patches Chrome 149 em `src/render/chrome/handler/{frame,target}.rs` estão explicitamente intocados por restrição do plano.

### Recomendação próximo ciclo (P1)
Port do `runtime-enable-fix.patch` do projeto rebrowser-patches pro vendor chromiumoxide:
- Substituir `runtime::EnableParams::default()` global por `Page.createIsolatedWorld` per-frame
- Evaluates em worlds isolados via `Runtime.callFunctionOn` com `executionContextId` específico
- Mundo principal nunca "toca" em Runtime.enable → brotector stack counter / FoxIO timing probe zeram
- Reference: `references/rebrowser-patches/patches/runtime-enable-fix.patch`

Estimativa: 1-2 dias (vendor patch + regression suite).

## P0-5 Permissions.notifications fix — APPLIED

### Antes
```js
safe(() => {
  const q = navigator.permissions && navigator.permissions.query;
  if (q) {
    const orig = q.bind(navigator.permissions);
    navigator.permissions.query = (p) =>
      p && p.name === 'notifications'
        ? Promise.resolve({ state: Notification.permission, onchange: null })
        : orig(p);
  }
});
```

Problemas:
- Só cobria `notifications`, não `push` (mesma estrutura de leak).
- Repassava `Notification.permission === 'default'` direto, mas a Permissions API **nunca** retorna `'default'` — sempre coage para `'prompt'`. Isto é um tell detectável: `permissions.query({name:'notifications'}).state === 'default'` é impossível em Chrome real.
- Falhava hard se `Notification` fosse undefined (contextos não-seguros).

### Depois (`src/render/stealth_shim.js` seção 3)
```js
safe(() => {
  const q = navigator.permissions && navigator.permissions.query;
  if (q) {
    const orig = q.bind(navigator.permissions);
    const coerce = (s) => (s === 'default' ? 'prompt' : s);
    const leaky = { notifications: 1, push: 1 };
    navigator.permissions.query = function (p) {
      if (p && p.name && leaky[p.name]) {
        const np = (typeof Notification !== 'undefined' && Notification.permission)
          ? Notification.permission : 'prompt';
        return Promise.resolve({ state: coerce(np), onchange: null });
      }
      return orig(p);
    };
  }
});
```

- Cobre os dois names leaky conhecidos.
- Coage `'default' → 'prompt'` igual Chrome real.
- Demais names (geolocation, camera, mic, clipboard, ...) continuam delegando ao impl nativo.

### Probe Puppeteer-clássico
```js
Notification.permission === 'denied' &&
(await navigator.permissions.query({name:'notifications'})).state === 'prompt'
```
Antes: retornava `true` se headless entregasse `denied` via Notification + `prompt` via Permissions — leak.
Depois: ambos retornam o mesmo valor (denied/granted/prompt), estado é coerente.

## P0-6 Canvas seed determinism — APPLIED

### Antes (`src/render/stealth_shim.js:476`)
```js
window.__crawlex_seed__ = ({{TZ_OFFSET_MIN}} | 0) ^ ((Date.now() & 0xfffff) >>> 0);
```

Problemas:
- Dependia de `Date.now()` → **não determinístico entre page loads do mesmo session**. Duas chamadas a `canvas.toDataURL()` separadas por >10ms na mesma session podiam divergir se o seed era recalculado (ex: reload de frame).
- TZ_OFFSET_MIN como único vetor "estável" é ~24 valores possíveis — detectors conseguem bucket.
- FingerprintJS faz double-render equality check: render N1, render N2, comparar bytes; se divergir por causa do Date.now na segunda call → leak.

### Depois
Seed derivada determinísticamente do `IdentityBundle.canvas_audio_seed` (session_seed passado em `IdentityBundle::from_chromium(major, session_seed)`):

**`src/render/stealth.rs`**:
```rust
fn seed_u31(raw: u64) -> u32 {
    let mixed = raw ^ (raw >> 32);
    (mixed as u32) & 0x7fff_ffff
}
// ...
canvas_seed: seed_u31(bundle.canvas_audio_seed),
```

Novo placeholder `{{CANVAS_SEED}}` substituído em `apply()`.

**`src/render/stealth_shim.js`**:
```js
if (typeof window.__crawlex_seed__ !== 'number') {
  window.__crawlex_seed__ = ({{CANVAS_SEED}} >>> 0) & 0x7fffffff;
  if (window.__crawlex_seed__ === 0) window.__crawlex_seed__ = 0x1779;
}
```

Propriedades:
- **Determinístico intra-session**: mesma session_seed → mesmo shim string → mesmo seed literal em JS → mesmo hash de canvas em N renders consecutivos. FPJS double-render equality ✅.
- **Distinto inter-session**: cada session gera `session_seed` novo no pool (`pool.rs:660` `SystemTime::now().duration_since(UNIX_EPOCH).as_nanos()`) → `canvas_audio_seed` distinto → hash distinto. Não há canvas-hash reutilizado cross-session atacável ✅.
- **Sem Date.now leakage**: teste `no_date_now_in_seed_block` guarda regressão.
- **31-bit safe**: masking garante ops bitwise dentro do shim não caem em sign-bit (JS int bitwise é 32-bit signed).
- **Fallback**: zero-seed é uma patologia; cai pra `0x1779` (o valor histórico pré-dynamic) se mixer retornar 0.

## Bônus: leaks verificados já corrigidos
- `navigator.webdriver`: `delete` no prototype + getter retornando `undefined` ✓ (seção 0)
- `window.chrome`: shape completo com `runtime`, `app`, `loadTimes`, `csi` ✓ (seção 2)
- Plugins: 5 PDF entries, tipos `PluginArray`/`MimeTypeArray` via prototype chain, `Symbol.iterator` ✓ (seção 4)
- toString traps: Proxy em `Function.prototype.toString` com WeakSet de targets, devolve `function () { [native code] }` para hooks registrados ✓ (seção 13)
- Languages: `{{LANGUAGES_JSON}}` driven por bundle, coerente com locale ✓ (seção 1)

## Tests added

`src/render/stealth.rs::tests`:
- `canvas_seed_is_deterministic_for_same_bundle`
- `canvas_seed_differs_across_sessions`
- `no_date_now_in_seed_block`
- `permissions_query_handles_notifications_and_push`

Todos passam (6/6 stealth tests green).

## Gates

| Gate | Resultado |
|---|---|
| `cargo build --all-features` | ✅ |
| `cargo build --no-default-features --features cli,sqlite` | ✅ |
| `cargo clippy --all-features --all-targets -- -D warnings` | ✅ |
| `cargo test --all-features --lib` | ✅ 86/86 |
| `cargo test --all-features --test live_news_navigation -- --ignored` | ✅ 32.78s (baseline ~33s) |
| `cargo test --all-features --test spa_scriptspec_live -- --ignored` | ✅ |
| `cargo test --all-features --test spa_deep_crawl_live -- --ignored` | ✅ |
| `cargo test --all-features --test throughput_live -- --ignored` | ✅ |

Falha em `typing_engine::balanced_wpm_within_tolerance` pertence ao trilho paralelo Task A (motion engine) — fora do escopo deste dispatch.

## Arquivos tocados
- `src/render/stealth.rs` — adicionado `{{CANVAS_SEED}}`, `seed_u31()`, 4 testes novos
- `src/render/stealth_shim.js` — permissions seção 3, canvas seção 11

Nenhum commit criado (conforme plano). Licenças preservadas. Patches Chrome 149 intocados.
