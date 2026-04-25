# Deep research — anti-bot evasion para crawler stealth-grade

Meta: levantar o estado-da-arte em evasion/anti-detection pra crawlers headless, cobrindo TODOS os vetores, com profundidade técnica + sinais que detectores usam + contramedidas conhecidas. Output é um documento estruturado que orienta próximas fases de antibot no crawlex.

## Âmbitos obrigatórios (cada um com detalhes profundos)

### 1. Behavioral detection (comportamental)
- Mouse movement: Fitts' law, bezier curves, jitter, idle drift, micro-tremor humano, velocidades típicas
- Keyboard: distribuições de inter-key delay (log-normal, gaussian), pauses "thinking", typo + backspace simulação
- Scroll: wheel vs keyboard vs touch, velocity profiles, pause-and-resume
- Viewport interactions: focus/blur, visibility changes, page scroll depth, dwell time
- Timing fingerprints: INP (Interaction to Next Paint), CLS, FID, como detectores modelam "humano vs bot"
- Event sequence: mousemove → mouseover → click — detectores flagam click sem move prévio

### 2. Browser fingerprinting (passive)
- Canvas fingerprinting: text rendering, image hash, subpixel rendering diffs
- WebGL: GPU vendor/renderer, extensions list, getParameter shadows, shader precision, readPixels
- WebGPU: adapter info, feature list — novo vetor FPJS usa desde 2024
- AudioContext: OfflineAudioContext oscillator fingerprint
- Fonts: `Intl.DateTimeFormat` + CSS font enumeration, measureText widths
- Screen: `screen.width/height/availWidth/colorDepth`, `devicePixelRatio` coherence
- Timezone/locale: `Intl.DateTimeFormat().resolvedOptions()` + `Date.toString()` consistency
- Battery: API removed in modern Chrome, mas presença anômala vira sinal
- Devices: `navigator.mediaDevices.enumerateDevices()`, Bluetooth, USB, HID, Serial APIs
- WebRTC: ICE candidates local IPs (leak via STUN), fingerprint de media constraints
- Hardware concurrency: `navigator.hardwareConcurrency`, `deviceMemory`
- User-Agent Client Hints (UA-CH): Sec-CH-UA-* headers, high-entropy values, UA consistency
- Sensors API: accelerometer/gyroscope — presença em desktop é tell
- Plugins/MimeTypes: deprecated mas detectores ainda olham shape

### 3. Network/TLS fingerprints
- JA3/JA4/JA4S: ClientHello cipher suites, extensions order, signature algos
- HTTP/2 fingerprint (Akamai hash): SETTINGS frames, WINDOW_UPDATE, PRIORITY, header order, pseudo-header order
- ALPS (Application-Layer Protocol Settings): framed vs unframed, payload shape
- Session ticket handling: presence, resumption patterns
- HTTP/3 fingerprint (QUIC) — emergente
- Header order canonicalization: Chrome vs Firefox vs bots
- Accept-Language / Accept-Encoding / Sec-Fetch-* consistency
- Connection: keep-alive vs close patterns

### 4. Headless/automation detection
- `navigator.webdriver` flag (Chrome)
- CDP domains enabled visibility via timing attacks (Runtime.evaluate cost)
- `window.chrome` object completeness
- Permissions API: `notifications` default value difference
- Plugin array shape
- Languages array consistency
- `toString` of native functions (Proxy-trap detection)
- Error stack traces: Puppeteer/Playwright leave signatures
- Iframe detection: headless often lacks proper iframe tree
- Worker creation timing
- Document.cookie behavior in file:// contexts
- Screen vs viewport mismatch patterns
- Debugger pause detection: `debugger` statement timing → reveals devtools

### 5. Vendor deep-dives (anti-bot commercial)
Pra cada: produto, sinais que coletam, endpoints, cookies, SDK, como se comportam, known bypasses:
- **Cloudflare** Bot Management + Turnstile: `cf_chl_jschl_tk`, `cf_clearance`, Challenge Platform, `/cdn-cgi/challenge-platform/`, Turnstile invisible vs managed
- **Akamai Bot Manager**: `_abck` cookie, sensor_data payload, ChallengeAPI
- **DataDome**: `datadome` cookie, captcha-delivery.com, bot scoring signals
- **PerimeterX** (HUMAN): `_px*` cookies, `captcha.px-cloud.net`, px-captcha widget
- **Kasada**: `x-kpsdk-*` headers, polymorphic challenge scripts
- **Imperva** (Incapsula): `incap_ses_*`, `visid_incap_*`, reese84
- **F5 Shape Security**: `TS*` cookies, VM-style JS obfuscation
- **reCAPTCHA v3**: score 0.0-1.0, action-based, risk signals
- **hCaptcha**: enterprise analytics, invisible mode
- **Arkose Labs** (FunCaptcha): image puzzles, enterprise grade

