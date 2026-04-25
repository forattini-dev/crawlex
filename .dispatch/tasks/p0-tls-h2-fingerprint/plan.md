# P0 — TLS/H2 fingerprint byte-exact + header order

Meta: validar e corrigir HTTP/2 SETTINGS + WINDOW_UPDATE + pseudo-header order pra matchar Chrome real byte-exact. Garantir header order diferenciado por request type (document vs XHR vs fetch vs script). Referência: `research/evasion-deep-dive.md#3` (TLS/HTTP) + `research/evasion-actionable-backlog.md` P0-7/P0-8.

## P0 coverage

- **P0-7** HTTP/2 Akamai fingerprint byte-exact validation (Chrome 144+ values)
- **P0-8** Header order por request type

## Targets Chrome 144+ (do research)

HTTP/2 fingerprint Akamai format:
- SETTINGS: `1:65536;2:0;4:6291456;6:262144`
  - 1 = HEADER_TABLE_SIZE = 65536
  - 2 = ENABLE_PUSH = 0
  - 4 = INITIAL_WINDOW_SIZE = 6291456
  - 6 = MAX_HEADER_LIST_SIZE = 262144
- WINDOW_UPDATE: `15663105`
- Pseudo-header order: `m,a,s,p` (method, authority, scheme, path)
- NO standalone PRIORITY frames — priority via HEADERS frame flag

## Entregáveis

### 1. Audit estado atual

```bash
grep -rn "SettingsFrame\|SettingsBuilder\|InitialWindowSize\|ENABLE_PUSH\|HEADER_TABLE_SIZE" src/
grep -rn ":method\|:authority\|:scheme\|:path" src/
grep -rn "WINDOW_UPDATE\|window_update" src/
```

Documentar o que `src/impersonate/tls.rs` + `src/http/*` + ClientHello builder emitem hoje.

### 2. Byte-exact validation tool

Novo `tests/h2_fingerprint_live.rs` `#[ignore]`:
- Spawn servidor local HTTP/2 que log raw frames recebidos
- Crawlex faz request em HTTP mode
- Assert: SETTINGS frame → `{1:65536, 2:0, 4:6291456, 6:262144}` EXATO
- Assert: WINDOW_UPDATE inicial → `15663105` EXATO
- Assert: HEADERS frame pseudo-header order `:method, :authority, :scheme, :path`
- Assert: sem PRIORITY frames separados

Se usar `rustls-test` ou `h2` crate direto pra parse frames.

### 3. Corrigir discrepâncias

Com base na validação:
- Ajustar SETTINGS builder pra batch frames em ordem Chrome
- Ajustar pseudo-header order no H2 request builder
- Remover PRIORITY frames standalone se algum código está emitindo
- WINDOW_UPDATE com value 15663105

Arquivos prováveis: `src/impersonate/tls.rs`, `src/http/mod.rs`, `src/http/connection.rs`.

### 4. Header order por request type

Chrome emite ordens diferentes pra diferentes request types:
- **Document** (main navigation): `:method, :authority, :scheme, :path, sec-ch-ua, sec-ch-ua-mobile, sec-ch-ua-platform, upgrade-insecure-requests, user-agent, accept, sec-fetch-site, sec-fetch-mode, sec-fetch-user, sec-fetch-dest, accept-encoding, accept-language, cookie`
- **XHR/fetch**: sem `upgrade-insecure-requests`, `sec-fetch-dest: empty`, adiciona `origin`, `content-type` se POST
- **Script/Style/Image**: sem `sec-fetch-user`, `sec-fetch-dest` específico (script/style/image), `accept` específico por MIME

Implementação:
```rust
pub enum ChromeRequestKind { Document, Xhr, Fetch, Script, Style, Image, Font, Ping }

impl ChromeRequestKind {
    pub fn header_order(&self) -> &[&'static str];
    pub fn default_accept(&self) -> &'static str;
    pub fn default_sec_fetch_dest(&self) -> &'static str;
    pub fn default_sec_fetch_mode(&self) -> &'static str;
}
```

