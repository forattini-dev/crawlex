# Stealth Hardening Plan

Plano de implementação derivado de análise comparativa entre nosso codebase e `h4ckf0r0day/obscura`. Foco: reduzir taxa de detecção/fingerprinting que ainda persiste mesmo com `stealth_shim.js` (1686 linhas, 29 seções).

**Premissa:** nossa cobertura de fingerprint estático já é superior à de Obscura. Os vazamentos prováveis estão em (a) flags de launch, (b) coerência cross-layer, (c) janelas de race em subframes, (d) signals comportamentais subutilizados.

---

## Sumário executivo

| Prioridade | Item | Esforço | Impacto esperado |
|---|---|---|---|
| P0 | #1 — Limpar `DEFAULT_ARGS` (enable-automation, mock-keychain, password-store) | 30min | Alto (-30% detecções estimado) |
| P0 | #2 — Permissões coerência completa | 1h | Médio |
| P0 | #3 — Adicionar APIs ausentes (bluetooth/usb/serial/hid/locks/wakeLock) | 2h | Médio |
| P1 | #4 — Validator: cross-check proxy ↔ bundle (locale, tz) | 4h | Alto |
| P1 | #5 — Validator: platform ↔ JA3 OS hint | 2h | Médio |
| P1 | #6 — Persistência de canvas seed por (persona, proxy_pool) | 6h | Alto |
| P2 | #7 — Adicionar `wreq` como TLS fallback profile | 6h | Médio (resiliência) |
| P2 | #8 — Tracker blocklist embedded (PGL ~3520 domínios) | 4h | Médio (configurável) |
| P2 | #9 — SSRF guard explícito | 2h | Baixo (segurança/leak) |
| P3 | #10 — Subframe injection race window | 8h | Alto (race door) |
| P3 | #11 — Behavioral signals nos paths principais | 6h | Alto |
| P4 | #12 — `Function.prototype.toString` Proxy edge cases | 2h | Baixo (audit) |

**Total:** ~42h. Critérios e2e ao fim do documento.

---

## Princípios de execução

1. **Implementar item por item, medindo delta.** Não aplicar tudo de uma vez. Após cada P0/P1 rodar `tests/e2e/stealth_probe.rs` (ver §Validação) e comparar `creep_score` / `lie_count`.
2. **Não introduzir abstrações.** Cada item edita arquivos existentes ou adiciona um arquivo único de pequeno escopo.
3. **Sem feature flags novos.** Reusar `stealth.runtime_enable_skip` quando aplicável.
4. **Testes integration > unit.** Stealth é sobre signals; unit test não captura o que detector vê.

---

## P0 — Quick wins (Dia 1, alto impacto)

### Item #1 — Limpar `DEFAULT_ARGS` que vazam automation

**Por quê:** flags puppeteer-default são tells diretos. `enable-automation` ativa `WebDriverEnable` blink feature mais profundo que `AutomationControlled`, vaza via `Runtime.runIfWaitingForDebugger` semantics, `chrome.app.isInstalled` shape, e infobar. `use-mock-keychain` + `password-store=basic` quebram WebCrypto subtle timing e `navigator.credentials` shape — nenhum user real tem.

**Arquivo:** `src/render/chrome/browser/config.rs`

**Mudanças:**
- `DEFAULT_ARGS` (linhas 574-602): remover entradas:
  - `ArgConst::key("enable-automation")` (linha 597)
  - `ArgConst::values("password-store", &["basic"])` (linha 598)
  - `ArgConst::key("use-mock-keychain")` (linha 599)
- Criar segundo array `DEFAULT_ARGS_NON_STEALTH` que mantém os 3 (compat com paths não-stealth, se houver).
- Em `build()` (~linha 482), selecionar via gate existente `self.hidden || self.stealth.runtime_enable_skip`.

**Validação:**
```bash
ps -ef | grep -- '--enable-automation'  # vazio quando stealth ativo
```

**Risco:** baixo. Flags afetam apenas UI/keychain shape — alinhamento com Chrome real é o objetivo.

---

### Item #2 — Permissões coerência completa

**Por quê:** `permissions.query` atualmente coerce só `notifications` e `push`. Real Chrome retorna `'prompt'` (não `'denied'`) para várias outras permissões em insecure context / pré-grant. Detectores cross-check todas.

