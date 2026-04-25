# Fase 1 — Antibot/Stealth de verdade

Meta: sair de "detecta challenge e escala" pra "mantém sessão limpa, detecta sinais cedo e reage com contexto".

## Entregáveis

### 1. `ChallengeState` por session/browser-context

Novo módulo `src/antibot/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChallengeLevel {
    Suspected,         // sinais fracos (rate limit, header estranho)
    ChallengePage,     // interstitial inteiro (CF JS challenge)
    WidgetPresent,     // Turnstile/hCaptcha/reCAPTCHA visível na página
    HardBlock,         // 403/429 definitivo, body curto, vendor identificado
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChallengeVendor {
    CloudflareJsChallenge,
    CloudflareTurnstile,
    Recaptcha,
    RecaptchaEnterprise,
    HCaptcha,
    DataDome,
    PerimeterX,
    Akamai,
    GenericCaptcha,
    AccessDenied,
}

#[derive(Debug, Clone)]
pub struct ChallengeSignal {
    pub vendor: ChallengeVendor,
    pub level: ChallengeLevel,
    pub url: url::Url,
    pub origin: String,
    pub proxy: Option<url::Url>,
    pub session_id: String,
    pub first_seen: std::time::SystemTime,
    pub metadata: serde_json::Value,  // vendor-specific details
}

pub fn detect_from_html(html: &str, url: &url::Url, headers: Option<&http::HeaderMap>) -> Option<ChallengeSignal>;
pub fn detect_from_http_response(status: u16, body: &[u8], headers: &http::HeaderMap, url: &url::Url) -> Option<ChallengeSignal>;
pub fn detect_from_cdp_cookies(cookies: &[Cookie], url: &url::Url) -> Option<ChallengeSignal>;
```

### 2. Vendor signatures (6 mínimos)

Fixtures em `tests/antibot_fixtures/*.html`:
- `cloudflare_jschallenge.html` — `<title>Just a moment...</title>` + `cf-chl-bypass` + script `/cdn-cgi/challenge-platform/`
- `cloudflare_turnstile.html` — `iframe[src*="challenges.cloudflare.com/turnstile"]` + `data-sitekey`
- `recaptcha_enterprise.html` — `script[src*="recaptcha/enterprise.js"]` + `div[class*="grecaptcha"]`
- `hcaptcha.html` — `iframe[src*="hcaptcha.com"]` OU `script[src*="hcaptcha.com/1/api.js"]`
- `datadome.html` — `iframe[src*="captcha-delivery.com"]` OU cookie `datadome`
- `perimeterx.html` — `div#px-captcha` OU script `client.perimeterx.net`
- `akamai.html` — `script[src*="/akam/"]` + header `Server: AkamaiGHost`

Detecção: heurística por DOM selectors (regex leve, não parser completo) + header inspection + cookie names.

### 3. Detecção no render path

Em `src/render/pool.rs` pós-`settle_after_actions`:
- Rodar `detect_from_html` contra `html_post_js`
- Rodar `detect_from_cdp_cookies` via `Storage.getCookies` se nenhum DOM hit mas cookies suspeitos
- Se hit: setar `RenderedPage::challenge: Option<ChallengeSignal>`, capturar snapshot adicional (HTML pós-JS + screenshot FullPage) com prefix `challenge_<vendor>_`, emitir `decision.made` com `why=antibot:<vendor>:<level>:render_detected`

### 4. Detecção no HTTP path

Em `src/crawler.rs` pós-HTTP fetch:
- Rodar `detect_from_http_response(status, body, headers, url)`
- Hit → `ChallengeSignal` + Policy engine vira `Decision::Render` (escalate) OU `Decision::SwitchProxy` dependendo de vendor/level

### 5. Telemetria SQLite

Nova tabela em `src/storage/sqlite.rs`:

```sql
CREATE TABLE IF NOT EXISTS challenge_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    vendor TEXT NOT NULL,
    level TEXT NOT NULL,
    url TEXT NOT NULL,
    origin TEXT NOT NULL,
    proxy TEXT,
    observed_at INTEGER NOT NULL,
    metadata TEXT
);
CREATE INDEX IF NOT EXISTS idx_challenge_session ON challenge_events(session_id);
CREATE INDEX IF NOT EXISTS idx_challenge_vendor ON challenge_events(vendor);
CREATE INDEX IF NOT EXISTS idx_challenge_observed ON challenge_events(observed_at);
```

API em `Storage` trait (ou inherent em `SqliteStorage`):
- `async fn record_challenge(&self, signal: &ChallengeSignal) -> Result<()>`
- `async fn session_challenges(&self, session_id: &str) -> Result<Vec<ChallengeSignal>>`

