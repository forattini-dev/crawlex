# Evasion Deep Dive — estado da arte anti-bot (2024–2026)

> Research destilado de vendor blogs, papers, OSS sources e reverse-engineering notes. Cada âmbito lista: **sinais** (o que detectores medem), **humano vs bot** (ranges / shapes), **contramedidas** (como spoofar sem quebrar), **OSS que implementa**, **fontes primárias**. Foco: profundidade técnica, não marketing.

Glossário rápido:
- **FP** = fingerprint. **FPJS** = FingerprintJS. **BM** = bot-management.
- **CDP** = Chrome DevTools Protocol. **Main world** = execution context padrão do frame. **Isolated world** = contexto separado criado via `Page.createIsolatedWorld`.
- **Sinal low-entropy vs high-entropy**: UA-CH low (brand, mobile, platform) vs high (fullVersionList, architecture, model, platformVersion, bitness, wow64).

---

## Âmbito 1 — Behavioral (mouse / keyboard / scroll / timing)

### 1.1 Mouse movement

Sinais coletados por detectores:
- **Trajetória geométrica**: coords `(x,y,t)` no `mousemove`. Detectores calculam:
  - Curvatura (ângulo entre vetores consecutivos). Humano: distribuição gaussiana em torno de 0 com cauda longa; bot linear: ~0. Bot bézier ingênuo: suavidade impossível (segunda derivada zero ou constante).
  - Velocidade instantânea `v_i = dist/dt`. Humano: perfil Fitts-law com pico ~no meio e deceleração de aproximação (ver §9). Bot constante: variância próxima de zero.
  - Aceleração (terceira derivada: jerk). Humanos têm jerk elevado por micro-tremor; bots têm jerk quase nulo em bézier suave.
