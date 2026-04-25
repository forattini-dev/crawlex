# Validation A.1 — Real-world antibot suite

Meta: testar crawlex contra **sites reais protegidos** pra saber o que funciona vs o que quebra. Não é feature nova — é **prova**.

## Alvos

Rodar render contra cada URL, capturar HTML final + screenshot + `page.challenge`, classificar resultado.

| # | URL | Alvo detectado |
|---|-----|----------------|
| 1 | `https://nowsecure.nl` | Cloudflare JS challenge |
| 2 | `https://antoinevastel.com/bots/` | FP tester + bot lists |
| 3 | `https://arh.antoinevastel.com/bots/areyouheadless` | Headless detector |
| 4 | `https://bot.sannysoft.com` | FPJS-style multi-check |
| 5 | `https://abrahamjuliot.github.io/creepjs/` | CreepJS score |
| 6 | `https://browserleaks.com/canvas` | Canvas FP |
| 7 | `https://browserleaks.com/webrtc` | WebRTC IP leak |
| 8 | `https://pixelscan.net/` | Coherence check |

## Entregáveis

### 1. Teste `tests/real_world_antibot_live.rs` `#[ignore]`

Usa system Chrome pra evitar bundled Chromium drift. Rate-limit: 1 request/site/run. `--motion-profile balanced`.

Para cada site:
```rust
async fn probe_site(url: &str) -> SiteVerdict {
    let rendered = render(url, balanced_motion, ...).await?;
    SiteVerdict {
        url,
        http_status: rendered.status,
        challenge: rendered.challenge.clone(),
        html_contains_target: check_per_site_content(url, &rendered.html_post_js),
        screenshot_valid: rendered.screenshot_png.is_some(),
        final_url: rendered.final_url,
        timing_ms: elapsed,
    }
}
```

Per-site criteria (evita asserts frágeis):
- nowsecure.nl: `challenge.is_none()` OU final_url não contem `/cdn-cgi/challenge`
- antoinevastel: html contém info de FP (prova que carregou)
- arh: html contém "you are" — texto do detector
- bot.sannysoft: sem `failed` na tabela de checks
- CreepJS: extract `bot_score` via JS eval; pass se < 0.5
- browserleaks/canvas: hash canvas extraído != known-Chromium-headless hash
- browserleaks/webrtc: local IP != null (proxy leak check diferente — aqui só validamos render)
- pixelscan.net: coherence score visível no DOM

### 2. Report `production-validation/real_world_report.md`

Tabela markdown:
```
| Site | Status | Challenge | Content OK | Screenshot | Notes |
|------|--------|-----------|------------|------------|-------|
| nowsecure.nl | ✅ | none | target found | 245KB | CF JS bypassed |
| bot.sannysoft | 🟡 | none | 3/11 checks fail | 189KB | webdriver ok, plugins weak |
| creepjs | ❌ | none | score=0.78 | 320KB | AudioContext leak detected |
```

Linha por site com verdict + notes breves. Screenshots salvas em `production-validation/screenshots/<host>.png`.

### 3. Summary row em `production-validation/summary.md` (criar)

Linha A.1 do tabelão claim→evidence→verdict.

## Checklist

- [ ] **Setup test file** `tests/real_world_antibot_live.rs` com array dos 8 sites + runner por site
- [ ] **System Chrome path selection** (reusa padrão de `live_news_navigation.rs`)
- [ ] **Rate-limit** — 1 request/site/run, sleep 3s entre sites pra não parecer scanner
- [ ] **Per-site verdict logic** com criteria acima (não genérico)
- [ ] **Capturar screenshots** em `production-validation/screenshots/<host>.png`
- [ ] **Gerar `production-validation/real_world_report.md`** com tabela + notes por site
- [ ] **Criar `production-validation/summary.md`** com row A.1 (será estendido por tasks futuras)
- [ ] **Gates verdes** non-negotiable:
  - `cargo build --all-features`
  - `cargo build --no-default-features --features cli,sqlite`
  - `cargo clippy --all-features --all-targets -- -D warnings`
  - `cargo test --all-features` non-ignored
  - Live HN ~33s sem regressão
- [ ] **Rodar de verdade** `cargo test --all-features --test real_world_antibot_live -- --ignored --nocapture` — não só compilar; precisa ter output real + report populado
- [ ] **Output** `.dispatch/tasks/validation-a1-realworld-antibot/output.md` com summary: pass count, fail count, surprises

## Restrições

- **Ético:** 1 request/site/run MAX. Sem flood.
- System Chrome preferido (sidestep bundled drift).
- Network pode falhar — site down ≠ bug nosso. Classificar site como "unreachable" (não "fail").
- **Sem commits.**
- Patches Chrome 149 intocados.
- Licenças preservadas.
- Se CreepJS ou FPJS bundles JS precisarem de eval extra pra extrair score, usar `page.evaluate_expression` padrão.
- **Honesto:** se 3 de 8 falharem, reportar 3/8 fail. NÃO marcar como `#[ignore="flaky"]`.
- Test timeout total: 10 min (8 sites × ~60s cada).

## Critério de sucesso

Teste roda de verdade, report é gerado, summary.md tem row. Números (pass/fail) podem ser qualquer coisa — o importante é **saber**. Se 6/8 passa é ótimo. Se 3/8 passa é info crítica pra próximos ciclos.
