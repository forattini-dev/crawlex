# Gap Analysis — crawlex vs estado-da-arte antibot

> Compara o que crawlex tem hoje vs sinais atuais + contramedidas. Prioridade: **P0** (crítico, blocker em sites top-tier), **P1** (diferença mensurável em vendor coverage), **P2** (nice-to-have, marginal signal), **P3** (exotic / future-proofing).

## Legenda do estado atual
- ✅ Já implementado
- 🟡 Parcial (existe mas tem gap específico)
- ❌ Não implementado

---

## Behavioral

| Sinal | Estado atual crawlex | Gap | Prioridade | Fonte |
|---|---|---|---|---|
| Mouse trajectory (bezier + jitter) | 🟡 "infra mínima (bezier + jitter)" | sem Fitts-law velocity profile, sem WindMouse/OU, sem sub-movements/overshoot, sem idle drift | **P0** | [ghost-cursor](https://github.com/Xetera/ghost-cursor), [WindMouse](https://ben.land/post/2021/04/25/windmouse-human-mouse-movement/) |
| Event stream integrity (mousemove→mouseover→click) | 🟡 | precisa garantia no actions.rs que todo click tem sequência completa precedente | **P0** | [brotector](https://github.com/kaliiiiiiiiii/brotector) |
| isTrusted=true em inputs | ✅ via CDP Input.dispatchMouseEvent | OS-level (xdotool/ydotool) não-implementado; detectores avançados (pageX/screenX divergence) podem flagar | **P1** | [Castle nodriver](https://blog.castle.io/from-puppeteer-stealth-to-nodriver-how-anti-detect-frameworks-evolved-to-evade-bot-detection/) |
| Keyboard log-normal hold/flight | ❌ | nenhuma simulação; type() provavelmente constante | **P0** | [keystroke PMC](https://pmc.ncbi.nlm.nih.gov/articles/PMC8606350/) |
| Keyboard thinking pauses + typos | ❌ | ausente | **P1** | |
| Scroll non-constant delta + bursts | ❌ | sem scroll simulado humano | **P1** | [SEL SearchGuard](https://searchengineland.com/inside-google-searchguard-467676) |
| Dwell time / reading model | ❌ | crawler visita e sai rápido | **P1** | |
| rAF cadence humana / jitter 16.67ms | 🟡 | Chrome render engine dá grátis; mas em `--headless=old` seria issue; confirmar `--headless=new` uso | **P2** | |

## Fingerprinting passivo

| Sinal | Estado atual crawlex | Gap | Prioridade | Fonte |
|---|---|---|---|---|
| navigator.webdriver drop | ✅ stealth shim v3 | confirmar que `--disable-blink-features=AutomationControlled` também usado e que info-bar não mostra | **P0** | |
| WebGL vendor/renderer mock | ✅ | coerência com UA/platform (NVIDIA em "Linux" UA?), renderer string format exato | **P1** | |
| WebGPU adapter spoofing | ✅ "WebGL/WebGPU mocks" | confirmar adapter.info + features list coherence com WebGL renderer | **P1** | [scrapfly WebGPU](https://scrapfly.io/web-scraping-tools/gpu-fingerprint/webgpu) |
| Canvas seed determinístico | ✅ "canvas/audio seed" | validar que mesmo seed across toDataURL + getImageData (noise em ambos) | **P1** | [Castle noise detection](https://blog.castle.io/detecting-noise-in-canvas-fingerprinting/) |
| AudioContext seed | ✅ | idem canvas | **P1** | |
| UA-CH low+high entropy | ✅ "UA-CH full" | validar `Sec-CH-UA-Full-Version-List` match com UA string; `platformVersion` correto p/ Windows 11; `formFactors` Chrome 128+ | **P1** | |
| Accept-Language vs navigator.languages coherence | 🟡 | precisa sempre sync automatic, audit | **P1** | |
| Accept-Encoding `zstd` (Chrome 123+) | ? | validar que client espuma `gzip, deflate, br, zstd` | **P1** | |
| Sec-Fetch-* regras estritas | 🟡 | validar por request type (document/xhr/fetch/script) | **P1** | |
| Screen coherence (inner ≤ outer, devicePixelRatio) | 🟡 | validar conjunto, não só spoof individual | **P2** | |
| Fonts list OS-consistent | ❌ | Linux chromium render font default — spoof Windows UA mas font list revela Ubuntu | **P1** | camoufox |
| Plugins shape (PluginArray type, Chrome PDF Plugin string) | 🟡 | "Chrome PDF Plugin" vs "Chromium PDF Plugin" lint | **P1** | [DataDome](https://datadome.co/bot-management-protection/detecting-headless-chrome-puppeteer-extra-plugin-stealth/) |
| Permissions API `notifications` coherence | ❌ | Puppeteer vira `'denied'` quando Notification.permission='default' — fix via override | **P0** | |
| WebRTC mDNS + enumerateDevices shape | 🟡 | confirmar que `enumerateDevices()` retorna ≥1 audioinput/videoinput labels blank | **P1** | |
| Sensors / Battery presence coherence | 🟡 | em Desktop UA, não expor Accelerometer/Gyroscope | **P2** | |
| performance.memory heap limit coherence | ❌ | em low-mem VPS leaks → spoof via shim | **P2** | |
| Battery API removed correctly | 🟡 | Chrome 103+ Linux removed; garantir não expor | **P2** | |

## Network / TLS / HTTP

| Sinal | Estado atual crawlex | Gap | Prioridade | Fonte |
|---|---|---|---|---|
| TLS JA4 Chrome-exact | ✅ via BoringSSL | versão alvo (Chrome 149) up-to-date com GREASE positions, ALPS, key_share, supported_versions | **P0** | [curl-impersonate](https://github.com/lwthiker/curl-impersonate) |
| JA4S validation server-side consciência | ❓ | tipicamente não controlado client-side; apenas verificar que o server response não é usado para challenge | **P3** | |
| JA4T (kernel TCP options) | ❌ | requires socket opts; normalmente não exposto | **P2** | [JA4T](https://medium.com/foxio/ja4t-tcp-fingerprinting-12fb7ce9cb5a) |
| HTTP/2 SETTINGS order (Akamai FP) | ❓ | crawlex deve mandar `1:65536;2:0;4:6291456;6:262144`; confirmar em tls.rs | **P0** | [Akamai WP](https://blackhat.com/docs/eu-17/materials/eu-17-Shuster-Passive-Fingerprinting-Of-HTTP2-Clients-wp.pdf) |
| HTTP/2 WINDOW_UPDATE increment = 15663105 | ❓ | validar h2 crate config | **P0** | |
| HTTP/2 pseudo-header order m,a,s,p | ❓ | confirmar em headers.rs | **P0** | |
| HTTP/2 sem PRIORITY separate frames (Chrome moderno) | ❓ | Chrome usa priority em HEADERS frame + `priority: u=0, i` header | **P1** | |
| ALPS extension (0x4469) | ❓ | BoringSSL adiciona se compilado com suporte — validar | **P1** | [curl-impersonate Chrome post](https://lwthiker.com/reversing/2022/02/20/impersonating-chrome-too.html) |
| HTTP/3 (QUIC) support + fingerprint | ❌ | crawler usa TCP — Chrome prefere H3 via Alt-Svc/HTTPS DNS; destoar | **P2** | |
| Happy Eyeballs H2/H3 racing | ❌ | Chrome races; crawler não | **P2** | |
| ECH (encrypted ClientHello) support | ❌ | Chrome 117+ default-on em CF sites | **P2** | [CF ECH](https://blog.cloudflare.com/encrypted-client-hello/) |
| Header order (document nav vs xhr) | 🟡 | validar ordem exata por request type | **P0** | |

## Headless / automation detection

| Sinal | Estado atual crawlex | Gap | Prioridade | Fonte |
|---|---|---|---|---|
| Runtime.Enable leak (console.log Error.stack) | ❓ | crawlex usa Chrome 149 patched — confirma que NÃO envia Runtime.enable após initial; implementar addBinding-style | **P0 🔴** | [Rebrowser](https://rebrowser.net/blog/how-to-fix-runtime-enable-cdp-detection-of-puppeteer-playwright-and-other-automation-libraries-61740) |
| Error.prepareStackTrace abuse | ❓ | validar | **P0** | |
| debugger; timing worker | ❌ | requires specific defense; se não há devtools attached, OK | **P2** | |
| window.chrome completeness (app, csi, loadTimes, runtime) | 🟡 | verificar cada uma no shim v3 | **P1** | puppeteer-stealth |
| iframe.contentWindow HEADCHR evasion | 🟡 | com Proxy cuidado para não fazer detectable swap | **P1** | [DataDome iframe](https://datadome.co/threat-research/how-datadome-detects-puppeteer-extra-stealth/) |
| cdc_* / $cdc_* globals clean | ✅ stealth shim | periodic scan | **P1** | |
| Playwright globals (`__pwInitScripts`, `__playwright__binding__`) | N/A | crawlex não usa Playwright | - | |
| sourceURL = `app.js` (not `pptr:`) | N/A | crawlex roda JS via seu próprio runtime; validar que `//# sourceURL=` não vaza "pptr/puppeteer/crawlex" | **P1** | |
| Utility world name genérico | N/A | crawlex não é Puppeteer mas se criar isolated worlds, dar nome comum | **P2** | |
| Permissions.notifications override | ❌ | ver §Fingerprinting | **P0** | |
| Function.toString [native code] | 🟡 | validar em todas funções spoofed no shim | **P1** | |

## Cookies / storage / session

| Sinal | Estado atual crawlex | Gap | Prioridade | Fonte |
|---|---|---|---|---|
| Cookies per-session persistente | ✅ | | - | |
| LocalStorage persistence | ✅ | | - | |
| IndexedDB persistence | ❓ | SPA deep crawl menciona IndexedDB — confirmar persistência & clear quando rotating | **P2** | |
| Service Worker unregister on rotate | ❓ | crítico para identity swap | **P1** | |
| Cache Storage clear | ❓ | idem | **P2** | |
| ETag supercookie awareness | ❌ | se rotating identity, ETag persiste — clear | **P2** | |
| HSTS cookie awareness | ❌ | rotating identity should clear HSTS storage | **P3** | |
| Partitioned cookies (CHIPS) | N/A | browser faz automatic | - | |
| Session ticket / TLS resumption | ❓ | validar se reutiliza entre identities | **P2** | |

## Vendor coverage

| Vendor | Estado atual | Gap | Prioridade | Fonte |
|---|---|---|---|---|
| Cloudflare (bot mgmt / Turnstile) | 🟡 detect via ChallengeHit outcome | coverage Challenge Platform endpoints; Turnstile widget no JS; behavioral entropy baseline | **P0** | |
| Akamai BM | 🟡 detect | sensor_data generator — sem replay capacity; H2 FP tem que bater | **P0** | [Edioff/akamai-analysis](https://github.com/Edioff/akamai-analysis) |
| DataDome | 🟡 detect | CDP signal specifically; puppeteer-stealth signatures não aplicáveis já que crawlex não usa | **P0** | |
| PerimeterX / HUMAN | 🟡 detect | payload PX320–PX348 com browser deep probe | **P1** | |
| Kasada | 🟡 detect | polymorphic; economically infeasible; fallback a solver SaaS | **P1** | |
| Imperva | 🟡 detect | reese84 endpoint awareness + utmvc; cookies tracked | **P1** | |
| F5 Shape | 🟡 detect | VM obfuscation; hardest. Fallback a SaaS solver | **P2** | |
| reCAPTCHA v3 | 🟡 detect | precisa Google cookies + reputação IP | **P1** | |
| hCaptcha | 🟡 detect | server-sent tasks com delay expected; fallback a solver SaaS | **P1** | |
| Arkose | 🟡 detect | visual puzzle, solver SaaS ou VLM | **P2** | |

## OS / env

| Sinal | Estado atual crawlex | Gap | Prioridade | Fonte |
|---|---|---|---|---|
| hardwareConcurrency spoof coherent com deviceMemory | 🟡 | em shim v3 — validar não contradiz WebWorker concurrency test | **P1** | |
| Font list OS-coherent | ❌ | Linux Chromium tem Ubuntu fonts by default | **P1** | camoufox |
| setTimeout jitter ≥1-4ms pattern | 🟡 | sob carga cpu pode destoar; rARS 50% | **P3** | |
| GPU refresh rate (testufo) | ❌ | requires long capture; raramente usado | **P3** | |

## OSS integration gaps (o que crawlex não usa mas poderia)

| Ferramenta | Estado | Valor | Prioridade |
|---|---|---|---|
| apify/fingerprint-generator (Bayesian net de FPs reais) | ❌ | rotate FP consistente por identity | **P1** |
| WindMouse / DMTG pra motion | ❌ | gold standard motion model | **P0** |
| camoufox pra Firefox vetor | ❌ | se Chromium detectado, Firefox-based engine paralelo útil | **P2** |
| FlareSolverr (proxy serve CF bypass) | ❌ | parcial / quebrado Oct 2024; não confiável | **P3** |
| tls-client pattern (Chrome_133, Chrome_144 profiles) | 🟡 (BoringSSL equiv) | confirmar versioning track Chrome 149 | **P1** |

## Emerging (2024-2026)

| Vetor | Estado atual | Gap | Prioridade |
|---|---|---|---|
| WebGPU adapter spoof | ✅ parcial | feature list must match GPU | **P1** |
| ECH support | ❌ | outer CH shape issue | **P2** |
| AI-based behavior classifier robustness | ❌ | sem motion ML-aware, mesmo boa estática falha | **P0** |
| Cross-origin iframe bot scoring | 🟡 | precisa handling de iframe challenges (Turnstile) | **P1** |
| HTTP/3 / QUIC FP | ❌ | | **P2** |
| Privacy Sandbox (rescinded Oct 2025) | N/A | | - |