- **Stop/corrigir**: sub-movimentos Fitts (Meyer 1988). Humano reaching tem geralmente 2-3 sub-movements com overshoot + correção. Bot direto não tem.
- **Event-rate/sampling**: Chrome entrega `mousemove` a ~60-120Hz dependendo da taxa do mouse e do rAF. Bot que gera via `Input.dispatchMouseEvent` (CDP) costuma ter samples espaçados uniformemente (ex: 50 eventos exatamente a cada 20ms) — detectável via Welford variance ~0.
- **Pré-click sequence**: browsers reais emitem `mousemove → mouseover → mouseenter → mousedown → mouseup → click`. Headless falho: `click` sem `mousemove` prévio → feature importante pro classifier.
- **pageX/Y vs screenX/Y**: brotector verifica divergência anômala; `Input.dispatchMouseEvent` pode gerar screen coords inconsistentes ([brotector.js](https://github.com/kaliiiiiiiiii/brotector/blob/master/brotector.js)).
- **isTrusted**: `event.isTrusted === false` em eventos sintetizados via JS dispatch. Só CDP ou OS-level input produz `isTrusted=true`. É por isso que nodriver migra pra OS-level input. ([Castle — Puppeteer-stealth to nodriver](https://blog.castle.io/from-puppeteer-stealth-to-nodriver-how-anti-detect-frameworks-evolved-to-evade-bot-detection/))

Humano vs bot (valores típicos):
- Peak velocity reaching: 400–1200 px/s. Duration 150–700 ms Fitts.
- Jitter stationary (idle drift): ±1–3 px por segundo (tremor fisiológico).
- Taxa de eventos: 60–120 Hz, com gaps irregulares (jitter ~3–15ms).
- Overshoot ratio: ~0.3–0.8 sub-movements/reach.
- Click offset em relação ao centro do target: gaussiana σ ≈ 20–30% do raio.

Contramedidas:
- **Path**: Bézier cúbica entre start/target com pontos de controle aleatorizados (offset ~15–40% da distância normal ao vetor). Melhor ainda: WindMouse (ben.land), DMTG (entropy-controlled diffusion, arxiv 2410.18233).
- **Velocity**: Fitts `MT = a + b*log2(D/W+1)`. Bot deve interpolar velocidade com perfil senoidal-like (peak ~40–55% do caminho).
- **Jitter**: Perlin/Ornstein-Uhlenbeck noise sobreposto (OU converge a média com ruído branco, modela tremor realista ([OU Wikipedia](https://en.wikipedia.org/wiki/Ornstein%E2%80%93Uhlenbeck_process))).
- **Sub-movements**: drop precision em ~70% do caminho; ajustar com novo Fitts movement menor.
- **Idle drift**: entre ações, emitir mousemoves de baixa amplitude (~2–5 px) com frequência baixa (2–10/s).
- **Click offset**: click em `target.center + gaussian(σ=0.25*W)`.
- **OS-level input** (nível máximo): `xdotool`/`ydotool` (Linux), `SendInput` Win, `CGEventPost` macOS — gera events `isTrusted=true` e passa por path real do browser. Nodriver implementa essa abordagem. [nodriver](https://github.com/ultrafunkamsterdam/nodriver)

OSS:
- [ghost-cursor](https://github.com/Xetera/ghost-cursor) — Fitts + bezier + Puppeteer.
- [OxyMouse](https://github.com/oxylabs/OxyMouse) — coleção Bezier/Perlin/Gaussian.
- [bezmouse](https://github.com/vincentbavitz/bezmouse) — xdotool OS-level.
- [human_mouse](https://github.com/sarperavci/human_mouse) — splines + fitts.
- [WindMouse Ben Land](https://ben.land/post/2021/04/25/windmouse-human-mouse-movement/) — gravity+wind model clássico.

### 1.2 Keyboard

Sinais:
- **Hold time** (dwell): duração keydown→keyup. **Flight time**: keyup→próximo keydown. Ambos são modelados por distribuições **log-normal** ou **log-logistic** (Shadman 2024, [Shape of timings PMC8606350](https://pmc.ncbi.nlm.nih.gov/articles/PMC8606350/)). Gaussian não serve; detectores detectam simetria.
- **Bigrams**: certos pares (e.g., "th", "er") têm flight times menores (familiarity). Bot que usa mean global é flagável por análise de n-gramas.
- **Typing speed bursts**: humanos têm "bursts" de 3–7 chars rápidos separados por micropausas (~200–500ms) pra pensar. Bots com delay constante entre chars (50ms, 100ms) têm variance quase zero.
- **Typos + backspace**: humanos cometem ~1–3% de typos com correção. Bot perfeito é suspicioso.
- **Modifier keys**: Shift held time durante letter; Caps Lock state.
- **Synthetic forgeries**: GaussianBot/NoiseBot papers mostram que mesmo gaussian-with-mean falha contra classifiers treinados com log-normal ground-truth ([Stefan 2010](https://cseweb.ucsd.edu/~dstefan/pubs/stefan:2010:keystroke.pdf)).

Humano vs bot:
- Hold: μ ≈ 80–120ms, σ grande, log-normal (cauda direita).
- Flight: μ ≈ 100–200ms, varia fortemente por bigram; valores negativos possíveis (roll-over digitando rápido).
- WPM: 40–80 normal, 80–120 skilled.

Contramedidas:
- Amostrar hold e flight de log-normal per-bigram.
- Inserir "thinking pauses" Pareto-distributed (pausa longa rara).
- Simular 1–2% typos com backspace.
- Usar `Input.dispatchKeyEvent` com `type: rawKeyDown/char/keyUp` separados e timings realistas.

OSS: [keystroke-dynamics review arxiv 2502.16177](https://arxiv.org/html/2502.16177v1).

### 1.3 Scroll

Sinais:
- **deltaY distribution**: wheel notch típico Windows = 100–120 px, macOS trackpad inertial = 0.5–3 px por evento mas com 40–80 eventos durante inertia. Bot que usa `Input.dispatchMouseWheelEvent` com deltas uniformes (120, 120, 120) é trivialmente detectado.
- **Variance deltaY**: Google (SearchGuard) usa Welford algorithm; variance <5 px é suspeito, humano típico 20–100 ([Scrape/bot detection SEL](https://searchengineland.com/inside-google-searchguard-467676)).
- **deltaMode**: 0=pixel, 1=line, 2=page. Touchpad macOS = 0; mouse wheel Windows = 0 (mas com notch alignment); Firefox Linux às vezes = 1.
- **Velocity profile**: humano scroll em bursts, pausa pra ler, scroll backward eventualmente.
- **Scroll depth + dwell time**: tempo parado em cada faixa da página — humano tem pico no início (lendo título).

Contramedidas:
- Emular **scroll bursts** com pausas Gamma-distributed (2–8s).
- Usar delta não-constante, ex: notch noise `delta = 100 + N(0,15)` ou inertial decay `delta_i = delta_0 * exp(-k*i)`.
- Scroll back ocasional.
- Alinhar com leitura: tempo por "viewport-height" ≈ 3–10s em páginas de conteúdo.

### 1.4 Timing fingerprints (Core Web Vitals como sinal)

Detectores medem:
- **INP**: Interaction to Next Paint. Bots síncronos que executam JS heavy após click têm INP alto (500ms+).
- **FID**: First Input Delay — humano típico 10–80ms; bot com JS heavy em paralelo >200ms.
- **Event cadência rAF**: `requestAnimationFrame` deltas devem alinhar com VSYNC ~16.67ms (60Hz), ±5% jitter normal por GC/layout. Headless sem display pode produzir cadência "too perfect" 16.666666 exato (detectável por variance near 0). Inversamente, CPU throttle via CDP expõe `requestAnimationFrame` rodando em 25–30fps com pattern específico.
- **testufo.com refresh rate** fingerprint (ClockChip GPU): [WebKit explainers #89](https://github.com/WebKit/explainers/issues/89).

### 1.5 Event sequence integrity

Checklist que detectores aplicam:
- `click` sem `mousemove` prévio no mesmo elemento → bot.
- `focus` sem `mousemove`/`keydown` → bot (ex: form submit programático).
- `keydown` sem `focus` do elemento → bot.
- `touchstart` → deve vir com `touchmove` + `touchend` + coordenadas coerentes (e.g., Touch.radiusX positivo).

---

## Âmbito 2 — Passive browser fingerprinting

### 2.1 Canvas

Sinais:
- Render de string padrão (`"Cwm fjordbank glyphs vext quiz 😃"`) em `<canvas>`, hash do pixel data. Entropia ~15–20 bits entre combinações GPU + driver + OS + sub-pixel rendering.
- `measureText` widths — diferenças sub-pixel.
- **Detecção de noise spoofing**: Castle e Kameleo mostram que detectores fazem **2 renders na mesma session** e comparam hash; se diferente, é noise. Solução: determinismo por seed estável (mesmo noise em todas as chamadas). ([Castle — detecting noise](https://blog.castle.io/detecting-noise-in-canvas-fingerprinting/))

Contramedidas:
- Não randomizar per-call; usar noise determinístico por visitorID+origin.
- Alternativa: passar valores de GPU específico real (match renderer string).
- Verificar que `toDataURL` e `getImageData` dão o mesmo pixel fingerprint (noise tem que ser aplicado em ambos, NÃO só em toDataURL).

### 2.2 WebGL

Sinais:
- `gl.getParameter(UNMASKED_VENDOR_WEBGL)` / `UNMASKED_RENDERER_WEBGL` (via WEBGL_debug_renderer_info). Chrome 113+ bloqueia em alguns contextos mas a maioria dos sites ainda lê.
- `gl.getSupportedExtensions()` — listas ordenadas; order diff entre GPU drivers.
- `getShaderPrecisionFormat` — precisão HIGH_FLOAT / MEDIUM_FLOAT. Alguns GPUs reportam rangeMin/rangeMax diferentes.
- `readPixels` de um draw calibrado — diff subpixel.

Contramedidas:
- Spoof de renderer string deve casar com GPU real (senão inconsistente com perf de shader real). Lista típica Chrome Win: `"ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0)"`.
- Apify fingerprint-suite gera combinações coherent GPU+driver+OS da "distribuição real do mercado".

### 2.3 WebGPU (vetor 2024+, FPJS 4.x)

- `navigator.gpu.requestAdapter()` → `adapter.info` (vendor, architecture, device, description) + `adapter.features` (set de features ex: `"texture-compression-bc"`, `"shader-f16"`).
- **Entropia**: 10–15 bits ([scrapfly webgpu db](https://scrapfly.io/web-scraping-tools/gpu-fingerprint/webgpu)). Combina com Chrome version e OS pra high-entropy vector.
- **Emerging**: FPJS adicionou WebGPU em 2024; CreepJS o inclui. [BrowserLeaks WebGPU](https://browserleaks.com/webgpu).

Contramedidas:
- Se spoofar WebGL, DEVE spoofar WebGPU coerentemente (mesmo vendor, feature list mapeável). Gap comum: WebGL spoofa NVIDIA mas WebGPU deixa Intel HD nativo.
- Opção simples em stealth shim: `navigator.gpu = undefined`. Mas anomalia — browsers pós-Chrome 113 expõem. Melhor spoofar.

### 2.4 AudioContext

Sinais:
- `OfflineAudioContext` + oscilador + dynamicCompressor → hash do buffer final. Diferenças <1e-6 entre implementações (CPU arch + OS + Chrome version).
- `getChannelData()` sum — número de 15-digit que identifica device.

Contramedidas:
- Noise determinístico por seed (FPJS detecta sessão inconsistente).
- Camoufox faz no C++ — idealmente patch DFG-level do browser.

### 2.5 Fonts

- `document.fonts.check("12px 'Font Name'")` enumerate. OS-specific fonts (Ubuntu Monospace → Linux Chromium; Helvetica Neue → macOS; Segoe UI → Windows).
- `measureText` deltas por font.

Contramedidas: spoofar lista consistente com `userAgentData.platform` + locale. Camoufox bundles OS font sets.

### 2.6 Screen / viewport

- `screen.width`, `availWidth`, `innerWidth`, `outerWidth`, `devicePixelRatio`.
- **Coherence**: `innerWidth <= outerWidth`; `availWidth <= width`. Headless default (800×600) é sinal forte; CDP `Emulation.setDeviceMetricsOverride` mas muitos outros gaps.
- puppeteer-extra-stealth tem `window.outerdimensions` evasion porque Puppeteer deixa outerWidth/outerHeight = 0 em alguns casos.

### 2.7 UA-CH (User-Agent Client Hints)

- **Low-entropy headers** (enviados sempre): `Sec-CH-UA`, `Sec-CH-UA-Mobile`, `Sec-CH-UA-Platform`.
- **High-entropy** (só quando explicitamente pedido via `Accept-CH` header ou `navigator.userAgentData.getHighEntropyValues([...])`): `architecture`, `bitness`, `model`, `platformVersion`, `fullVersionList`, `wow64`, `formFactor`.
- **Consistency**: UA string "Chrome/149" tem que bater com `Sec-CH-UA-Full-Version-List`. `platform` no UA ("Windows NT 10.0; Win64; x64") tem que bater com `Sec-CH-UA-Platform: "Windows"` + `Sec-CH-UA-Platform-Version: "15.0.0"`.
- Chrome 128+ adiciona `formFactors` (pode ser `["Desktop", "XR"]`).

### 2.8 WebRTC

- **mDNS ICE candidates**: Chrome 76+ mascara IP local como UUID.local. Mas se site pede `RTCPeerConnection({iceServers:[STUN]})` com gathering policy `"all"`, ainda vaza IP público (proxy → IP real pode vazar!). Precisa bloquear STUN ou usar `srflx` com proxy ciente.
- `navigator.mediaDevices.enumerateDevices()` — count de `audioinput/audiooutput/videoinput`. Headless com flag default retorna 0 → red flag. Spoof: devolver ~2 audioinput, 1 videoinput, 2 audiooutput com labels vazios até permission.

### 2.9 Sensors / hardware APIs

- `Battery` API — removida do Chrome 103 em Linux mas em Android ainda.
- `Accelerometer/Gyroscope` — Desktop Chrome não expõe → presença = mobile. Se UA é Desktop mas Sensors expõe, inconsistência.
- `navigator.usb`, `navigator.bluetooth`, `navigator.hid`, `navigator.serial` — presença atrelada a permission; não expor é fine em headless se consistente.

### 2.10 Plugins / MimeTypes

- Chrome real: 5 plugins internos (Chrome PDF Plugin, Chrome PDF Viewer, Native Client, Widevine + PDFium). `navigator.plugins.length === 0` em headless antigo. **Chrome 131+ ainda retorna 5** em headful e 0 em `--headless` — em `--headless=new` retorna idêntico ao headful.
- **Shape detection**: puppeteer-extra-stealth overrides com "Chromium PDF Plugin" em vez de "Chrome PDF Plugin" — lint fácil.

### 2.11 Sources/reference

- [CreepJS](https://abrahamjuliot.github.io/creepjs/) — poster child de FP deep.
- [browserleaks.com/webgpu](https://browserleaks.com/webgpu) etc — tabela de signals.
- [niespodd/browser-fingerprinting](https://github.com/niespodd/browser-fingerprinting) — survey.

---

## Âmbito 3 — Network / TLS / HTTP fingerprints

### 3.1 JA3 (legacy) vs JA4 (atual)

JA3 (Salesforce, 2017): `SSLVersion,Ciphers,Extensions,EllipticCurves,EllipticCurvePointFormats` → MD5. Weakness: order changes ⇒ hash changes; GREASE poluindo; não discrimina QUIC.

JA4 (FoxIO 2023): formato legível `txxsscceeaa_cipherhash12_exthash12`. ([spec](https://github.com/FoxIO-LLC/ja4/blob/main/technical_details/JA4.md)):
- `(t|d|q)` protocol (TCP/DTLS/QUIC).
- `(12|13|10|d2)` TLS version via `supported_versions` (ignorar GREASE).
- `(d|i)` SNI presente / IP direto.
- 2 dígitos cipher count (sem GREASE).
- 2 dígitos extension count (sem GREASE) — inclui SNI + ALPN.
- 2 chars ALPN (first+last do first ALPN, ex: `h2` → `h2`, `http/1.1` → `h1`, hex se não-alfanum).
- `_` + truncated SHA256(sorted_hex(ciphers sem GREASE)).
- `_` + truncated SHA256(sorted_hex(exts − GREASE − SNI − ALPN) + "_" + sig_algs ordem original).

JA4 suite (BSD para JA4; FoxIO 1.1 para restante):
- **JA4H**: HTTP request fingerprint. Componentes: método (`ge`/`po`), HTTP version, cookies flag, referrer flag, header count (não-Cookie/Referer), primary language, truncated hash do **header order** + hash dos cookie name order + hash dos cookie name=value.
- **JA4S**: server hello — cipher escolhido + ext ordem + ALPN. Combinar com JA4 identifica library/malware family.
- **JA4L**: latency/light — RTT estimation via handshake timings.
- **JA4T**: TCP options fingerprint (MSS, window scale, options order, SACK).
- **JA4X**: X.509 cert fingerprint.
- Cloudflare adotou JA4 em 2024 em Bot Management signals. ([Cloudflare blog](https://blog.cloudflare.com/ja4-signals/))

Contramedidas:
- **curl-impersonate** / **curl_cffi** compilam com BoringSSL + injetam mesmo set de extensions + GREASE positions que Chrome ([Chrome impersonation post](https://lwthiker.com/reversing/2022/02/20/impersonating-chrome-too.html)).
- **utls (gaukas/utls)** — fork Go com ClientHelloSpec customizável (Chrome_Auto, Chrome_131, Firefox_105, etc).
- **rustls-based** stacks: rustls não expõe API pra ordem de extensions; precisa **BoringSSL via boring-rs crate** ou patch rustls.
- O crawlex já tem "TLS Chrome-exact via BoringSSL (JA4 match)" — bom. Gap: JA4S (server validation via jars nem sempre), JA4T (kernel TCP opts). JA4T requires kernel-level control (sockopt) e não é normalmente exposto por TLS libs.

### 3.2 HTTP/2 fingerprint (Akamai 2017 whitepaper)

Formato: `SETTINGS|WINDOW_UPDATE|PRIORITY|PSEUDO_HEADER_ORDER`. ([Akamai whitepaper](https://blackhat.com/docs/eu-17/materials/eu-17-Shuster-Passive-Fingerprinting-Of-HTTP2-Clients-wp.pdf))
- **SETTINGS**: `ID:Value` pairs separados por `;` na ordem enviada. Chrome atual (144): `1:65536;2:0;4:6291456;6:262144` (HEADER_TABLE_SIZE, ENABLE_PUSH=0, INITIAL_WINDOW_SIZE 6MB, MAX_HEADER_LIST_SIZE 256KB).
- **WINDOW_UPDATE**: increment em stream 0. Chrome ~15663105 (15MB).
- **PRIORITY frames**: `StreamID:Exclusivity:DependentStreamID:Weight`, comma-separated. **Chrome moderno NÃO envia separate PRIORITY frames**: usa `priority` em HEADERS frame + header RFC 9218 `priority: u=0, i`. Se seu client envia PRIORITY frames, destoa.
- **Pseudo-header order**: `m,a,s,p` (method/authority/scheme/path). Chrome = `m,a,s,p`; Firefox = `m,p,a,s`; curl default = `m,p,s,a`.

Contramedidas:
- tls-client / bogdanfinn: permite custom `H2Settings`, `H2Priorities`, `PseudoHeaderOrder`, header `Order` slice.
- Go `net/http` default tem pseudo-header errado (`m,a,s,p` hard-coded? não, manda m,s,a,p) e falha.
- Para Rust: `h2` crate é customizável via `SendSettings` mas é verbose; hyper default OK-ish mas gap em priority dependency.

### 3.3 ALPS (application_settings extension, 0x4469)

- Google-added extension que envia HTTP/2 SETTINGS durante o ClientHello. Chrome envia. curl-impersonate inclui. Muitos bots (Go stdlib) não.
- Detecção: presença da extension ID 0x4469 no ClientHello.

### 3.4 HTTP/3 / QUIC fingerprint

- **Transport parameters** no QUIC CRYPTO frame: ordem, initial_max_data, initial_max_streams_bidi, max_ack_delay.
- ALPN = `h3`. 
- Chrome prefere HTTP/3 via Alt-Svc ou DNS HTTPS record. Se client sempre fallback TCP, é sinal.
- bogdanfinn/tls-client implementa "Happy Eyeballs" Chrome-style.

### 3.5 Headers

- **Accept-Language**: tem que bater com `navigator.languages` e locale. `q`-values ex: `en-US,en;q=0.9`.
- **Accept-Encoding**: Chrome = `gzip, deflate, br, zstd` (zstd desde Chrome 123). Ausência de `br` ou `zstd` destoa de UA Chrome 149.
- **Sec-Fetch-\***: `Sec-Fetch-Site`, `Sec-Fetch-Mode` (`navigate|cors|no-cors|same-origin`), `Sec-Fetch-Dest` (`document|image|script|...`), `Sec-Fetch-User` (`?1` se user-initiated). Estes têm regras estritas dependendo do request type; errar = bot.
- **Header order**: Chrome order para document navigation: `Host, Connection(HTTP/1), Upgrade-Insecure-Requests, User-Agent, Accept, Sec-Fetch-Site, Sec-Fetch-Mode, Sec-Fetch-User, Sec-Fetch-Dest, Accept-Encoding, Accept-Language, Cookie`. Pra XHR/fetch: `sec-ch-ua, sec-ch-ua-mobile, sec-ch-ua-platform, Accept, sec-fetch-site, sec-fetch-mode, sec-fetch-dest, Referer, Accept-Encoding, Accept-Language, Cookie`.
- **Case**: HTTP/2 força lowercase; HTTP/1 tem title-case. Libs que mandam lowercase em HTTP/1 flagam.

### 3.6 ECH (Encrypted ClientHello, 2024+)

- Encrypt inner ClientHello com ECHConfig pública. SNI oculto.
- **Impacta detectores**: não podem mais JA3/JA4 no outer, só no envelope. Mas outer ClientHello TEM shape — Cisco EVE fingerprint outer shape mesmo sem decrypt.
- Chrome 117+ e Firefox 118+ suportam; Cloudflare ativou default em 2024 ([CF blog](https://blog.cloudflare.com/encrypted-client-hello/)).
- Implicação pro crawler: se não suporta ECH e alvo serve ECH config, aparência destoa de Chrome atual.

---

## Âmbito 4 — Headless / automation detection

### 4.1 navigator.webdriver

- Chrome injeta `navigator.webdriver = true` quando `--enable-automation` ativo ou quando conectado via CDP com `Target.setAutoAttach`. Default: flag passa a `true`.
- **Bypass simples**: `--disable-blink-features=AutomationControlled` previne injection. Tradeoff: Chrome mostra info bar amarela "unsupported command-line flag" (só em debug mode ou se user vê).
- **Bypass definitivo**: patch CDP / usar CDP.Page.addScriptToEvaluateOnNewDocument pra `Object.defineProperty(navigator, 'webdriver', {get: () => undefined})` — mas detectável via `Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver')` se mal feito.

### 4.2 CDP / Runtime.Enable leak (a big one)

- **Técnica DataDome/Cloudflare** ([DataDome blog](https://datadome.co/threat-research/how-new-headless-chrome-the-cdp-signal-are-impacting-bot-detection/)): quando `Runtime.enable` foi chamado, qualquer `console.*` call triggers CDP `Runtime.consoleAPICalled` event com serialization de argumentos. Se o argumento tem getter pra `.stack`/`.name`, o getter é invocado durante serialization → counter aumenta. Detecção:
```js
let count = 0;
const e = new Error();
Object.defineProperty(e, 'stack', { get(){ count++; return 'x'; } });
console.log(e);
// if Runtime.Enable active: count ≥ 1; else: count === 0
```
- Também via **`Error.prepareStackTrace`** hook: CDP captura stack → se prepareStackTrace está defined, é chamado sob CDP mas não em normal.
- **brotector** usa combinação: worker with `debugger;` timing + lookup counters. ([brotector.js](https://github.com/kaliiiiiiiiii/brotector/blob/master/brotector.js))

Contramedidas (rebrowser-patches):
1. **addBinding mode**: não chama `Runtime.Enable`; cria bindings via `Runtime.addBinding` por frame e extrai contextId do runtime binding.
2. **alwaysIsolated mode**: cria isolated world via `Page.createIsolatedWorld`; main world nunca tem Runtime.Enable side-effects.
3. **enableDisable mode**: enable → capture contextId → immediately disable. Race-y mas funciona se load rápido.

### 4.3 window.chrome

- Real Chrome: `window.chrome = {app, csi, loadTimes, runtime}`. Puppeteer headless: `undefined`. puppeteer-extra-stealth plugins `chrome.app/csi/loadTimes/runtime` preenchem.
- **Shape check**: detectores testam `typeof chrome.runtime.PlatformOs` → real Chrome expõe enum, polyfill às vezes tem diff.

### 4.4 Permissions API

- Real Chrome: `navigator.permissions.query({name:'notifications'})` com `Notification.permission === 'default'` retorna `'prompt'`.
- Headless Puppeteer: retorna `'denied'` com `Notification.permission === 'default'` → **inconsistência**. Clássica detecção do Castle/headless-detect gists. Stealth plugin `navigator.permissions` corrige.

### 4.5 Plugins array (já discutido §2.10)

### 4.6 Languages array

- `navigator.languages` empty vira sinal. Default Puppeteer `['en-US']`. Chrome real tem 2+ languages geralmente (locale + fallback).

### 4.7 `toString` native detection (Proxy traps)

- `Function.prototype.toString.call(navigator.plugins.item)` em Chrome real: `"function item() { [native code] }"`. Se a função foi monkey-patched, toString pode vazar source (`"function item() { return x }"`). Fix: proxy toString para retornar `[native code]` string.
- **Detectores checam**: `"" + navigator.plugins.item === "function item() { [native code] }"` e também `Function.prototype.toString.toString()`.
- **Proxy detection**: `try { Proxy.toString() } catch(e) { /* real Proxy throws TypeError ... */}`. Ou `new Proxy({}, {}).valueOf() === target.valueOf()`. Se monkey-patched, diffs.

### 4.8 Error stack traces

- Puppeteer script evaluation: stack tem `at __puppeteer_evaluation_script__`. Rebrowser-patches muda `sourceURL` pra `app.js` via env `REBROWSER_PATCHES_SOURCE_URL`.
- Playwright: `__pwInitScripts`, `__playwright__binding__` globals. Brotector checks.

### 4.9 Iframe detection (HEADCHR_IFRAME)

- Em headless, `iframe.contentWindow.self.get?.toString()` retorna valor diferente de real. Stealth plugin `iframe.contentWindow` troca Proxy. DataDome detecta presença desse Proxy via comparações `iframe.contentWindow === iframe.contentWindow` (identity) ou via `frameElement` quirks.
- Playwright: iframe inspection pode revelar `__pw_*` propriedades no contentDocument.

### 4.10 CDC/ChromeDriver signature

- `window.cdc_adoQpoasnfa76pfcZLmcfl_*` — ChromeDriver legacy injection. Undetected-chromedriver remove.
- `window.$cdc_*` variants. Brotector scan patterns `cdc_[a-z0-9]`.

### 4.11 Debugger pause timing

- Worker threads com `debugger;` statement: se devtools/CDP attached, pausa até continue; timing muito maior. Worker measures `performance.now()` delta → reveals attached debugger.

### 4.12 Input event isTrusted

- Already discussed §1.1. `Input.dispatchMouseEvent` do CDP gera `isTrusted=true` (passa pelo browser input pipeline), mas **JavaScript `element.click()` ou `dispatchEvent` gera `isTrusted=false`**. Detectores requerem `isTrusted` em form submit handlers.

### 4.13 --headless=new vs legacy

- Chrome 109+ `--headless=new` compartilha codebase com headful. Flags detectable:
  - `--headless=new` não tem maioria dos "headless" giveaways.
  - Mas muitos bots ainda usam `--headless=old`.
- Sinal: `navigator.webdriver` + ausência de tudo que `--headless=new` preserva (UA "HeadlessChrome" era old legacy; new é "Chrome/...").

---

## Âmbito 5 — Vendor deep-dives

### 5.1 Cloudflare Bot Management + Turnstile

Componentes:
- **Managed Challenge** — decide automaticamente entre JS, captcha interactivo, ou silent.
- **Turnstile** — invisível, managed, ou non-interactive widget. Proof-of-work + proof-of-space + probing de APIs + behavioral. ([CF developer docs](https://developers.cloudflare.com/cloudflare-challenges/challenge-types/javascript-detections/))
- `/cdn-cgi/challenge-platform/h/{b|g}/...` é o endpoint script-delivery. `/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1?ray=XXX` retorna JS obfuscado que faz POST pra `/flow/ov1/...`.
- Cookies: `__cf_bm` (bot management), `cf_clearance` (após challenge), `__cf_chl_jschl_tk__` (token ativo durante challenge).
- Sinais Turnstile 2025 (confirmados): **entropia de mouse trajectory** (não-linear, não-bezier puro), timing randomness, TLS JA4, HTTP/2 Akamai, UA consistency, IP reputation (`__cf_bm` rotating).
- Signal "entropy": curvas bezier puras FAIL; linear interpolation FAIL. Exigem jitter real-time stocástico.
- **Iframe Turnstile widget** `challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile/if/ov2/av0/rcv/...`. Scripts contêm VM-like interpreter.

Known bypass:
- Não há "solver" estável. Melhor abordagem: FlareSolverr (ficou quebrado em Oct 2024); Puppeteer+nodriver+proxy residencial + tempo; ou offload pra 2Captcha/Capsolver.
- Crawlex: dado o CF é agressivo, JS execution real (browser engine) é mandatory — HTTP-only falha.

### 5.2 Akamai Bot Manager (v2 "sbsd", v1.7 legacy)

- Cookie `_abck` (token), `ak_bmsc` (sessão), `bm_sz` (device score), `bm_sv`. ([Edioff/akamai-analysis](https://github.com/Edioff/akamai-analysis), [Akamai whitepaper](https://blackhat.com/docs/eu-17/materials/eu-17-Shuster-Passive-Fingerprinting-Of-HTTP2-Clients-wp.pdf))
- **sensor_data**: payload opaco POST pra `/xx/yy/zz.js` endpoint (URL varia por site). Estrutura: `{"sensor_data":"<base64 or delimited string>"}` com campos: UA, plugins, canvas hash, webgl, screen, audio, mouse events, keyboard events, timestamps, device orientation, touch, network timing. Criptografia: v1.7 usa XOR + key derivada do script (variável); v2 ("sbsd") usa AES-CBC com key derivada de device chars + server-side pepper.
- **script obfuscation**: string obfuscation, control-flow flattening, timing traps (measures execution time of canvas ops → detects debugger slowdown).
- Flow: load Akamai script → collect signals por ~500–1500ms → POST sensor_data → server retorna `_abck` válido.
- Known bypass repos (fase ~2022-2024): cirleamihai/akamai-1.7-cookie-generator, xiaoweigege/akamai2.0-sensor_data, i7solar/Akamai. **Estáveis? Não.** Akamai rotaciona script weekly.
- JA4 + HTTP/2 fingerprint validation server-side — Akamai valida que TLS+H2 batem com UA claimed.

### 5.3 DataDome

- Cookies: `datadome` (token, rotates). Challenge endpoint `captcha-delivery.com` ou `*.captcha-delivery.com`.
- **Signals collected**: navigator deep dive (UA, plugins shape), canvas, webgl, battery, hardware concurrency, audio, WebGL precision, `Error.prepareStackTrace` abuse, CDP Runtime.Enable detection, console proxy detection.
- **Detecta puppeteer-extra-stealth specificamente**: plugin signature via plugin.description string mismatch, Proxy on navigator.plugins.item, iframe contentWindow Proxy. ([DataDome blog](https://datadome.co/bot-management-protection/detecting-headless-chrome-puppeteer-extra-plugin-stealth/))
- **CDP signal (2024)**: console.log de Error com getter stack. Antoine Vastel blog.
- Bypass: real browser (non-Puppeteer) + proxy residencial + behavior. Challenge payload às vezes pode ser replayed (curto TTL).

### 5.4 PerimeterX / HUMAN

- Cookies: `_px`, `_px2`, `_px3`, `_pxde`, `_pxhd`, `pxcts`. Endpoint `captcha.px-cloud.net` ou `*.perimeterx.net`.
- **Payload**: base64 JSON POST pra `/api/v2/collector`. Campos numerados PX320–PX348 (device model, name, OS, timestamp, UUID, SHA1, SDK ver, bundle id). Mobile iOS SDK semelhante ([PerimeterX-Reverse repos, antibot.blog](https://antibot.blog/posts/1741549175263)).
- **Challenge do array**: server envia `do: [...]` com operators; cliente computa int → POST como PX257.
- V3 recent: mais obfuscação, WebAssembly challenge.
- Bypass: PerimeterX-Solver v6.7.9 open-source é outdated. Produção usa SaaS.

### 5.5 Kasada

- Headers: `x-kpsdk-cd`, `x-kpsdk-ct`, `x-kpsdk-st`, `x-kpsdk-r`, `x-kpsdk-c`. 
- **Polymorphic challenge**: IPS loads JS que é diferente a cada request (op-code rotation, function name shuffling). Heavy obfuscation. Goal é economic infeasibility.
- Flow: `/147/...` endpoint returns challenge; client computes POST pra `/tl/...` retorna tokens; tokens required em header próximo request.
- Bypass: reverse engineering repos existem mas caem em dias. [lktop/kpsdk](https://github.com/lktop/kpsdk) tem algoritmo partial.

### 5.6 Imperva Incapsula

- Cookies: `reese84`, `incap_ses_*`, `visid_incap_*`, `nlbi_*`. Utmvc challenge (`___utmvc`) também. ([BottingRocks/Incapsula](https://github.com/BottingRocks/Incapsula), [yoghurtbot deobf blog](https://yoghurtbot.github.io/2023/03/04/Deobfuscating-Incapsula-s-UTMVC-Anti-Bot/))
- **reese84**: payload POST pra `/reese84/...` ou per-site endpoint. Contém device telemetry em JSON + heavy obfuscation wrapper. Encoder function tem morphed (variant per request via server seed).
- Bypass: reese84 solvers existem; chega-se ao token via replay rápido antes de rotate.

### 5.7 F5 Shape / Distributed Cloud Bot Defense

- Cookies: `TS*` names (TS01xxxx, TS0123abc) — encrypted session tokens. Device ID computed server-side.
- **VM obfuscation**: custom stack-based CISC VM em JS, rotating opcode table (changes per-script), virtualized bytecode. Deobfuscation requires reversing the VM first. ([g2asell2019/shape-security-decompiler-toolkit](https://github.com/g2asell2019/shape-security-decompiler-toolkit))
- The hardest enterprise target. Bypass typically requires human in-loop + residential proxies + real browser profile + aged cookies.

### 5.8 reCAPTCHA v3

- Score 0.0–1.0, action-based (`action: "login"`).
- **Signals**: mouse, scroll, timing, IP reputation, Google account cookies presence (hugely boosts score), UA reputation, referrer graph.
- Sites usam score threshold (0.5 default, 0.7 strict).
- Bypass: maintain good Google cookies (logged-in gmail helps), residential IP, real mouse.

### 5.9 hCaptcha

- Enterprise analytics — 225+ signal types. Privacy-preserving ML, per-site model.
- Sinais behavior: mouse entropy, timing intentional (server sends task with expected delay — bot responds faster → flag).
- Bypass: 2Captcha-style human solvers; otherwise hard.

### 5.10 Arkose Labs (Matchkey / FunCaptcha)

- Image-puzzle matching (match rotating 3D object). 225+ risk signals.
- **Signals collected**: WebGL render times, cursor drift, acceleration curves, micromotions, previous tokens, device intelligence.
- Per-session token; answer key per-puzzle.
- Bypass: human solvers; AI vision solvers for simpler puzzles (v2/v3 games).

---

## Âmbito 6 — Cookies / storage / session signals

### 6.1 Supercookies

- **ETag**: server sends unique ETag → browser stores → next request sends `If-None-Match`. Persists even with cookie-clear (no user visibility). Safari ITP blocks, Chrome doesn't (sort of).
- **HSTS**: subdomain enumeration. `a.tracker.com` set HSTS, b.tracker.com doesn't → encoded bit pattern into 32 subdomains = 32-bit cookie.
- **TLS session ID / session ticket**: server-provided, resumption detection. Chrome aggressively tickets.

### 6.2 Storage partitioning (CHIPS)

- Chrome 114+ third-party cookies partitioned per top-level site. `Set-Cookie: name=value; SameSite=None; Secure; Partitioned`. Affects SSO / embed trackers.
- **Storage Access API**: `document.requestStorageAccess()` — Chrome fires prompt; user decides.
- Bot crawling bypass: sessão separada por top-level site helps but may break multi-SSO flows.

### 6.3 Service Worker interception

- Workers intercept `fetch` → can inject identity headers, rewrite requests, persist IDs.
- Detectors install SW that refuses to unregister → persistent across sessions.
- Crawlex: precisa `Storage.clearDataForOrigin` + `ServiceWorker.unregister` antes de switch identity.

### 6.4 IndexedDB / CacheStorage / BroadcastChannel

- IDB stores device IDs. Cache Storage preload models. BroadcastChannel cross-tab sync.
- For crawler: `Storage.clearDataForOrigin({origin, storageTypes:"all"})` is comprehensive.

### 6.5 First-party sets (FPS / RWS Related Website Sets)

- Chrome 118+. Top-level site can declare related sets → cookies/storage share within set.
- Bot detectors use FPS to link identities across example.com and example-shop.com.

---

## Âmbito 7 — OS / env signals

### 7.1 performance.memory

- Chrome-only. `.totalJSHeapSize`, `.usedJSHeapSize`, `.jsHeapSizeLimit`. Heap limit typically 2–4 GB (1GB em Chrome Android). Too-small limit (e.g., 512MB) = sandboxed/restricted env.
- Inconsistência: headless em low-mem VPS reports 512MB limit but UA says desktop Chrome = red flag.

### 7.2 deviceMemory + hardwareConcurrency coherence

- `navigator.deviceMemory` retorna `0.25|0.5|1|2|4|8` (capped 8 for privacy). `hardwareConcurrency` retorna logical cores.
- Mismatch: `deviceMemory=8`, `hardwareConcurrency=1` — anomalia. Or WebWorker concurrency test (spawn N workers, measure execution time) indica cores reais ≠ navigator claim ([Castle deep dive](https://blog.castle.io/deep-dive-how-navigator-devicememory-can-be-used-for-fingerprinting-and-bot-detection/)).

### 7.3 Font list vs OS

- Ubuntu fonts on "Windows" UA → spoof leak. Camoufox bundles cross-OS sets.

### 7.4 CPU throttling / setTimeout jitter

- setTimeout(fn, 0) mínimo ~1–4ms, maior sob load. Pattern anomalias (muito uniforme ou muito caótico) = headless/throttle.
- rAF cadence detection (see §1.4).

### 7.5 Screen refresh rate

- testufo.com clockchip FP. Requires long capture (5-30min) but can detect GPU model to 5–6 digits precision.

---

## Âmbito 8 — OSS counter-techniques

### 8.1 puppeteer-extra-plugin-stealth (Node, MIT)

Plugins (17–19): `navigator.webdriver`, `navigator.languages`, `navigator.vendor`, `navigator.hardwareConcurrency`, `navigator.plugins`, `navigator.permissions`, `chrome.app`, `chrome.csi`, `chrome.loadTimes`, `chrome.runtime`, `iframe.contentWindow`, `media.codecs`, `sourceurl`, `user-agent-override`, `webgl.vendor`, `window.outerdimensions`, `defaultArgs`. ([evasions dir](https://github.com/berstend/puppeteer-extra/tree/master/packages/puppeteer-extra-plugin-stealth/evasions))

Limitations (as of 2024-2025): "plugin hasn't been updated since 2022; open-source nature makes it easy for anti-bots to block." DataDome detects the plugin itself.

### 8.2 rebrowser-patches (MIT)

- Patches Puppeteer/Playwright source to fix Runtime.Enable leak (modes: `addBinding` default, `alwaysIsolated`, `enableDisable`).
- Muda default `sourceURL=pptr:` → `app.js` (configurable).
- Renomeia utility world `__puppeteer_utility_world__<ver>` → `util`.
- Expose `browser._connection()` API.
- Status: up to Puppeteer 24.8.1, Playwright 1.52.0.
- **Crítico**: sem este patch, Puppeteer/Playwright são **red-flag** em todos os major vendors ([Rebrowser blog](https://rebrowser.net/blog/how-to-fix-runtime-enable-cdp-detection-of-puppeteer-playwright-and-other-automation-libraries-61740)).

### 8.3 nodriver (ultrafunkamsterdam, MIT)

- Python. Successor de undetected-chromedriver.
- **CDP-minimal**: evita Runtime.Enable, Console, alguns domains inteiros. Async.
- Input via CDP (ainda) mas com timing e sequence humana.
- `start(expert=True)` disables web security + shadow-roots always open.
- Fresh profile per session.
- Architecture: sidesteps, não patches. ([Castle review](https://blog.castle.io/from-puppeteer-stealth-to-nodriver-how-anti-detect-frameworks-evolved-to-evade-bot-detection/))

### 8.4 undetected-chromedriver (MIT)

- Patches Selenium ChromeDriver. Remove cdc_* globals. Rename navigator.webdriver. Suits.
- Legacy; gaps vs modern detectors. Superseded by nodriver.

### 8.5 camoufox (MIT, Firefox fork)

- **C++ level patches** (não JS injection!): navigator, screen, WebGL, audio, hardware, geolocation, WebRTC (protocol level). ([github.com/daijro/camoufox](https://github.com/daijro/camoufox))
- Juggler patched for sandboxing Playwright from page inspection.
- Generates fingerprints via BrowserForge matching real-world distribution.
- OS fonts bundled.
- Anti-tracing: disables CSS animations, removes pointer-type headless tell, re-enables PDF.js.
- Para Chrome-based: sem equivalent C++-level fork estável (vanadium-like?). Camoufox é **gold standard** antifp.

### 8.6 curl-impersonate + curl_cffi

- C + BoringSSL. Matches Chrome/Firefox/Safari/Edge ClientHello byte-perfect.
- ALPS extension (0x4469), GREASE positions, HTTP/2 SETTINGS/WU/PRIORITY.
- Python binding `curl_cffi` — drop-in `requests.get(impersonate="chrome131")`.
- **No browser engine** — purely HTTP. Complementa ao browser-based engine.

### 8.7 bogdanfinn/tls-client (Go, MIT)

- utls-based. Client profiles: Chrome_133, Chrome_144, Firefox, Safari, OkHTTP.
- Custom H2Settings, H2Priorities, pseudo-header order, header order slice.
- HTTP/3 support.

### 8.8 impit (Rust, emerging)

- Rust client spoof TLS+H2. Similar ao tls-client; immature mas active.

### 8.9 FlareSolverr

- Proxy server spawning undetected-chromedriver per-request; returns cookies.
- Status: 2024 Oct Cloudflare update broke many; maintained forks exist.

### 8.10 apify/fingerprint-suite (MIT)

- `fingerprint-generator`: Bayesian network de real fingerprints, genera consistent {headers, screen, navigator, webgl, audio}.
- `fingerprint-injector`: applies FP to Puppeteer/Playwright context before navigation.
- Header generation matches browser family.

### 8.11 Academic / detection research

- **CreepJS** (Abraham Juliot) — inspeciona tudo.
- **bot-detection-technology**: Iqbal et al "Khaleesi: Breaker of Advertising and Tracking Request Chains".
- "Fingerprinting Information in JavaScript Implementations" (Mowery 2011) — classic.
- "Beauty and the Burst" — browser perf timing FP.
- "FPJS Pro bypass" research threads.

---

## Âmbito 9 — Human motion models (simulation)

### 9.1 Fitts' Law

`MT = a + b · log₂(D/W + 1)` onde D=distância alvo, W=largura alvo.
- Typical `a`=0.1s, `b`=0.15 s/bit. Movement time MT em seconds.
- Use: duration total do movement.
- Shannon-Fitts index of difficulty ID = log₂(D/W+1).

### 9.2 Bezier paths

- Cubic: 4 control points P0..P3. Trajetória `B(t) = (1-t)^3·P0 + 3(1-t)^2·t·P1 + 3(1-t)·t^2·P2 + t^3·P3`.
- P1/P2 aleatorizados: offset normal-ao-vetor com magnitude `|D|/4 · N(0,1)`, e parallel `|D|/3 · U(0.3, 0.7)`.
- Catmull-Rom splines (C¹ continuous, passa pelos pontos): mais realista pra multi-waypoint motion.

### 9.3 WindMouse (ben.land 2021)

Pseudo:
```
def windmouse(x0, y0, x1, y1, G=9, W=3, M=15, D=12):
    vx, vy, wx, wy = 0, 0, 0, 0
    while hypot(x1-x, y1-y) > 1:
        dist = hypot(x1-x, y1-y)
        wmag = min(W, dist)
        if dist >= D:
            wx = wx/√3 + (rand()*2-1)*wmag/√5
            wy = wy/√3 + (rand()*2-1)*wmag/√5
        else:
            wx /= √3
            wy /= √3
            if M < 3: M = rand()*3+3
            else: M /= √5
        gx = G * (x1-x) / dist
        gy = G * (y1-y) / dist
        vx += wx + gx
        vy += wy + gy
        v = hypot(vx, vy)
        if v > M:
            vclip = M/2 + rand()*M/2
            vx = vx/v * vclip
            vy = vy/v * vclip
        x += vx; y += vy
        emit(round(x), round(y))
        sleep(step_delay)
```

### 9.4 Perlin noise jitter

- 1D Perlin com frequency ~0.1 Hz, amplitude 1–3 px. Sobreposto à trajetória.

### 9.5 Ornstein-Uhlenbeck jitter

- SDE `dX_t = θ(μ - X_t)dt + σ dW_t`. Mean-reverting to μ, σ controls jitter amplitude.
- Melhor pra simular tremor stationary em idle.

### 9.6 Gaussian sampling keystroke

- Per-bigram: `hold_time ~ LogNormal(μ=log(0.1), σ=0.3)`. Flight: `LogLogistic(α=0.12, β=3)`.
- Pauses (thinking): `Pareto(x_m=0.5, α=2)` — long tail.

### 9.7 Reading model

- Words per minute: skilled adult 200–300 WPM. Convert to scroll cadence: se viewport mostra 500 words, dwell ~100–150s.
- Eye saccade analogue: mouse moves em "reading checkpoints" — pequenas paradas simulando fixação.

### 9.8 DMTG (diffusion-based, 2024)

- Entropy-controlled diffusion networks generate trajectories that bypass ML classifiers. ([arxiv 2410.18233](https://arxiv.org/html/2410.18233v1))

---

## Âmbito 10 — Emerging detections (2024–2026)

### 10.1 AI behavioral classifiers

- Sites top-100 rodam classifier tree-based (XGBoost, LightGBM) + deep (LSTM/Transformer) em sequencias de eventos. Input: 30s window de mouse/keyboard/scroll + FP static.
- Arkose 225+ features, hCaptcha per-site models, Cloudflare ML + rule hybrid.
- Implicação: bot que replicate static FP perfeitamente still flagged se trajetória for bezier limpa.

### 10.2 ECH impact

- Outer CH shape still fingerprintable. Cisco EVE, Cloudflare com visibility interna. Bots sem ECH destoam de Chrome 117+ prod.

### 10.3 Privacy Sandbox (rescinded)

- Chrome retired Topics/Protected Audience Oct 2025. Curto tempo de vida como vetor. Google mantém Trust Tokens / Private State Tokens em alguns contextos — podem identificar "trusted device" vs "fresh".

### 10.4 WebGPU FP boom

- FPJS 4.x. Critical para spoof coherent com WebGL.

### 10.5 Client Hints evolution

- `Sec-CH-UA-Form-Factors`, `Sec-CH-UA-WoW64`, `Sec-CH-Prefers-Reduced-Motion`, `Sec-CH-Prefers-Color-Scheme`. Coherence across growing.

### 10.6 Cross-origin iframe bot scoring

- Detectores embutem iframe de bot-score-domain (CF Turnstile, DataDome); iframe coleta signals isolados e phones home. Storage partitioning dificulta mas também ajuda rotação.

### 10.7 QUIC fingerprinting

- Transport params order, stream limits, ACK ranges. Emerging; pouco tooling OSS.

### 10.8 AI-based captcha solvers vs AI-based detectors (arms race)

- Detectors usando GPT-4V/VLM para validate visual answers. Captchas com semantic questions ("pick the item that doesn't belong") hard for current vision. 2026 target: biometric-like behavioral continuous auth.

---

## Fontes consolidadas (primárias)

**Vendors / blogs**
- [DataDome — CDP signal](https://datadome.co/threat-research/how-new-headless-chrome-the-cdp-signal-are-impacting-bot-detection/)
- [DataDome — detecting puppeteer-stealth](https://datadome.co/bot-management-protection/detecting-headless-chrome-puppeteer-extra-plugin-stealth/)
- [DataDome — headless chromeless](https://datadome.co/headless-browsers/chromeless/)
- [Cloudflare — JA4 signals](https://blog.cloudflare.com/ja4-signals/)
- [Cloudflare — ECH](https://blog.cloudflare.com/encrypted-client-hello/)
- [CF challenges docs](https://developers.cloudflare.com/cloudflare-challenges/)
- [Rebrowser — Runtime.Enable fix](https://rebrowser.net/blog/how-to-fix-runtime-enable-cdp-detection-of-puppeteer-playwright-and-other-automation-libraries-61740)
- [Castle — nodriver evolution](https://blog.castle.io/from-puppeteer-stealth-to-nodriver-how-anti-detect-frameworks-evolved-to-evade-bot-detection/)
- [Castle — canvas noise detection](https://blog.castle.io/detecting-noise-in-canvas-fingerprinting/)
- [Castle — deviceMemory](https://blog.castle.io/deep-dive-how-navigator-devicememory-can-be-used-for-fingerprinting-and-bot-detection/)
- [FoxIO — JA4+](https://blog.foxio.io/ja4+-network-fingerprinting)
- [lwthiker — Chrome impersonation](https://lwthiker.com/reversing/2022/02/20/impersonating-chrome-too.html)
- [antibot.blog — PerimeterX SDK](https://antibot.blog/posts/1741549175263)
- [yoghurtbot — utmvc deobf](https://yoghurtbot.github.io/2023/03/04/Deobfuscating-Incapsula-s-UTMVC-Anti-Bot/)

**Specs / whitepapers**
- [JA4 tech spec](https://github.com/FoxIO-LLC/ja4/blob/main/technical_details/JA4.md)
- [Akamai HTTP/2 whitepaper (Shuster 2017)](https://blackhat.com/docs/eu-17/materials/eu-17-Shuster-Passive-Fingerprinting-Of-HTTP2-Clients-wp.pdf)
- [Privacycheck LRZ HTTP/2 FP](https://privacycheck.sec.lrz.de/passive/fp_h2/fp_http2.html)

**OSS**
- [curl-impersonate](https://github.com/lwthiker/curl-impersonate) / [lexiforest fork](https://github.com/lexiforest/curl-impersonate)
- [bogdanfinn/tls-client](https://github.com/bogdanfinn/tls-client)
- [puppeteer-extra-plugin-stealth](https://github.com/berstend/puppeteer-extra/tree/master/packages/puppeteer-extra-plugin-stealth)
- [rebrowser-patches](https://github.com/rebrowser/rebrowser-patches)
- [nodriver](https://github.com/ultrafunkamsterdam/nodriver)
- [undetected-chromedriver](https://github.com/ultrafunkamsterdam/undetected-chromedriver)
- [camoufox](https://github.com/daijro/camoufox)
- [apify/fingerprint-suite](https://github.com/apify/fingerprint-suite)
- [FlareSolverr](https://github.com/FlareSolverr/FlareSolverr)
- [brotector](https://github.com/kaliiiiiiiiii/brotector)
- [ghost-cursor](https://github.com/Xetera/ghost-cursor)
- [OxyMouse](https://github.com/oxylabs/OxyMouse)
- [niespodd/browser-fingerprinting](https://github.com/niespodd/browser-fingerprinting)

**Reverse engineering repos (educational, licenças variadas)**
- [Edioff/akamai-analysis](https://github.com/Edioff/akamai-analysis)
- [xiaoweigege/akamai2.0-sensor_data](https://github.com/xiaoweigege/akamai2.0-sensor_data)
- [Pr0t0ns/PerimeterX-Reverse](https://github.com/Pr0t0ns/PerimeterX-Reverse)
- [MiddleSchoolStudent/PerimeterX-Reverse](https://github.com/MiddleSchoolStudent/PerimeterX-Reverse)
- [BottingRocks/Incapsula](https://github.com/BottingRocks/Incapsula)
- [g2asell2019/shape-security-decompiler-toolkit](https://github.com/g2asell2019/shape-security-decompiler-toolkit)
- [lktop/kpsdk](https://github.com/lktop/kpsdk) (Kasada)

**Papers**
- [Shape of timings (PMC8606350)](https://pmc.ncbi.nlm.nih.gov/articles/PMC8606350/)
- [Detecting Web Bots via Keystroke Dynamics (IFIP 2024)](https://ifip.hal.science/IFIP-AICT-710/hal-05043682)
- [Keystroke-dynamics vs synthetic forgeries (Stefan 2010)](https://cseweb.ucsd.edu/~dstefan/pubs/stefan:2010:keystroke.pdf)
- [DMTG diffusion networks](https://arxiv.org/html/2410.18233v1)
- [Emulating Human-Like Mouse Movement (IJIRT)](https://ijirt.org/publishedpaper/IJIRT183343_PAPER.pdf)
- [Keystroke dynamics review (2024)](https://arxiv.org/html/2502.16177v1)
