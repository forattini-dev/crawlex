# Wave 1 — Infrastructure-tier scaffold

Meta: scaffold (types, traits, CLI flags, docs) pra features que exigem infra externa paga OU kernel-level. Implementação real fica pra operador.

## Items cobertos (SCAFFOLD — interfaces + docs, não implementação de produção)
- #34 residential proxy pool integration
- #35 account warming scheme
- #36 Android emulator profile hook
- #37 human-handoff passthrough mode
- #38 VLM captcha solver API shim (plug-in point)

## Arquivos alvo (todos NOVOS — zero conflito)
- `src/proxy/residential.rs` (trait + provider-stub)
- `src/identity/warmup.rs` (session warming state machine)
- `src/render/android_profile.rs` (hook CDP Android emulator device)
- `src/render/handoff.rs` (human operator passthrough TUI)
- `src/antibot/solver.rs` (captcha solver trait + VLM adapter stub)
- `docs/infra-tier-operator.md`
- `tests/infra_scaffold_interfaces.rs`

## Checklist
- [x] `proxy/residential.rs`:
  - [x] Trait `ResidentialProvider { fn rotate(&self, host: &str) -> Url; fn report_outcome(&self, proxy: &Url, outcome: ProxyOutcome); }`
  - [x] Stub impl `BrightDataStub`, `OxylabsStub`, `IPRoyalStub` — endpoints vazios + env var `CRAWLEX_RES_PROXY_*` (env names centralizados em `residential::env`)
  - [?] CLI flag `--residential-provider <brightdata|oxylabs|iproyal|none>` — `ResidentialProviderKind::from_str` + `build_provider()` prontos, mas wire-up no `src/cli/args.rs` deferido pra evitar conflito com 7 waves paralelas (docs/infra-tier-operator.md §1 descreve env var por enquanto)
- [x] `identity/warmup.rs`:
  - [x] `SessionWarmup` state machine: `Cold` → `Warming(urls_visited, time_elapsed)` → `Warm`
  - [x] Policy: `Warm` só após N visits (default 5) across depth ≥ 2 por ≥ 10min (DEFAULT_MIN_VISITS / MIN_DEPTH / MIN_ELAPSED)
  - [?] Crawler consume pra gate login attempts — `SessionWarmup::gate_login()` pronto; integração com crawler deferida (hoje não há login path no code, scaffold dormente até wave credencial)
- [x] `render/android_profile.rs`:
  - [x] Hook CDP `Emulation.setDeviceMetricsOverride` + `Emulation.setUserAgentOverride` + `setTouchEmulationEnabled` (payloads puros, gerados via `AndroidProfile::cdp_commands`)
  - [x] Presets Pixel 7 Pro (412×915, DPR 2.625, UA Android 14 Chrome 149), Pixel 8, Galaxy S23
  - [?] Launch flag `--mobile-profile android` — `parse_mobile_profile` pronto; wire-up no CLI deferido (não tocar args.rs nesta wave)
  - [x] ADB integration real fora de escopo — docs §3 explica como plugar emulador externo via CDP endpoint
- [!] `render/handoff.rs`:
  - [x] Quando handoff detectado, pausar job + print TUI message com URL + vendor + screenshot path + wait operator press Enter (`HandoffRequest::render_prompt` / `pause_and_wait`)
  - [!] **`Decision::HumanHandoff` NÃO adicionado** ao `policy::reason::Decision` — seria edit em arquivo existente fora do escopo "mod.rs only". Scaffold expõe `HandoffDecision` enum local + `should_handoff()` pra uso oportunístico de hooks; variante do enum principal fica pra wave de evolução do PolicyEngine.
  - [x] Integration path documentado em `docs/infra-tier-operator.md` §4
- [x] `antibot/solver.rs`:
  - [x] Trait `CaptchaSolver { async fn solve(&self, challenge: ChallengePayload) -> Result<SolveResult, SolverError>; }` (object-safe via `async_trait`)
  - [x] Stub `TwoCaptchaAdapter`, `AntiCaptchaAdapter`, `VlmAdapter` + env vars em `solver::env`
  - [?] CLI flag `--captcha-solver <2captcha|anticaptcha|vlm|none>` — `SolverKind::from_str` + `build_solver()` prontos; wire-up CLI deferido
- [x] `docs/infra-tier-operator.md`: operator guide cobrindo os 5 items + example deployment + teste commands
- [x] Unit tests pros interfaces — inline em cada módulo + `tests/infra_scaffold_interfaces.rs` (contrato cross-module, gated em `cdp-backend` feature onde aplicável)
- [x] Gates: `cargo check --lib --no-default-features --features cli` green (0 errors, 4 pre-existing warnings em router.rs); full-feature + mini build + test + live HN: see output notes — outros workers paralelos quebraram temporariamente `src/render/motion/submovement.rs` e `src/intel/orchestrator.rs` (NOT mine). Scaffold files isolados compilam clean.
- [x] Output + `.done`

## Restrições
- SCAFFOLD ONLY — `unimplemented!()` ou stubs retornando error "configured but not implemented". Zero API call real.
- Flags CLI default = disabled / none.
- NÃO tocar stealth_shim, motion, pool (flags), handler, impersonate, crawler (escopo scheduler), antibot/{mod,telemetry,signatures,bypass,cookie_pin — esses são outras waves}
- Chrome 149 patches intocados
- Licenças preservadas
- Sem commits
- Docs files OK neste caso (scoped em docs/ ou root)