**Arquivo:** `src/render/stealth_shim.js` linhas 188-202.

**Mudança:** expandir `leaky` map:

```js
const leaky = {
  notifications: 1, push: 1,
  geolocation: 1, camera: 1, microphone: 1,
  'clipboard-read': 1, 'clipboard-write': 1,
  'background-sync': 1, 'background-fetch': 1,
  'persistent-storage': 1, 'screen-wake-lock': 1,
  'accelerometer': 1, 'gyroscope': 1, 'magnetometer': 1,
  'ambient-light-sensor': 1,
};
```

Para `notifications`/`push` mantém `Notification.permission`. Para os outros: retornar `{state: 'prompt'}`. Para `midi`, `storage-access`: `{state: 'granted'}` (default real Chrome).

**Validação:** probe page que itera `await navigator.permissions.query({name})` para cada nome — todos retornam estado plausível, nenhum retorna `'denied'` salvo onde real Chrome também retorna.

---

### Item #3 — Adicionar APIs ausentes

**Por quê:** Chrome 120+ desktop expõe `navigator.bluetooth`, `usb`, `serial`, `hid`, `locks`, `wakeLock` mesmo quando feature-gated. Ausência = mismatch com UA "desktop Chrome".

**Arquivo:** `src/render/stealth_shim.js` — **nova seção 30** antes do `})()` final.

**Conteúdo:**
```js
// Section 30 — Hardware-API surfaces. Persona-gated por GPU_VENDOR_KEYWORD:
// mobile adreno persona suprime estas porque mobile Chrome real não envia.
safe(() => {
  if ('{{GPU_VENDOR_KEYWORD}}' === 'adreno') return;

  if (!('bluetooth' in navigator)) {
    Object.defineProperty(navigator, 'bluetooth', {
      get: () => ({
        requestDevice: () => Promise.reject(new DOMException('NotFoundError')),
        getAvailability: () => Promise.resolve(false),
        addEventListener(){}, removeEventListener(){},
      }),
      configurable: true,
    });
  }

  if (!('usb' in navigator)) {
    Object.defineProperty(navigator, 'usb', {
      get: () => ({
        getDevices: () => Promise.resolve([]),
        requestDevice: () => Promise.reject(new DOMException('NotFoundError')),
        addEventListener(){}, removeEventListener(){},
      }),
      configurable: true,
    });
  }

  if (!('serial' in navigator)) {
    Object.defineProperty(navigator, 'serial', {
      get: () => ({
        getPorts: () => Promise.resolve([]),
        requestPort: () => Promise.reject(new DOMException('NotFoundError')),
        addEventListener(){}, removeEventListener(){},
      }),
      configurable: true,
    });
  }

  if (!('hid' in navigator)) {
    Object.defineProperty(navigator, 'hid', {
      get: () => ({
        getDevices: () => Promise.resolve([]),
        requestDevice: () => Promise.resolve([]),
        addEventListener(){}, removeEventListener(){},
      }),
      configurable: true,
    });
  }

  if (!('locks' in navigator)) {
    Object.defineProperty(navigator, 'locks', {
      get: () => ({
        request: (name, opts, cb) => {
          const callback = typeof opts === 'function' ? opts : cb;
          return Promise.resolve(callback ? callback({name, mode: 'exclusive'}) : undefined);
        },
        query: () => Promise.resolve({held: [], pending: []}),
      }),
      configurable: true,
    });
  }

  if (!('wakeLock' in navigator)) {
    Object.defineProperty(navigator, 'wakeLock', {
      get: () => ({
        request: () => Promise.reject(new DOMException('NotAllowedError')),
      }),
      configurable: true,
    });
  }
});
```

**Validação:** `'bluetooth' in navigator === true` em personas desktop, `false` em adreno mobile. Novos deletes em §17 não conflitam (sensor APIs são distintas).

---

## P1 — Coerência cross-layer (Dia 2-5)

### Item #4 — Validator: cross-check proxy ↔ bundle

**Por quê:** sites avançados (DataDome, PerimeterX) cross-validam `Intl.timeZone` vs IP geo do proxy. Bundle BR-tz + proxy US = bot instantâneo. Validator atual (`src/identity/validator.rs`) não checa proxy.

**Arquivo:** `src/identity/validator.rs`

**Mudanças:**

