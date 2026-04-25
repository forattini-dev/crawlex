# Evasion Backlog — actionable items priorizados

> Formato por item:
> - **Objetivo**: outcome observable
> - **Arquivo/módulo**: path provável no crawlex
> - **Esforço**: S (<1d), M (1–3d), L (3–10d), XL (>10d)
> - **Dependência**: âmbitos que precisam estar prontos
> - **Trilho roadmap**: antibot-stealth / browser-control / spa-pwa / artifacts / scale / network

Ordenado por prioridade P0 → P3. Itens marcados 🔴 são blockers em sites top-tier.

---

## Shipped (as of 2026-04)

- ✅ **P1.1 Scroll bursts** — `src/render/motion/scroll.rs:104` (`schedule_for_active_profile`, bell-curve deltas + Pareto inter-burst dwells); wired via `src/render/interact.rs:298` (`scroll_by`).
- ✅ **P1.3 Function.toString coverage** — `src/render/stealth_shim.js:617` (section 13: WeakSet + Proxy on `Function.prototype.toString`, installed at line 666); covers every registered override.
- ✅ **P1.4 Plugins Chrome PDF** — `src/render/stealth_shim.js:202` (section 4) ships 5 PDF plugin strings at lines 223–227.
- ✅ **P1.11 Accept-Encoding zstd** — `src/impersonate/mod.rs:836` (`chrome_http_headers_full`) emits `accept-encoding: gzip, deflate, br, zstd` at lines 912–915.

---

## P0 — Blockers em sites top-tier

### P0.1 🔴 Human motion engine (Fitts + WindMouse + OU jitter)
- **Objetivo**: `actions.move_to(x,y)` gera trajetória com duration Fitts, perfil de velocidade pico-meio, jitter OU estacionário ±1–3 px, sub-movements com overshoot; idle drift entre ações
- **Arquivo**: `src/render/interact.rs` (novo `src/render/motion.rs` dedicado)
- **Esforço**: L (3–5 dias com testes)
- **Dependência**: âmbito 1 + 9
- **Trilho**: antibot-stealth
- **Aceitação**: classifier básico (LinearRegression sobre curvature+velocity variance) classifica movimento como "humano" em >95% dos samples vs bot bezier puro em <10%

### P0.2 🔴 Event sequence integrity (mousemove→mouseover→click garantido)
- **Objetivo**: `click(selector)` NUNCA emite click sem preceder com trajectory em direção ao elemento. Implementar em `interact.rs::click` como composição forçada de move → hover → click.
- **Arquivo**: `src/render/interact.rs`, `src/render/actions.rs`
- **Esforço**: S (4h)
- **Dependência**: P0.1
- **Trilho**: antibot-stealth
- **Aceitação**: auditoria de todo caminho de `click`, `submit`, `tap` garante sequencia completa; test harness injeta listener em CDP Runtime.evaluate para validar.

### P0.3 🔴 Keystroke log-normal hold/flight + thinking pauses
- **Objetivo**: `type(text)` usa per-bigram LogNormal hold / LogLogistic flight, thinking pauses Pareto em espaços/pontos de pontuação, 1–2% typos com backspace opcional.
- **Arquivo**: `src/render/interact.rs` + novo `src/render/typing.rs`
- **Esforço**: M (2 dias)
- **Dependência**: âmbito 1.2
- **Trilho**: antibot-stealth
- **Aceitação**: distribution fit test (KS-test) contra `free-text keystroke` dataset público; typos ativáveis via config.

