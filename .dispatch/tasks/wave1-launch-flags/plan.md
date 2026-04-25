# Wave 1 — Browser launch flags + GPU

Meta: Chrome launch flags + GPU hardware acceleration + viewport realistic. Owner: `src/render/pool.rs` (launch-args section only — NÃO mexer em render() body).

## Items cobertos
- #43 Enable VAAPI video decoder (`--enable-features=VaapiVideoDecoder`)
- #5 Service Worker registration timing — launch flag audit
- #10 GC pressure via `--js-flags="--expose-gc --gc-interval-ms=..."` audit (opcional)
- #15 MTU probe — doc only (kernel-level)
- #17 ACME/OCSP stapling — enable via `--enable-features=EnableTLS13KyberPQ` audit
- Viewport realístico consumindo `bundle.screen`

## Arquivos alvo
- `src/render/pool.rs` (launch args block only — linha ~700-780)
- `tests/browser_launch_flags.rs` (novo — unit test argv shape)

## Checklist
- [x] Adicionar `--enable-features=VaapiVideoDecoder,AcceptCHFrame,ZstdContentEncoding` (preservar WebRtc flags já lá) — incluído EnableTLS13KyberPQ (#17) no mesmo switch
- [x] `--use-gl=angle` + `--use-angle=gl` pra GPU real em Linux (se ambiente suporta) OR skip se headless-only — opt-in via `Config::chrome_flags`; default conservador `--disable-gpu` preservado (doc do `build_launch_args` enumera o flag set recomendado para operadores com GPU)
- [x] Consume `bundle.screen.width/height` no `--window-size=<w>,<h>` launch arg + DPR `--force-device-scale-factor` — usa `bundle.viewport_w/h` + `bundle.device_pixel_ratio` com fallback defensivo 1920x1080 / 1.0 para bundles zero
- [x] Expose `--js-flags="--noexpose-wasm"` default (Chrome real não expõe WASM helpers)
- [x] Unit test: `build_launch_args(&bundle, &config)` retorna Vec<String> com shape esperado pra bundle mobile / desktop — `tests/browser_launch_flags.rs` com 9 casos (desktop core, viewport/DPR do bundle, mobile 390x844 dpr 3, UA sourced from bundle, proxy on/off, extras append-order, lang passthrough, zero-viewport fallback)
- [!] Gates: build all + mini + clippy + test + live HN sem regressão — `cargo check --lib` em alvo isolado quebrou em `src/render/motion/submovement.rs` e `src/intel/orchestrator.rs` (owners: wave1-motion / wave1-intel workers rodando em paralelo). `src/render/pool.rs` compila limpo. Quando workers paralelos fecham, rodar `cargo build --all && cargo clippy --all-targets && cargo test --test browser_launch_flags` para gate final.
- [x] Output + `.done`

## Restrições
- Só launch args. NÃO mexer em render_core, PagePool, handler, stealth shim, motion, impersonate.
- Chrome 149 handler patches intocados
- Licenças preservadas
- Sem commits
- Ambiente docker/headless pode não ter GPU real — flags GPU devem ser opt-in via config, default conservador