Adicionar variantes em `ValidationError`:
```rust
#[error("proxy country {proxy_cc} timezone region disagrees with bundle tz {tz}")]
ProxyTimezoneMismatch { proxy_cc: String, tz: String },
#[error("proxy country {proxy_cc} disagrees with locale country {locale_cc}")]
ProxyLocaleCountryMismatch { proxy_cc: String, locale_cc: String },
```

Nova função pública:
```rust
pub fn validate_with_proxy(
    b: &IdentityBundle,
    proxy_country: Option<&str>,
) -> Vec<ValidationError> {
    let mut errs = validate(b);
    if let Some(cc) = proxy_country {
        if let Some(expected) = guess_country_for_tz(&b.timezone) {
            if !expected.eq_ignore_ascii_case(cc) {
                errs.push(ValidationError::ProxyTimezoneMismatch {
                    proxy_cc: cc.to_string(),
                    tz: b.timezone.clone(),
                });
            }
        }
        // locale soft check — VPN users existem, só flag em divergência regional grossa
        let locale_cc = b.locale.split('-').nth(1).unwrap_or("");
        if !locale_cc.is_empty() {
            // ... lógica de "regional cluster" (BR/PT/AR juntos? não.)
        }
    }
    errs
}

fn guess_country_for_tz(tz: &str) -> Option<&'static str> {
    match tz {
        "America/Sao_Paulo" | "America/Recife" | "America/Manaus"
            | "America/Bahia" | "America/Fortaleza" => Some("BR"),
        "America/New_York" | "America/Chicago" | "America/Los_Angeles"
            | "America/Denver" | "America/Phoenix" => Some("US"),
        "America/Toronto" | "America/Vancouver" => Some("CA"),
        "America/Mexico_City" => Some("MX"),
        "America/Buenos_Aires" => Some("AR"),
        "Europe/London" => Some("GB"),
        "Europe/Berlin" => Some("DE"),
        "Europe/Paris" => Some("FR"),
        "Europe/Madrid" => Some("ES"),
        "Europe/Rome" => Some("IT"),
        "Europe/Amsterdam" => Some("NL"),
        "Asia/Tokyo" => Some("JP"),
        "Asia/Shanghai" | "Asia/Hong_Kong" => Some("CN"),
        "Asia/Singapore" => Some("SG"),
        "Asia/Seoul" => Some("KR"),
        "Australia/Sydney" | "Australia/Melbourne" => Some("AU"),
        _ => None,
    }
}
```

**Wiring:** `src/identity/session_registry.rs` — busca por `validate(`. Trocar para `validate_with_proxy(bundle, proxy.country_code())`.

Se proxy module não expõe `country_code()`, adicionar:
- Static lookup table de IP-range → ISO-3166 (MaxMind GeoLite2-Country.mmdb embedded ou download em build.rs).
- Cache em SQLite por `proxy_url` para evitar re-lookup.

**Validação:** unit test `bundle{tz:"America/Sao_Paulo"} + proxy{cc:"US"}` retorna `ProxyTimezoneMismatch`.

---

### Item #5 — Validator: platform ↔ JA3 OS hint

**Por quê:** TLS JA3 carrega OS hint via ALPS extension, ALPN order, GREASE pattern. Bundle Linux platform + Windows TLS profile = imediato.

**Arquivos:** `src/identity/validator.rs` + `src/impersonate/ja3.rs`

**Mudanças:**

Em `src/impersonate/ja3.rs`:
```rust
pub fn os_hint_for_profile(p: Profile) -> &'static str {
    match p {
        Profile::Chrome131Linux => "linux",
        Profile::Chrome131Windows => "windows",
        Profile::Chrome131Mac => "macos",
        // expandir conforme variantes existentes
    }
}
```

(Se o enum `Profile` ainda não distingue por OS, adicionar variantes — atualmente `current_chrome_fingerprint_summary` em ja3.rs:505 trata profiles como wire-config-equivalent.)

Em `src/identity/validator.rs`:
```rust
#[error("bundle platform {platform} disagrees with TLS profile OS {tls_os}")]
PlatformTlsMismatch { platform: String, tls_os: String },
```

