# Wave 1 — Stealth shim coherence (shim JS + identity bundle)

Meta: fechar 12 leaks shim + coherence matrix completa. Owner ÚNICO: `src/render/stealth_shim.js` + `src/identity/bundle.rs` + `src/identity/validator.rs` + novos fixtures.

## Items cobertos

**Shim-local leaks:**
- #1 `window.outerWidth - innerWidth` scrollbar shape (15-17px desktop)
- #2 `performance.memory.jsHeapSizeLimit` variance por build
- #3 Error.stack DevTools timing detection
- #4 `chrome.runtime.id` + extensions object shape completo
- #6 `Intl.Segmenter`/`Intl.DisplayNames` consistency
- #8 `speechSynthesis.getVoices()` array shape per-OS
- #9 `Number.toLocaleString()` JIT warmup cadence
- #11 `requestAnimationFrame` throttling em hidden (1Hz quando visibility=hidden)
- #12 `performance.now()` precision (5μs main, 1ms isolated)
- #44 `AudioContext.sampleRate` variation (48000/44100/22050 por GPU+audio device)
- #45 `navigator.mediaDevices.enumerateDevices()` fake list (mic+speaker)

**Identity coherence matrix (A.1 do plan anterior):**
- Tabela OS × locale × timezone × language × screen × hardware_concurrency × deviceMemory × fonts
- Validator audita coerência na construção
- Pool: Win10 en-US / Win10 pt-BR / macOS en-US / Linux en-US / Mobile Android pt-BR
- Fonts list coerente com OS (Linux fonts em Linux bundle, Windows fonts em Windows)

**UA-CH dynamic coherence (A.2):**
- `navigator.userAgentData.getHighEntropyValues()` retorna valores que matcham headers `Sec-CH-UA-*`

**WebGL coherence (A.3):**
- Tabela GPU-profile: Intel HD 620, NVIDIA GTX 1060, AMD RX 580, Apple M1, Mobile Adreno
- `UNMASKED_VENDOR_WEBGL` + `UNMASKED_RENDERER_WEBGL` + `MAX_TEXTURE_SIZE` + `MAX_VIEWPORT_DIMS` consistentes
- Extensions ordering per GPU

**Audio/Canvas determinism (A.4, A.5):**
- Canvas jitter deterministic seed (bundle+session), não random
- Audio jitter gaussiano com σ=seed-derived (não uniform random)

## Arquivos alvo

- `src/render/stealth_shim.js` (edits pesados)
- `src/identity/bundle.rs` (nova matriz + WebGL profile + UA-CH shape)
- `src/identity/validator.rs` (coherence checks)
- `src/identity/profiles.rs` (novo — catalog pools OS/locale/GPU)
- `tests/identity_coherence.rs`
- `tests/stealth_shim_leaks.rs`

## Checklist
- [ ] `src/identity/profiles.rs` catalog: OS × locale × tz × screen × cpu × ram × gpu × fonts × audio sample rate
- [ ] `IdentityBundle` expandir com novos fields + sort coerente
- [ ] Validator checks: OS×locale plausible, tz em pool, screen em pool, fonts bate OS, GPU bate OS
- [ ] Shim placeholders pros novos fields: `{{SCROLLBAR_WIDTH}}`, `{{HEAP_SIZE_LIMIT}}`, `{{DEVICE_MEMORY}}`, `{{HW_CONCURRENCY}}`, `{{GPU_VENDOR}}`, `{{GPU_RENDERER}}`, `{{MAX_TEXTURE_SIZE}}`, `{{AUDIO_SAMPLE_RATE}}`, etc
- [ ] Implement shim overrides #1 outerWidth, #2 jsHeapSizeLimit, #4 chrome.runtime.id, #8 getVoices, #11 rAF throttle, #12 performance.now precision, #44 AudioContext sampleRate, #45 mediaDevices fake
- [ ] Canvas jitter: trocar Math.random por seeded PRNG determinístico por (bundle, session, op_index, pixel)
- [ ] Audio jitter: Box-Muller gaussiano com σ derivada
- [ ] UA-CH dynamic getHighEntropyValues match headers
- [ ] WebGL per-GPU table + consistent return
- [ ] `tests/identity_coherence.rs` — 100 bundles → 100% passam validator
- [ ] `tests/stealth_shim_leaks.rs` — parse shim JS + assert cada placeholder presente + override shape
- [ ] Gates: build all + mini + clippy + test + live HN sem regressão
- [ ] Output + `.done`

## Restrições
- Chrome 149 handler patches intocados
- Licenças preservadas
- Sem commits
- Single ownership: NÃO tocar em `src/render/motion/`, `src/render/pool.rs` (flags), `src/render/chrome/handler/`, `src/impersonate/`, `src/crawler.rs`, `src/antibot/`
- Live HN baseline ~33s sem regressão
- Throughput baseline 14.9 rps sem regressão