### P0.4 🔴 Validar/enforcar absent Runtime.Enable + addBinding-style context resolution
- **Objetivo**: confirmar que crawlex NÃO chama `Runtime.enable` em main world; se precisar executionContextId, usar `Runtime.addBinding` + `Page.addScriptToEvaluateOnNewDocument` pra expose binding → extrair context via callback. Senão, usar `Page.createIsolatedWorld`.
- **Arquivo**: `src/render/chrome_protocol/*` + `src/render/pool.rs`
- **Esforço**: M (2–3 dias com audit)
- **Dependência**: âmbito 4.2
- **Trilho**: antibot-stealth / browser-control
- **Aceitação**: brotector-like JS (stack/name lookup counter + debugger timing) corre in-page retorna score `webdriver≈0`. Verificar no brotector.js benchmark.
- **Fonte**: [rebrowser Runtime.Enable fix](https://rebrowser.net/blog/how-to-fix-runtime-enable-cdp-detection-of-puppeteer-playwright-and-other-automation-libraries-61740), [brotector](https://github.com/kaliiiiiiiiii/brotector)

### P0.5 🔴 Permissions.notifications override
- **Objetivo**: `navigator.permissions.query({name:'notifications'})` retorna `'prompt'` quando `Notification.permission === 'default'` (não `'denied'`).
- **Arquivo**: `src/render/stealth_shim.js`
- **Esforço**: S (1h)
- **Dependência**: âmbito 4.4
- **Trilho**: antibot-stealth
- **Aceitação**: check JS snippet returns `'prompt'`, matches real Chrome.

### P0.6 🔴 HTTP/2 fingerprint Akamai-exact (SETTINGS/WINDOW_UPDATE/pseudo-header)
- **Objetivo**: validar (ou implementar) que requests HTTP/2 do engine HTTP spoof mandam SETTINGS `1:65536;2:0;4:6291456;6:262144`, WINDOW_UPDATE 15663105 em stream 0, pseudo-header order `m,a,s,p`, SEM standalone PRIORITY frames, priority via HEADERS frame.
- **Arquivo**: `src/impersonate/tls.rs`, configuração `h2` crate ou BoringSSL bindings
- **Esforço**: M (2–3 dias com teste via browserleaks.com/http2 ou scrapfly)
- **Dependência**: âmbito 3.2
- **Trilho**: network/antibot-stealth
- **Aceitação**: FP capturado via tls-fingerprint service bate com Chrome 149 current.

### P0.7 🔴 Header order por request type (document vs xhr/fetch)
- **Objetivo**: headers HTTP/1 e pseudo-headers HTTP/2 emitidos em ordem Chrome-exact, diferenciada por `Sec-Fetch-Mode` (navigate vs cors vs no-cors).
- **Arquivo**: `src/impersonate/headers.rs`
- **Esforço**: M (2 dias)
- **Dependência**: âmbito 3.5
- **Trilho**: network/antibot-stealth
- **Aceitação**: dump de headers bate byte-a-byte com Chrome real (capturado via Wireshark).

### P0.8 🔴 Canvas/Audio seed determinismo per-visitorID+origin
- **Objetivo**: seed do canvas e audio noise NÃO muda per-call dentro da mesma session. Castle detection: render duas vezes e comparar hash → se diff, é noise. Seed = hash(visitorID + origin) e noise pós-calculado uma vez por session.
- **Arquivo**: `src/render/stealth_shim.js` (validar implementation)
- **Esforço**: S (4h)
- **Dependência**: âmbito 2.1
- **Trilho**: antibot-stealth
- **Fonte**: [Castle noise detection](https://blog.castle.io/detecting-noise-in-canvas-fingerprinting/)

### P0.9 🔴 Vendor-specific coverage enhancements
- **Objetivo**: ajustar heuristic detection em `src/antibot/` para cada vendor específico:
  - Cloudflare: detect Turnstile iframe (`challenges.cloudflare.com/cdn-cgi/challenge-platform/.../turnstile/if/`), `cf_clearance` cookie flow, `__cf_chl_jschl_tk__` query param.
  - Akamai: detect `_abck`, `bm_sz`, `ak_bmsc`, endpoint patterns `/xx/yy/zz.js`.
  - DataDome: `datadome` cookie, `captcha-delivery.com` endpoint.
  - PerimeterX: `_px*` cookies, `/api/v2/collector` POST.
- **Arquivo**: `src/antibot/mod.rs`
- **Esforço**: M (2 dias)
- **Trilho**: antibot-stealth
- **Aceitação**: cada vendor flagável com ≥2 signals.

---

## P1 — Diferença mensurável em coverage

### P1.1 Scroll behavior humano (bursts + variance)
- **Objetivo**: `scroll_by(dy)` composta de múltiplos wheel events com deltaY não-constante (100 + N(0,15)), pausa Gamma(2, 1s) entre bursts.
- **Arquivo**: `src/render/interact.rs`
- **Esforço**: S (half-day)
- **Dependência**: âmbito 1.3
- **Trilho**: antibot-stealth
- **Status: ✅ shipped** — `src/render/motion/scroll.rs:104` (`schedule_for_active_profile`, bell-curve deltas + Pareto dwells); wired in `src/render/interact.rs:298` (`scroll_by`) via import at `src/render/interact.rs:33`.

### P1.2 Reading dwell time simulado
- **Objetivo**: antes de next-action em página de conteúdo, dwell time = f(words_visible / WPM) com WPM ~ N(250, 40).
- **Arquivo**: `src/render/wait_strategy.rs` + hooks
- **Esforço**: S (half-day)
- **Trilho**: antibot-stealth

### P1.3 Function.toString coverage audit
- **Objetivo**: toda função overridden em shim deve ter `.toString()` retornando `"function X() { [native code] }"` via `Function.prototype.toString` proxy.
- **Arquivo**: `src/render/stealth_shim.js`
- **Esforço**: S (4h)
- **Trilho**: antibot-stealth
- **Status: ✅ shipped** — `src/render/stealth_shim.js:617` (section 13: WeakSet `targets` + Proxy on `Function.prototype.toString`, installed at line 666); registered overrides in sections 16/17 (lines 653, 707, 799, 980).

### P1.4 Plugins shape com "Chrome PDF Plugin" exact
- **Objetivo**: plugins override retorna `"Chrome PDF Plugin"` (não "Chromium"), PluginArray type correct, plugin.description matches real Chrome.
- **Arquivo**: `src/render/stealth_shim.js`
- **Esforço**: S (2h)
- **Fonte**: [DataDome detection](https://datadome.co/bot-management-protection/detecting-headless-chrome-puppeteer-extra-plugin-stealth/)
- **Status: ✅ shipped** — `src/render/stealth_shim.js:202` (section 4) lists 5 PDF plugin strings at lines 223–227 (PDF Viewer / Chrome PDF Viewer / Chromium PDF Viewer / Microsoft Edge PDF Viewer / WebKit built-in PDF).

### P1.5 UA-CH fullVersionList coherence check
- **Objetivo**: test assertion que Sec-CH-UA-Full-Version-List sempre tem mesmo major version que UA Chrome/X.Y.Z. Tooling em `src/identity/` ou `src/impersonate/`.
- **Arquivo**: `src/impersonate/headers.rs`, testes em `tests/`
- **Esforço**: S (2h)

### P1.6 WebGL + WebGPU renderer coherence
- **Objetivo**: se WebGL renderer = `"ANGLE (NVIDIA ...)"`, WebGPU adapter.info.vendor = `"nvidia"`, features list correspondente. Lista de perfis NVIDIA/AMD/Intel coherent.
- **Arquivo**: `src/render/stealth_shim.js`
- **Esforço**: M (1 day)

### P1.7 Apify fingerprint-generator-style Bayesian FP rotation
- **Objetivo**: ao criar nova identity, escolhe FP coherent de uma "pool" derivada de distribuição real-mundo (OS, GPU, UA, UA-CH). Importante para scale com rotação.
- **Arquivo**: `src/identity/` + possível bridge `src/impersonate/profiles/*.json`
- **Esforço**: L (3–5 days)
- **Fonte**: [apify/fingerprint-generator](https://github.com/apify/fingerprint-generator)
- **Trilho**: antibot-stealth / scale

### P1.8 Service Worker + CacheStorage clear em identity rotate
- **Objetivo**: `Storage.clearDataForOrigin({storageTypes:"all"})` + `ServiceWorker.unregister` on rotate.
- **Arquivo**: `src/identity/` + `src/render/pool.rs`
- **Esforço**: S (half-day)
- **Trilho**: antibot-stealth / spa-pwa

### P1.9 ALPS extension validation no TLS
- **Objetivo**: test que BoringSSL build emite extension 0x4469 application_settings com HTTP/2 SETTINGS payload.
- **Arquivo**: `src/impersonate/tls.rs`
- **Esforço**: S (sociedade de capture + audit)

### P1.10 iframe.contentWindow safe override (não-Proxy shape detectable)
- **Objetivo**: em vez de Proxy swap (detectável via DataDome), usar `Object.defineProperty` em `HTMLIFrameElement.prototype` para return valid contentWindow that matches Chrome.
- **Arquivo**: `src/render/stealth_shim.js`
- **Esforço**: M (1 day + testing contra DataDome demo)

### P1.11 Accept-Encoding zstd
- **Objetivo**: HTTP client anuncia `gzip, deflate, br, zstd` (Chrome 123+).
- **Arquivo**: `src/impersonate/headers.rs`
- **Esforço**: S (1h se zstd decoder já existir; senão M para integrar zstd crate)
- **Status: ✅ shipped** — `src/impersonate/mod.rs:836` (`chrome_http_headers_full`) emits `accept-encoding: gzip, deflate, br, zstd` at lines 912–915.

### P1.12 Vendor challenge iframe handling (Turnstile embed)
- **Objetivo**: detect Turnstile widget em página; se Managed Challenge, wait complete ou offload to SaaS solver.
- **Arquivo**: `src/antibot/mod.rs`, `src/escalation.rs`
- **Esforço**: M (1–2 days)

---

## P2 — Nice-to-have, marginal

### P2.1 OS-level input via xdotool/ydotool (headful only)
- **Objetivo**: quando rodando headful, usar OS input para max isTrusted + pageX/screenX coherent.
- **Arquivo**: novo `src/render/os_input.rs`
- **Esforço**: L (requires headful display management, wayland/x11 detection)
- **Trilho**: antibot-stealth

### P2.2 Font list OS-coherent bundle
- **Objetivo**: crawler em Linux mas spoofing Windows UA — bundle Windows font shapes; alternativamente reject Linux-specific fonts do enumerate.
- **Arquivo**: `src/render/stealth_shim.js` (bloqueia `document.fonts.check` de Linux-only)
- **Esforço**: M (2 days)

### P2.3 performance.memory heap limit spoof
- **Objetivo**: se low-mem VPS reporta <1GB heap, shim spoof pra 2-4GB coerente com UA desktop.
- **Arquivo**: `src/render/stealth_shim.js`
- **Esforço**: S (2h)

### P2.4 HTTP/3 (QUIC) support
- **Objetivo**: engine HTTP-spoof e/ou Chromium render usam H3 quando Alt-Svc/HTTPS DNS indica; racing H2/H3.
- **Esforço**: XL (integrate quinn or similar; match Chrome QUIC FP)
- **Trilho**: network

### P2.5 ECH support
- **Objetivo**: ClientHello envelope com outer matching CF, inner SNI encrypted.
- **Esforço**: L (waits for BoringSSL ECH GA; rustls ECH PR)
- **Trilho**: network

### P2.6 Battery/Sensors presence coherence
- **Objetivo**: em Desktop UA, shim remove/undefined Battery (Chrome 103+), Accelerometer, Gyroscope.
- **Esforço**: S (2h)

### P2.7 HSTS/ETag supercookie clear on rotate
- **Objetivo**: `Network.clearBrowserCache` + `Security.clearBrowserHSTS` equivalente.
- **Esforço**: S

### P2.8 Worker pool concurrency consistency
- **Objetivo**: test `navigator.hardwareConcurrency` vs actual worker-parallel execution time; match.
- **Esforço**: M

---

## P3 — Exotic / future-proofing

### P3.1 DMTG diffusion-based trajectory generation
- **Objetivo**: substitui WindMouse por diffusion model trained em dataset humano — indistinguível por ML classifier.
- **Esforço**: XL
- **Fonte**: [arxiv 2410.18233](https://arxiv.org/html/2410.18233v1)

### P3.2 GPU refresh rate clockchip FP defense
- **Objetivo**: se detector corre testufo.com-like long test, emit rAF com pequeno jitter modelado em GPU real.
- **Esforço**: XL, raramente necessário

### P3.3 Camoufox-equivalent C++ fork Chromium
- **Objetivo**: patch Chromium source para spoof navigator/screen/WebGL no C++ level — zero JS shim leak.
- **Esforço**: XXL (weeks+ of Chromium patching; build infra)
- **Trilho**: antibot-stealth / scale / build

### P3.4 Polymorphic vendor challenge solver SaaS integration
- **Objetivo**: escalation via 2Captcha/Capsolver/Hyper-Solutions quando Cloudflare Turnstile / hCaptcha / Arkose interativo aparecer.
- **Arquivo**: `src/escalation.rs`
- **Esforço**: M

### P3.5 VLM-based visual captcha solver (self-hosted)
- **Objetivo**: Gemini/GPT-4V para Arkose/hCaptcha visual puzzles.
- **Esforço**: L

---

## Roadmap por trilho (roll-up)

| Trilho | P0 | P1 | P2 | P3 |
|---|---|---|---|---|
| antibot-stealth | P0.1, P0.2, P0.3, P0.5, P0.8, P0.9 | P1.1–P1.8, P1.10, P1.12 | P2.1, P2.2, P2.3, P2.6 | P3.1, P3.3, P3.4, P3.5 |
| browser-control | P0.4 | P1.7 | | |
| network | P0.6, P0.7 | P1.9, P1.11 | P2.4, P2.5 | |
| spa-pwa | | P1.8 | P2.7 | |
| scale | | P1.7 | | P3.3 |

## Sequência sugerida de execução (sprints de 1 semana)

**Sprint 1 — Motion + Runtime.Enable** (P0.1, P0.2, P0.4)
  - Mouse humano + event integrity + CDP Runtime.Enable audit/fix
  - Validação: brotector score

**Sprint 2 — Keyboard + Network FP** (P0.3, P0.6, P0.7)
  - Keystroke log-normal + HTTP/2 Akamai FP + header order
  - Validação: scrapfly TLS/H2 test service, capture diff

**Sprint 3 — Shim tightening + vendor coverage** (P0.5, P0.8, P0.9, P1.3, P1.4, P1.10)
  - Permissions/Canvas/Plugins/toString/iframe + vendor detect refine

**Sprint 4 — UA-CH coherence + WebGL/GPU** (P1.5, P1.6, P1.9, P1.11)

**Sprint 5 — Identity rotation + behavior sim scaffolding** (P1.1, P1.2, P1.7, P1.8)

**Sprint 6 — Emerging** (P1.12, P2.x)