Adicionar checagem em `validate()`:
```rust
if let Some(tls_profile) = b.tls_profile.as_ref() {
    let tls_os = ja3::os_hint_for_profile(*tls_profile);
    let platform_os = match b.platform.as_str() {
        p if p.starts_with("Linux") => "linux",
        p if p.starts_with("Win")   => "windows",
        p if p.contains("Mac")      => "macos",
        _ => "",
    };
    if !platform_os.is_empty() && tls_os != platform_os {
        errs.push(ValidationError::PlatformTlsMismatch { ... });
    }
}
```

**Pré-requisito:** `IdentityBundle.tls_profile: Option<Profile>` — adicionar field se ausente; popular em `from_chromium`.

**Validação:** unit test `Linux platform + Profile::Win` retorna erro.

---

### Item #6 — Persistência de canvas seed por (persona, proxy_pool)

**Por quê:** real users têm fingerprint **estável por meses**. Nosso `canvas_audio_seed = session_seed` rotaciona por sessão. Detector com cohort tracking vê "mesmo cookie + mesmo IP + canvas_hash diferente" = bot.

**Arquivos:** `src/identity/session_registry.rs` + `src/storage/sqlite.rs` + `src/identity/bundle.rs`

**Mudanças:**

Nova tabela em `sqlite.rs`:
```sql
CREATE TABLE IF NOT EXISTS persona_seeds (
    persona_key       TEXT PRIMARY KEY,
    canvas_audio_seed INTEGER NOT NULL,
    created_at_unix   INTEGER NOT NULL,
    last_used_unix    INTEGER NOT NULL,
    contaminated      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_persona_seeds_last_used
    ON persona_seeds(last_used_unix);
```

`persona_key` formato: `"{os}-{gpu}-{locale}-{proxy_pool_id}"` (ex: `"linux-intel-en-us-pool-residential-br"`).

Em `IdentityBundle::from_chromium`, refatorar para receber `persona_key: &str` e seed store handle:
```rust
pub fn from_chromium(
    major: u32,
    session_seed: u64,
    persona_key: &str,
    seed_store: &dyn PersonaSeedStore,
) -> Self {
    let canvas_audio_seed = match seed_store.get(persona_key) {
        Some(s) if !seed_store.is_contaminated(persona_key) => s,
        _ => {
            let s = derive_seed_from_key(persona_key, session_seed);
            seed_store.upsert(persona_key, s);
            s
        }
    };
    // ...
}
```

`derive_seed_from_key`: SipHash13 do `persona_key` mixado com `session_seed`. Determinístico por key, único por proxy_pool.

Lifecycle:
- **Reset:** quando antibot pipeline marca `SessionState::Contaminated` (já existe), chamar `seed_store.mark_contaminated(persona_key)`.
- **Reset por TTL:** seed > 30 dias é regenerado (rotação natural).
- **Reset por persona change:** mudança explícita de persona regenera.

`PersonaSeedStore` trait com impl SQLite default, in-memory para testes.

**Validação:**
- Mesmo `persona_key` em duas chamadas consecutivas retorna mesmo seed.
- `mark_contaminated` + nova chamada retorna seed novo.
- Integration test: dois bundles na mesma session com mesmo persona_key produzem mesmo `creep_score` para fingerprint estável.

**Risco:** médio. Afeta entrails de identidade. Cobrir com integration test que valida double-fingerprint equality cross-session.

---

## P2 — Defesa contra TLS drift (Semana 2)

### Item #7 — `wreq` como fallback TLS profile

**Por quê:** `wreq` 6.0 (curl-impersonate-rs descendente) auto-atualiza Chrome version emulation. Nosso BoringSSL tuned manual em `src/impersonate/tls.rs` (450 linhas) precisa atualização toda Chrome release. Drift silencioso = JA3 desatualizado = soft block.

**Arquivos:** `Cargo.toml`, `src/impersonate/mod.rs`, **novo** `src/impersonate/wreq_client.rs`

**Mudanças `Cargo.toml`:**
```toml
[features]
wreq-fallback = ["dep:wreq", "dep:wreq-util"]

[dependencies]
wreq = { version = "6", optional = true, features = ["prefix-symbols"] }
wreq-util = { version = "3", optional = true }
```