### 6. Cookies / storage / session
- Supercookies: ETag, HSTS, TLS session ID abuse
- Storage partitioning (CHIPS, Storage Access API)
- Service Worker interception patterns
- IndexedDB fingerprint stores
- BroadcastChannel cross-tab signals
- First-party sets

### 7. OS/env signals
- Process list introspection via timing attacks
- Memory consumption patterns (detectores medem via perf.memory)
- Font list cross-OS (Linux-only fonts are a tell)
- CPU throttling detection (jitter em setTimeout)
- Screen refresh rate / requestAnimationFrame cadence

### 8. Counter-techniques OSS (projetos pra estudar/portar)
- **puppeteer-extra-plugin-stealth**: 20+ evasion plugins, specific strategies
- **rebrowser-patches**: Node.js patches for Puppeteer/Playwright undetectability
- **nodriver** (UC → Cursor): post-selenium evolution
- **undetected-chromedriver**: strategies, known gaps
- **camoufox**: custom Firefox fork (vs Chromium), fingerprint rotation
- **playwright-stealth**: port derivative
- **botasaurus**: commercial-grade lib
- **curl-impersonate**: TLS/H2 fingerprint matcher
- **azuretls-client**: Go alternative
- **tls-client** / **cycletls** (Go): production-grade TLS spoofing
- **impit** (Rust): moderno, em evolução
- **FlareSolverr**: Cloudflare bypass proxy
- Papers acadêmicos: "FPJS bypass", "Headless Chrome detection", "Behavioral Biometrics for Bot Detection"

### 9. Human motion models (pra simulação)
- Bezier curves (2-control vs 3-control vs Catmull-Rom)
- Jitter models: Perlin noise, Ornstein-Uhlenbeck process
- Fitts' law: `MT = a + b*log2(D/W + 1)`
- Gaussian sampling pra keystroke timing com μ, σ típicos
- Saccadic eye model analogues
- Microsleep/pause distributions (Pareto)
- Reading speed models (words per minute, scroll cadence correlation)

### 10. Emerging detections (2024-2026)
- AI-based behavioral classifiers (detectores usando ML agora)
- TLS 1.3 encrypted ClientHello (ECH) — bot detectors têm que revisar estratégia
- Chrome Privacy Sandbox APIs (Topics, FLEDGE) — novos vetores de fingerprinting
- WebGPU fingerprinting boom (FPJS 4.x adicionou)
- Client Hints 2 evolution
- Cross-origin iframe bot scoring

## Entregáveis

- [ ] `research/evasion-deep-dive.md` — documento estruturado por âmbito com:
  - Sinais detectáveis (o que detectores medem)
  - Valores esperados de humano vs bot (ranges, shapes)
  - Contramedidas conhecidas (como spoofar sem quebrar site)
  - Ferramentas OSS que implementam
  - Referências (links, papers, CVEs, source code)

- [ ] `research/evasion-gap-analysis.md` — comparação brutal: o que crawlex TEM hoje vs o que precisa. Tabela por âmbito:
  ```
  | Âmbito | Estado atual crawlex | Gap | Prioridade | Fonte de referência |
  ```

- [ ] `research/evasion-actionable-backlog.md` — lista priorizada de implementações derivadas do gap analysis, cada uma com:
  - Objetivo
  - Arquivo/módulo atingido
  - Esforço estimado (S/M/L/XL)
  - Dependência de qual âmbito
  - Trilho do roadmap (antibot/stealth, browser control, spa/pwa, artifacts, scale)

- [ ] Use **WebSearch + WebFetch agressivamente**. Cada vendor + cada técnica precisa ter pelo menos 2 fontes primárias citadas (blog do vendor, paper, source code do OSS).

- [ ] **Sem resumo executivo raso** — usuário pediu PROFUNDIDADE. Prefira 200 linhas detalhadas por âmbito vs 20 superficiais.

## Restrições
- Só research. Não modificar código fonte do crawlex.
- Pode criar diretório `research/` na raiz do repo com os 3 documentos.
- Output concreto, acionável, técnico. Sem generalidades.
- Cite fontes (URLs) sempre.
- Respeite licenças dos OSS mencionados — informe as licenças quando for relevante pra porting.
- Sem commits.
- Escrever `.done` em `.dispatch/tasks/evasion-research/ipc/.done` quando terminar.
