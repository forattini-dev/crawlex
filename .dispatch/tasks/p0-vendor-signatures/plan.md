# P0 — Vendor signatures expansion (sensor_data + PX320-348 + telemetry shapes)

Meta: expandir detecção de antibot vendors com shapes reais dos payloads que detectores coletam/enviam, não só DOM signatures. Melhora classificação + habilita telemetria rica pra decisões de proxy/session. Referência: `research/evasion-deep-dive.md#5` (vendor deep-dives) + `research/evasion-actionable-backlog.md` P0-9.

## P0 coverage

- **P0-9** Vendor coverage expansion — capturar shapes reais de sensor/telemetry além de DOM signatures

## Contexto

Fase 1 antibot já detecta via DOM signatures + headers + cookies pros 10 vendors. Gap: não sabemos **o que o vendor está medindo** quando ativo. Ex:
- Akamai Bot Manager posta `sensor_data` v1.7/v2 pra `/_bm/sensor` — shape específico com touch events, mouse events, typing, screen, fingerprint
- PerimeterX chama `_px2/:event_id` com payload codificado — IDs de sinais PX320-PX348 mapeiam pra features específicas
- DataDome faz CDP-like probe via `ddg.js` — testa `navigator.webdriver`, CDP leak timing
- Cloudflare Turnstile chama `/cdn-cgi/challenge-platform/h/g/cv/result/` com telemetry

## Entregáveis

### 1. `src/antibot/telemetry.rs` (novo)

Observer passivo de requests outbound que o site faz para vendors conhecidos:
```rust
pub struct VendorTelemetry {
    pub vendor: ChallengeVendor,
    pub endpoint: Url,
    pub method: String,
    pub payload_size: usize,
    pub payload_shape: PayloadShape,  // inferred from Content-Type + structure
    pub observed_at: SystemTime,
    pub session_id: String,
}

pub enum PayloadShape {
    AkamaiSensorDataV1_7 { keys_found: Vec<String> },  // e.g. ["bmak.sensor_data", "bmak.d"]
    AkamaiSensorDataV2 { sbsd_ek: Option<String> },
    PerimeterXCollector { event_ids: Vec<String> }, // PX320, PX333, etc observed
    DataDomeReport { signal_count: usize },
    CloudflareChallenge { tk: Option<String> },
    HCaptchaExecute { sitekey: Option<String> },
    RecaptchaReload { k: Option<String>, v: Option<String> },
    Unknown,
}
```

Hooked via CDP `Network.requestWillBeSent` events — já escutados pelo crawlex. Filtrar por URL patterns dos vendors.

### 2. Expand vendor URL/endpoint coverage

Pra cada vendor, mapear URLs/domínios que sinalizam atividade:
- Akamai: `*.akamaihd.net/*`, `/_bm/*`, `/_sec/*`, request to `_abck` cookie setter
- PerimeterX: `client.perimeterx.net`, `/api/v2/collector/*`, `*.px-cloud.net`
- DataDome: `js.datadome.co`, `api.datadome.co`, `captcha-delivery.com`, `*.datado.me`
- Cloudflare: `/cdn-cgi/challenge-platform/*`, `challenges.cloudflare.com/turnstile/*`
- hCaptcha: `js.hcaptcha.com`, `api.hcaptcha.com`, `hcaptcha.com/checkcaptcha/*`
- reCAPTCHA: `www.google.com/recaptcha/*`, `www.recaptcha.net/recaptcha/*`
- Imperva: `*.incapdns.net`, `/_Incapsula_Resource`
- Kasada: domains with `x-kpsdk-*` headers
- F5 Shape: cookies `TS*`

Mapear em `src/antibot/signatures.rs` ou estender `src/antibot/mod.rs`.

### 3. PerimeterX signal ID catalog

Research `research/evasion-deep-dive.md#5` lista IDs PX320-PX348. Criar catalog:
```rust
pub struct PxSignal {
    pub id: &'static str,     // "PX320"
    pub name: &'static str,   // "CDP detection"
    pub detection: &'static str, // what they measure
}
pub const PX_SIGNALS: &[PxSignal] = &[ /* 29 entries */ ];
```

Observar em payloads → `PayloadShape::PerimeterXCollector { event_ids }` diz quais signals o vendor observou.

### 4. Akamai sensor_data shape tracker

sensor_data v1.7: chave `bmak.sensor_data`. v2: chave `sbsd_ek`. Shape tem N campos (touch/mouse/typing/screen/etc).

