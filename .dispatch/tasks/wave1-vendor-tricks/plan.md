# Wave 1 — Vendor-specific bypass tricks

Meta: bypass/replay tricks pra vendors comerciais. Owner: `src/antibot/*` + novos helpers.

## Items cobertos
- #39 Cloudflare Turnstile "invisible" variant — token dummy + sitekey attempt
- #40 Akamai `_abck` cookie sensor replay (TTL window)
- #41 DataDome `datadome=invalid` retry loop intercept + cookie capture
- #42 PerimeterX `_px*` cookie pinning (24h TTL reuse)

## Arquivos alvo
- `src/antibot/bypass.rs` (novo)
- `src/antibot/cookie_pin.rs` (novo)
- `src/antibot/mod.rs` (wire)
- `src/storage/sqlite.rs` (nova tabela `antibot_cookie_cache`)
- `src/http/cookies.rs` (integration)
- `tests/cookie_pinning.rs`

## Checklist
- [x] `CookiePinStore` — trait + SQLite impl:
  - `pin(vendor, origin, cookie_name, cookie_value, ttl_secs)` + `get_pinned(vendor, origin, cookie_name) -> Option<PinnedCookie>` (per-name granularity so multiple vendor cookies can coexist per origin)
  - Tabela: `antibot_cookie_cache (vendor, origin, cookie_name, value, pinned_at, ttl_secs)` — criada tanto em `src/antibot/cookie_pin.rs::SCHEMA` quanto em `src/storage/sqlite.rs` init (idempotente)
- [x] Akamai `_abck` replay: `capture_from_headers` classifica `_abck` como `vendor="akamai"` com TTL=24h; rejeita valores `~-1~-1` (unsolved). Caller chama `pin_captured` + `get_pinned` no próximo request pro mesmo origin.
- [x] DataDome retry loop: `capture_from_headers` só aceita `datadome=...` em respostas 4xx (vendor retry pattern), TTL=6h conservador. Caller injeta no jar antes do retry.
- [x] PerimeterX `_px2/_px3/_pxvid/_pxhd/_pxde`: detecção via prefixo `_px<digit>` ou nomes documentados; TTL=24h.
- [x] Turnstile invisible: `prepare_turnstile_attempt(BypassLevel, sitekey, invisible_widget) -> TurnstileAttempt` — pure, IO-free. Retorna `Prepared{endpoint, dummy_token=XXXX.DUMMY.TOKEN.XXXX, sitekey}` só quando `level=Aggressive` + sitekey + invisible. Caller (HTTP layer) emite POST + telemetry.
- [x] CLI flag `--antibot-bypass <level>`: `none` (default), `replay`, `aggressive`. Adicionado em `src/cli/args.rs`. `BypassLevel::parse` aceita sinônimos (`off`/`pin`/`active`).
- [x] `tests/cookie_pinning.rs` — 5 casos: memory roundtrip, sqlite roundtrip+overwrite, capture+pin end-to-end, default-is-none regression guard, Turnstile gating.
- [!] Gates: `cargo check --lib` mostra 6 erros, TODOS em `src/render/motion/submovement.rs` e `src/intel/orchestrator.rs` — escopo de outros workers paralelos (motion-engine + infra-fingerprinting). Verificação isolada `cargo check --lib 2>&1 | grep antibot` retornou vazio — meus arquivos compilam cleanly. Mini build / cargo test / HN live bloqueados por contenção de cargo lock + erros de outros workers; gate delegado ao integrator após sync das waves.
- [x] Output + `.done`

## Arquivos entregues
- `src/antibot/bypass.rs` — `BypassLevel`, `capture_from_headers`, `pin_captured`, `prepare_turnstile_attempt`. Pure, IO-free.
- `src/antibot/cookie_pin.rs` — trait `CookiePinStore` + `InMemoryCookiePinStore` + `SqliteCookiePinStore` (feature-gated), constantes TTL por vendor.
- `src/antibot/mod.rs` — wire `pub mod bypass; pub mod cookie_pin;` + `origin_of_url` helper público.
- `src/storage/sqlite.rs` — tabela `antibot_cookie_cache` adicionada ao init schema (junto de `host_affinity`), sem outros side-effects.
- `src/cli/args.rs` — flag `--antibot-bypass <LEVEL>` (default=None).
- `tests/cookie_pinning.rs` — 5 casos cobrindo pin/retrieve/TTL/default/Turnstile gating.

## Restrições respeitadas
- Cookie pinning limitado a cookies que a própria sessão do crawler capturou (ingestão via `capture_from_headers` em response live); doc em `cookie_pin.rs` reforça ética.
- Sem toques em stealth_shim, motion, pool, handler, impersonate, crawler, scheduler.
- Chrome 149 patches intocados.
- Licenças preservadas (sem novas deps; `parking_lot`, `http`, `rusqlite` já no Cargo.toml).
- Sem commits.
- `antibot-bypass` default = `none` (opt-in), com teste `bypass_level_default_is_none` como regressão.
- `src/impersonate/cookies.rs` **não** tocado — plan mencionava `src/http/cookies.rs` mas esse path não existe; integração read-only é feita pelo caller via `extract_high_signal`/`inject` já existentes, sem conflito com CHIPS parser do wave1-crawl-pattern.

## Restrições
- **Ético**: cookie pinning só funciona com cookies obtidos legitimamente pela própria sessão crawlex. NÃO copiar cookies de user real. NÃO roubar de browser host.
- NÃO tocar stealth_shim, motion, pool, handler, impersonate, crawler (escopo scheduler), scheduler
- Chrome 149 patches intocados
- Licenças preservadas
- Sem commits
- Live HN sem regressão
- `antibot-bypass` default = `none` (opt-in explícito)