**Novo `src/impersonate/wreq_client.rs`** (~150 linhas), shape espelhando referência `crates/obscura-net/src/wreq_client.rs` no clone Obscura (`/tmp/obscura-research/obscura`):
- `pub struct WreqClient { client: wreq::Client, cookie_jar: Arc<CookieJar>, ... }`
- Builder usa `wreq_util::EmulationOption::builder()`:
  ```rust
  .emulation(wreq_util::Emulation::Chrome145)
  .emulation_os(match os {
      "linux"   => wreq_util::EmulationOS::Linux,
      "windows" => wreq_util::EmulationOS::Windows,
      "macos"   => wreq_util::EmulationOS::MacOS,
      _ => wreq_util::EmulationOS::Linux,
  })
  ```
- `pub async fn fetch(&self, url: &Url) -> Result<Response>` — mesma trait que client atual.
- Cookie jar shared (não duplicar storage).

**Wiring `src/impersonate/mod.rs`:**
```rust
pub enum BackendKind { Boring, Wreq }

pub struct ImpersonateClient {
    backend: BackendImpl,
    // ...
}

enum BackendImpl {
    Boring(BoringClient),
    #[cfg(feature = "wreq-fallback")]
    Wreq(WreqClient),
}
```

`Config::tls_backend: BackendKind` (default Boring para zero break).

**Operação:** A/B rotation 70/30 boring/wreq, controlado por env var ou config. Métricas: success rate por backend por target site.

**Validação:**
- `cargo build --features wreq-fallback` compila.
- Integration test contra `https://tls.peet.ws/api/all` retorna JA3 hash diferente entre os dois backends, ambos válidos Chrome.
- Smoke test contra um site DataDome-protected — wreq passa onde boring é blocked (se aplicável).

---

### Item #8 — Tracker blocklist embedded

**Por quê:** real Chrome user típico tem uBlock/Brave Shields filtrando trackers. Browser que carrega 100% dos requests sem filtro é signal "automation/no extension". Configurável: pode estar OFF se persona for "vanilla user".

**Arquivos:** **novo** `src/impersonate/blocklist.rs` (~80 linhas) + **novo** `src/impersonate/pgl_domains.txt` (~60KB) + `Config`

**Conteúdo `blocklist.rs`:**
```rust
use std::collections::HashSet;
use std::sync::OnceLock;

const PGL_LIST: &str = include_str!("pgl_domains.txt");

fn blocklist() -> &'static HashSet<&'static str> {
    static BL: OnceLock<HashSet<&str>> = OnceLock::new();
    BL.get_or_init(|| {
        PGL_LIST.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect()
    })
}

pub fn is_blocked(host: &str) -> bool {
    let bl = blocklist();
    if bl.contains(host) { return true; }
    let mut domain = host;
    while let Some(pos) = domain.find('.') {
        domain = &domain[pos + 1..];
        if bl.contains(domain) { return true; }
    }
    false
}
```

**Populate `pgl_domains.txt`:** download de https://pgl.yoyo.org/adservers/serverlist.php?showintro=0&mimetype=plaintext (formato hostnames). Versionar no repo. Adicionar `scripts/update_blocklist.sh` que faz GET + diff check.

**Modos configuráveis em `Config`:**
```rust
pub enum BlockMode {
    Off,                       // default, alinhamento com vanilla user
    BlockKnown,                // bloqueia domínios PGL
    BlockKnownAndUnknown3p,    // PGL + heuristic third-party block
}

pub block_trackers: BlockMode,
```

**Wiring point:** em `ImpersonateClient::fetch` antes de enviar request:
```rust
if let BlockMode::BlockKnown | BlockMode::BlockKnownAndUnknown3p = self.block_trackers {
    if let Some(host) = url.host_str() {
        if blocklist::is_blocked(host) {
            return Ok(Response::blocked(url));  // status 0, empty body
        }
    }
}
```

Para CDP path, hook em `Fetch.requestPaused` handler, retornar `Fetch.failRequest{ errorReason: "BlockedByClient" }` (mimicar uBlock).

**Validação:**
- Request para `googletagmanager.com` em modo `BlockKnown` aborta com status 0.
- Modo `Off`: passa.
- Telemetry: counter `trackers_blocked` por session.

---

### Item #9 — SSRF guard explícito

**Por quê:** redirect chains podem apontar para metadata endpoints (AWS 169.254.169.254), localhost services, internal IPs. Sem guard, server-side fetch path leak interno. Obscura tem isso em `validate_url`.

**Arquivo:** `src/impersonate/mod.rs` (ou onde `send()` mora)