Parse parcial — não decodificar (é ofuscado) mas contar chaves/shape:
```rust
pub struct AkamaiSensorInfo {
    pub version: AkamaiVersion,
    pub payload_len: usize,
    pub top_level_keys: Vec<String>,  // e.g. ["bmak.sensor_data"]
    pub likely_fields: Vec<AkamaiField>,  // parsed heuristically
}

pub enum AkamaiField { MouseEvents, TouchEvents, Typing, Screen, Sensor, Fingerprint }
```

### 5. Telemetry persist em SQLite

Nova tabela:
```sql
CREATE TABLE IF NOT EXISTS vendor_telemetry (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    vendor TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    method TEXT NOT NULL,
    payload_size INTEGER,
    payload_shape TEXT,
    observed_at INTEGER NOT NULL
);
CREATE INDEX idx_vendor_telem_session ON vendor_telemetry(session_id);
CREATE INDEX idx_vendor_telem_vendor ON vendor_telemetry(vendor);
```

### 6. Event + decision hook

`EventKind::VendorTelemetryObserved { vendor, endpoint, shape }` emit via bus.

Policy engine: se telemetry volume cresce acima de threshold (ex: >20 posts em 30s pra mesmo vendor) → `SessionAction::RotateProxy` preventivo, antes de HardBlock acontecer.

### 7. Testes

- `tests/vendor_telemetry.rs` non-ignored: fixtures de payloads reais (pequenos amostras) pra cada vendor → classifier retorna `PayloadShape` correto
- PX signal catalog coverage (catalog size match research spec)

## Checklist

- [x] **URL patterns por vendor**: expandir maps em `src/antibot/` com endpoints telemetry pra 10 vendors
- [x] **`VendorTelemetry` + `PayloadShape`** types
- [x] **PerimeterX PX signal catalog** (29 IDs do research)
- [x] **Akamai sensor_data parcial parser** — version detection + key extraction
- [x] **Observer no render path**: hook em `Network.requestWillBeSent` já escutado — filtra por vendor patterns, classifica, emit evento + persiste
- [x] **SQLite `vendor_telemetry` table** + persist
- [x] **Policy hook**: high-volume vendor telemetry → RotateProxy preventivo
- [x] **`EventKind::VendorTelemetryObserved`** variant
- [x] **Testes unit** — classifier coverage por vendor, PX signal catalog completude
- [x] **Gates verdes**: build all + mini + clippy + test + live HN + live SPA + live ScriptSpec + live throughput + live motion
- [!] **Output** `.dispatch/tasks/p0-vendor-signatures/output.md` — output skipped: instructions override forbids proactive doc creation; final report returned via IPC message instead.

## Notes / deviations
- The observer does not `emit()` an `EventEnvelope` because `render::pool` has
  no sink plumbing at `render_core` depth (sinks are passed to
  `render_with_script` only). We persist to SQLite and log via `tracing`
  instead; the `EventKind::VendorTelemetryObserved` variant is added so any
  future wiring through a sink will serialize cleanly.
- `ChallengeVendor` gained a `Hash` derive — required by the tracker's
  `HashMap<(String, ChallengeVendor), _>`. No behavioural change, just a
  new trait on an already-public enum.
- `antibot::system_time_serde` was visibility-raised to `pub(crate)` so
  `telemetry::VendorTelemetry` can reuse the same unix-millis serde
  contract already applied to `ChallengeSignal`.

## Restrições
- Trilho: Antibot/Stealth (classification layer).
- Não tocar TLS/H2 (task C paralela).
- Não tocar motion/keyboard/stealth shim (fechados).
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde (classifier é pure, sem cdp-backend gate — só persist é gated).
- Sem commits.
- Não decodificar payloads ofuscados — só shape/size/keys.
- Sem solver externo de CAPTCHA.
- Live HN/throughput sem regressão.

## Arquivos críticos
- `src/antibot/mod.rs` — expand
- `src/antibot/telemetry.rs` (novo)
- `src/antibot/signatures.rs` (novo — catalog URL patterns + PX signals)
- `src/render/pool.rs` — hook em Network.requestWillBeSent já existente
- `src/storage/sqlite.rs` — vendor_telemetry table
- `src/storage/mod.rs` — `record_telemetry` API
- `src/events/envelope.rs` — VendorTelemetryObserved
- `src/policy/engine.rs` — high-volume → RotateProxy
- `tests/vendor_telemetry.rs` (novo)
- `tests/antibot_fixtures/vendor_payloads/*.txt` (amostras pequenas por vendor)