### 6. Session marking + Policy

`SessionIdentity` ganha field `state: SessionState`:

```rust
pub enum SessionState {
    Clean,
    Warm,
    Contaminated,  // pelo menos 1 challenge Suspected/ChallengePage
    Blocked,       // HardBlock em rota crítica
}
```

Policy de sessão orientada por `ChallengeSignal` (em `src/policy/engine.rs`, novo `decide_post_challenge`):

```rust
pub enum SessionAction {
    ReuseSession,
    RotateProxy,    // consulta ProxyRouter::best_alternative()
    KillContext,    // drop BrowserContext + cookies, nova session
    ReopenBrowser,  // respawn Browser inteiro
    GiveUp,         // drop URL, quarantine host
}

pub fn decide_post_challenge(signal: &ChallengeSignal, session: &SessionState, proxy: Option<&Url>) -> SessionAction;
```

Regras iniciais (conservadoras):
- `Suspected` → `RotateProxy` (troca proxy, mantém session)
- `ChallengePage` + `Clean` → `KillContext` (abre nova)
- `ChallengePage` + `Warm/Contaminated` → `ReopenBrowser`
- `WidgetPresent` → `KillContext` (a gente não resolve captcha)
- `HardBlock` → `GiveUp` + marca host quarantined no ProxyRouter

### 7. `record_outcome(ChallengeHit)` no ProxyRouter

Ponta frouxa da Fase 4.3: quando challenge detecta, alimentar `router.record_outcome(proxy, ProxyOutcome::ChallengeHit)` pro score penalizar proxy associado.

### 8. Events estruturados

`src/events/kinds.rs` — novo variant ou campo em `DecisionMade`:

```rust
ChallengeDetected {
    vendor: ChallengeVendor,
    level: ChallengeLevel,
    url: Url,
    session_id: String,
    session_action: SessionAction,
}
```

Emitir em render path + HTTP path quando challenge detectar.

## Checklist

- [x] Criar `src/antibot/mod.rs` com types + 6 vendor signatures + 3 detect functions
- [x] Fixtures HTML em `tests/antibot_fixtures/*.html` (6 vendors)
- [x] Unit tests `tests/antibot_detection.rs` — cada fixture matcha vendor correto; false-positive tests (HTML inocente não vira challenge)
- [x] Schema SQLite `challenge_events` + `record_challenge`/`session_challenges` API
- [x] Wire detect no HTTP path (`src/crawler.rs`)
- [x] Wire detect no render path (`src/render/pool.rs`)
- [x] Screenshots/snapshots extras quando challenge: `challenge_<vendor>_<session_id>.png|.html`
- [x] `SessionState` enum + integração no `SessionIdentity`
- [x] `decide_post_challenge` em `src/policy/engine.rs`
- [x] Feed `ChallengeHit` outcome no ProxyRouter
- [x] `ChallengeDetected` event + emit nos 2 paths
- [?] `render_outcome` no render path (ponta frouxa 4.3) — ChallengeHit wired; full lifecycle→outcome mapping deferred: success/latency already flow via HTTP fetcher; render path doesn't thread timing yet. Non-blocking for Phase 1.
- [x] Build + clippy + test all-features + mini verdes
- [x] Live HN test continua PASS (~32.7s)
- [x] Output `.dispatch/tasks/phase1-antibot/output.md`

## Restrições
- Sem solver externo de CAPTCHA — só detecta + escalona.
- Sem fetch interceptor (Fase 2 não landed).
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde (antibot compila sem cdp-backend — detect_from_html + detect_from_http_response são pure).
- Signatures conservadoras: falso-positivo é pior que falso-negativo no crawl. Prefira especificidade.
- Sem commits.
- Render path detection só com `cdp-backend` feature.
- Não tocar Fase 2 (runtime ScriptSpec) nem Fase 3 (SPA).

## Arquivos críticos
- `src/antibot/mod.rs` (novo)
- `src/lib.rs` — `pub mod antibot`
- `src/render/pool.rs`
- `src/crawler.rs`
- `src/policy/engine.rs`
- `src/storage/sqlite.rs`
- `src/storage/mod.rs` (trait extension)
- `src/identity/bundle.rs` — `SessionIdentity` ganha `state`
- `src/events/kinds.rs`
- `src/proxy/router.rs` — já tem `ChallengeHit` outcome, só wire-up
- `tests/antibot_detection.rs` (novo)
- `tests/antibot_fixtures/*.html` (6 novos)