**Adicionar:**
```rust
fn validate_url_external(url: &Url) -> Result<(), Error> {
    let scheme = url.scheme();
    if !matches!(scheme, "http" | "https") {
        return Err(Error::Forbidden(format!("scheme {scheme}")));
    }
    if let Some(host) = url.host() {
        match host {
            url::Host::Ipv4(ip) => {
                if ip.is_loopback() || ip.is_private() || ip.is_link_local()
                    || ip.is_broadcast() || ip.is_documentation() {
                    return Err(Error::Forbidden(format!("private IPv4 {ip}")));
                }
            }
            url::Host::Ipv6(ip) => {
                if ip.is_loopback() || ip.is_unicast_link_local() {
                    return Err(Error::Forbidden(format!("private IPv6 {ip}")));
                }
            }
            url::Host::Domain(d) => {
                let l = d.to_lowercase();
                if l == "localhost" || l.ends_with(".localhost")
                    || l == "127.0.0.1" || l == "::1" {
                    return Err(Error::Forbidden(format!("localhost {d}")));
                }
            }
        }
    }
    Ok(())
}
```

**Chamar em:**
- Toda entrada de `fetch()`.
- Após cada `Location:` header em redirect chain follow.
- CDP `Fetch.continueRequest` se URL foi rewritten.

**Validação:**
- Request para `http://169.254.169.254/latest/meta-data/` retorna `Forbidden`.
- Redirect chain `https://example.com → http://localhost:8080` aborta no segundo hop.

---

## P3 — Behavioral & frame coverage (Semana 2-3)

### Item #10 — Subframe injection race window

**Por quê:** `src/render/pool.rs:990` faz polling de 250ms para detectar novos targets. Nessa janela, subframe pode executar JS sem shim. Detector que faz `requestAnimationFrame(()=>checkWebdriver())` no primeiro frame vê window não-shimado.

**Arquivos:** `src/render/pool.rs`, `src/render/chrome/handler/target.rs`, `src/render/chrome/browser/config.rs`

**Mudanças:**

1. **Trocar polling por evento.** `target.rs:264` já tem `on_attached_to_target`. Expor mpsc channel:
   ```rust
   pub struct TargetEventChannel {
       tx: mpsc::UnboundedSender<TargetAttachedEvent>,
   }
   pub struct TargetAttachedEvent {
       pub target_id: TargetId,
       pub target_type: String,
       pub session_id: SessionId,
   }
   ```

2. **No handler:** ao receber `Target.attachedToTarget`, enviar evento ANTES de processar mais nada.

3. **Em `pool.rs`:** trocar `tokio::spawn(async move { loop { sleep(250ms); pages(); } })` por:
   ```rust
   tokio::spawn(async move {
       while let Some(ev) = target_rx.recv().await {
           if ev.target_type == "page" || ev.target_type == "iframe" {
               let install = AddScriptToEvaluateOnNewDocumentParams {
                   source: shim.clone(), ...
               };
               // executar SINCRONAMENTE antes do resume
               page.execute(install).await?;
           }
           // se waitForDebuggerOnStart=true, agora resume:
           page.execute(RuntimeRunIfWaitingForDebuggerParams::default()).await?;
       }
   });
   ```

4. **Em `config.rs` (build setAutoAttach):** trocar para `wait_for_debugger_on_start: true`. Isso pausa subframe em boot até CDP enviar `Runtime.runIfWaitingForDebugger` após shim install.

5. **Fallback timeout:** 5s — se shim install falha, log error e resume mesmo assim (preferir blocked-but-functional sobre deadlock).

**Validação:**
- Integration test: cria iframe via `document.body.appendChild(document.createElement('iframe'))`, iframe roda `navigator.webdriver`, asserta retorna `undefined` no primeiro tick (sem sleep no test).
- Stress test: 10 iframes simultâneos, todos vêem shim instalado.

**Risco:** alto. `waitForDebuggerOnStart=true` pode deadlockar se shim install falha. Manter timeout 5s + path de fallback.

---

### Item #11 — Behavioral signals nos paths principais de crawl

**Por quê:** temos `src/render/motion/` e `src/render/keyboard/bimodal.rs`. Mas `crawler.rs` provavelmente faz navigate direto + extract sem motion replay (grep não mostra `interact::*` em `crawler.rs`). DataDome 2024 pesa behavioral 40%+: sem mouse/scroll/dwell, score cai mesmo com fingerprint perfeito.