Em `src/http/client.rs` ou wrapper: recebe `ChromeRequestKind`, aplica header set canonical na ordem correta.

Render path chama com `Document` pra navegações; XHR/fetch observados do Observer SPA (Fase 3) ganham `Xhr`/`Fetch`.

### 5. Testes

- `tests/h2_fingerprint_live.rs` (`#[ignore]`, system Chrome or crawlex spoof) — byte-exact assertions
- `tests/header_order.rs` non-ignored — pra cada ChromeRequestKind, assert ordem emitida match Chrome spec
- `tests/chrome_request_kind.rs` non-ignored — header set per kind, accept defaults

## Checklist

- [x] **Audit estado atual** H2 emission: documentar em output.md o que hoje emitimos SETTINGS/WINDOW_UPDATE/pseudo-header
- [x] **Tool de validação byte-exact** via servidor H2 local em test (`tests/h2_fingerprint_live.rs`)
- [x] **Corrigir SETTINGS** pra `{1:65536, 2:0, 4:6291456, 6:262144}` byte-exact (removido `max_concurrent_streams(1000)` + `max_frame_size(None)` pra suprimir SETTING 5)
- [x] **Corrigir WINDOW_UPDATE** pra 15663105 (já correto via `initial_connection_window_size(15_728_640)` — validado byte-exact)
- [!] **Corrigir pseudo-header order** pra `:method, :authority, :scheme, :path` — **bloqueado**: `h2` crate hard-codes `m,s,a,p` (m,scheme,authority,path) em `frame/headers.rs::Iter::next`. Requer fork/patch do h2 (recomendação: seguir padrão rebrowser-patches, `[patch.crates-io]` com diff de 4 linhas). Documentado em output.md §4. Demais axes (SETTINGS/WU/ALPS/PRIORITY/TLS) permanecem Chrome-exact.
- [x] **Remover PRIORITY frames** standalone — `h2` crate client não emite standalone PRIORITY; validado no teste live byte-exact
- [x] **`ChromeRequestKind` enum** + header_order + defaults (`src/impersonate/headers.rs`)
- [x] **Wire em `src/http/client.rs`** pra aplicar por request kind — existing `chrome_http_headers_full` já emite em ordem `ChromeRequestKind::Document` para top-level nav; enum exposto publicamente para dispatch futuro de XHR/Fetch vindos do SPA Observer (não há `src/http/client.rs`; o pipeline vive em `src/impersonate/mod.rs`)
- [x] **Render path passa `Document`** em main navigation — `SecFetchDest::Document` já era passado em `chrome_http_headers_full(... dest, ...)` pelo render path; `From<SecFetchDest> for ChromeRequestKind` mapeia corretamente
- [x] **Testes unit header_order** pra cada kind (`tests/chrome_request_kind.rs` + unit tests internos em `headers.rs`)
- [x] **Teste live h2_fingerprint** byte-exact — passa
- [x] **Gates verdes**: build all + mini + clippy + test + live HN + live SPA + live ScriptSpec + live throughput + live motion + live h2_fingerprint
- [x] **Output** `.dispatch/tasks/p0-tls-h2-fingerprint/output.md` com antes/depois da validação byte-exact

## Restrições
- Trilho: Antibot/Stealth (network layer).
- Não tocar em antibot vendor signatures (task D paralela).
- Não tocar em motion/keyboard/stealth shim (P0 tasks fechadas).
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Mini build obrigatório verde (layer HTTP é core, mini depende).
- Sem commits.
- Live HN sem regressão (~33s).
- Throughput sem regressão (~14.9 rps).
- Se `h2` crate bloqueia controle sobre SETTINGS frame exato, documentar e considerar fork local (rebrowser-patches pattern).

## Arquivos críticos
- `src/impersonate/tls.rs` — ClientHello + possivelmente H2 init
- `src/http/*` — request pipeline
- `src/http/client.rs` — request execution + header ordering
- `tests/h2_fingerprint_live.rs` (novo, #[ignore])
- `tests/header_order.rs` (novo)