**Arquivos:** `src/crawler.rs`, `src/render/handoff.rs`, `src/render/actions.rs`

**Mudanças:**

1. **Auditoria:** mapear cada call site de `navigate` em `crawler.rs`. Após navigate bem-sucedido, antes de extract:
   ```rust
   crate::render::interact::spawn_idle_drift(
       page.clone(),
       initial_mouse_pos,
       idle_state.clone(),
   );
   ```

2. **Antes de qualquer click/extract:**
   ```rust
   pos = crate::render::interact::move_mouse_to_target(page, target_pos, pos).await?;
   ```

3. **Para páginas longas:** simular scroll bimodal:
   ```rust
   crate::render::interact::scroll_by(page, ScrollParams::natural_for(viewport_h)).await?;
   ```

4. **Nova fn em `crawler.rs`:**
   ```rust
   async fn simulate_human_dwell(
       page: &Page,
       seed: u64,
       dwell_ms_range: std::ops::Range<u64>,
   ) -> Result<()> {
       // motion engine + idle drift + opcional scroll
       // duração derivada de seed para determinismo
   }
   ```
   Chamar após cada `navigate_with_wait`.

5. **Guard no startup:**
   ```rust
   if config.stealth_enabled && config.motion_profile.is_off() {
       tracing::warn!("stealth ligado mas motion_profile=Off — recomendado: Slow ou Realistic");
   }
   ```

**Validação:**
- SQLite telemetry: counter `mouse_events_dispatched > 0` em cada crawl run com stealth ativo.
- Audit log: `engine.movements_executed` por sessão.
- Integration test: roda probe que mede mouse event count via `document.addEventListener('mousemove', ...)` antes de extract — esperar ≥ 5 movements.

---

## P4 — Hardening final (Semana 3)

### Item #12 — `Function.prototype.toString` Proxy edge cases

**Por quê:** nossa Proxy em `stealth_shim.js:798-806` lida com `apply` trap. Detectores avançados probam outras superfícies do Proxy.

**Arquivo:** `src/render/stealth_shim.js`

**Probes a defender:**
```js
Object.prototype.toString.call(Function.prototype.toString)
// real Chrome: "[object Function]"
// Proxy de função (callable target): "[object Function]" — OK
Reflect.getPrototypeOf(Function.prototype.toString) === Function.prototype
// real Chrome: true
Function.prototype.toString.length  // 0
Function.prototype.toString.name    // "toString"
```

**Probes que podem quebrar:**
```js
const f = Function.prototype.toString;
f.toString.call(f)  // recursivo — Proxy hits apply, thisArg === proxiedToString → linha 802 retorna FAKE — OK
```

**Mudança defensiva:** adicionar `getPrototypeOf` e `get` traps ao Proxy se algum probe falhar:
```js
const proxiedToString = new Proxy(nativeToString, {
  apply(target, thisArg, args) {
    if (targets.has(thisArg)) return FAKE;
    if (thisArg === proxiedToString) return FAKE;
    try { return Reflect.apply(target, thisArg, args); } catch (_) { return FAKE; }
  },
  get(target, prop, receiver) {
    // Forward 'name', 'length', etc. para o native original.
    return Reflect.get(target, prop, receiver);
  },
  getPrototypeOf(target) {
    return Reflect.getPrototypeOf(target);
  },
});
```

**Adicionar test em CI:** crawl probe page (self-hosted ou `tls.peet.ws`-style) que executa os 4 checks e reporta. Se algum falha, refinar.

**Validação:** test page retorna `{nativeOK: true, prototypeOK: true, toStringOnSelfOK: true, lengthOK: true, nameOK: true}`.

---

## Validação fim-a-fim

### `tests/e2e/stealth_probe.rs`

**Setup:** levanta browser stealth mode, navega para CreepJS.

**Steps:**
1. `let browser = Browser::launch_stealth().await?;`
2. `let page = browser.new_page().await?;`
3. `page.navigate("https://abrahamjuliot.github.io/creepjs/").await?;` (ou self-hosted clone — recomendado para evitar dependência de URL externa).
4. Esperar `#fingerprint` selector.
5. Extrair via `page.evaluate`:
   - `creep.scoreData.lies.length` → `lie_count`
   - `creep.scoreData.score` → `score`
   - `creep.fingerprint.canvas.dataURI` → `canvas_hash`

**Assertions:**
- `lie_count <= 3`
- `score >= 75` (max 100; threshold conservador)
- `canvas_hash` estável entre dois loads na mesma sessão (double-render equality)

### `tests/e2e/datadome_probe.rs`

**Setup:** browser stealth, navega para endpoint público com DataDome ativo.

**Steps:**
1. Lista de URLs DD-protected conhecidas (ex: alguns sites e-commerce, airline).
2. Para cada URL: navigate + wait_until=domcontentloaded.
3. Verificar:
   - Body contém conteúdo real (e.g., "<title>" não vazio, body length > 5KB).
   - Body NÃO contém interstitial markers ("captcha", "datadome.co/captcha", "blocked").

**Assertion:** ≥ 80% dos sites passam (alguns DD são extra-strict, threshold realista).

### Métricas de progresso

Rodar ambos antes e depois de cada item. Log em `production-validation/stealth_progress.csv`:

```csv
date,item_completed,creep_score_avg,lie_count_avg,dd_pass_rate,wpa_pass_rate
2026-04-25,baseline,72,5,55%,n/a
2026-04-25,#1-DEFAULT_ARGS,79,3,68%,n/a
...
```

Esperado após P0 completo: `creep_score >= 80`, `lie_count <= 4`, `dd_pass_rate >= 65%`.
Esperado após P1 completo: `creep_score >= 85`, `lie_count <= 3`, `dd_pass_rate >= 75%`.
Esperado após P2-P3 completo: `creep_score >= 90`, `lie_count <= 2`, `dd_pass_rate >= 85%`.

---

## Notas operacionais

- **Não aplicar tudo de uma vez.** Itens #1, #2, #3 isolados primeiro pra medir delta. Se taxa de detecção cair muito, talvez #4-#7 já não sejam urgentes (rever priorização).
- **`enable-automation` é o suspeito #1.** DataDome 2024 detecta esse flag via cross-check de `chrome.app.isInstalled` shape + Runtime.runIfWaitingForDebugger semantics. Highest-ROI fix.
- **#6 (seed persistence)** só importa se há retries no mesmo site/persona. Se telemetria mostra crawls one-shot, deprioriza.
- **#7 (wreq)** é defesa contra drift, não ataque imediato. Importante para longevidade do TLS profile.
- **#10 (race window) e #11 (behavioral)** são os que mexem em paths críticos. Cobrir com integration tests robustos antes de merge.

---

## Apêndice — referências de código

**Repo Obscura clonado:** `/tmp/obscura-research/obscura` (commit `99e75f1`, shallow).

**Arquivos-chave nossos auditados:**
- `src/render/stealth_shim.js` (1686 linhas, 29 seções)
- `src/render/stealth.rs` (528 linhas, ShimVars + render_shim_from_bundle)
- `src/render/chrome/browser/config.rs` (DEFAULT_ARGS:574, build:482, gate:549)
- `src/identity/validator.rs` (898 linhas, validate())
- `src/identity/bundle.rs` (562 linhas, IdentityBundle + canvas_audio_seed:71)
- `src/identity/profiles.rs` (424 linhas, PersonaProfile + 5-row catalog)
- `src/render/pool.rs` (poll loop:990, setAutoAttach:957)
- `src/impersonate/tls.rs` (450 linhas, BoringSSL connector)
- `src/impersonate/ja3.rs` (597 linhas, ClientHello + Profile)
- `src/render/motion/` (device, fatigue, idle, scroll, submovement, touch)
- `src/render/keyboard/bimodal.rs`
- `src/crawler.rs` (sem chamadas a `interact::*` no path direto)
- `src/antibot/` (586 mod.rs + signatures + bypass + cookie_pin + solver + telemetry)

**Referências externas:**
- CreepJS: https://abrahamjuliot.github.io/creepjs/
- FingerprintJS: https://github.com/fingerprintjs/fingerprintjs
- Camoufox notes: https://camoufox.com/
- PGL blocklist: https://pgl.yoyo.org/adservers/
- curl-impersonate: https://github.com/lwthiker/curl-impersonate
- wreq (curl-impersonate-rs): https://github.com/wreq-rs/wreq
