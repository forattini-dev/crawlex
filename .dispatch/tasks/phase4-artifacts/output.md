# Fase 4 — Artifacts/Screenshots: output

## Gates
- `cargo build --all-features` — verde
- `cargo build --no-default-features --features cli,sqlite` — verde
- `cargo clippy --all-features --all-targets -- -D warnings` — verde
- `cargo test --all-features` (non-ignored) — verde
- `cargo test --all-features --test spa_scriptspec_live -- --ignored` — PASS
- `cargo test --all-features --test live_news_navigation -- --ignored` — PASS (~33s)

## O que mudou

### `src/storage/mod.rs`
- Novos tipos públicos: `ArtifactKind`, `ArtifactMeta<'a>`, `ArtifactRow`.
- `ArtifactKind` expõe `wire_str()` / `from_wire()` / `mime()` / `extension()` — contrato estável para SQL / eventos / filesystem.
- Trait `Storage` ganhou:
  - `save_artifact(&ArtifactMeta, &[u8]) -> Result<()>` (default `Ok(())`).
  - `list_artifacts(Option<&str>, Option<ArtifactKind>) -> Result<Vec<ArtifactRow>>` (default `[]`).
- `save_screenshot` default virou wrapper que delega pra `save_artifact(ScreenshotFullPage, ...)` — callers antigos continuam funcionando e aparecem automaticamente na tabela unificada.
- Helper `session_id_for_url` gera `legacy:<host>` pra agrupar saves que não carregam session.

### `src/storage/sqlite.rs`
- Nova tabela `artifacts` + 4 índices (`session_id`, `kind`, `step_id`, `url`).
- Novo `Op::SaveArtifact` na writer-thread + handler INSERT-only (append-only).
- `save_artifact` override serializa meta via writer; `save_screenshot` preserva `screenshots` legacy **e** popula `artifacts`.
- `list_artifacts` via connection read-only paralela (não bloqueia writer).

### `src/storage/filesystem.rs`
- Layout `<root>/artifacts/<session_id>/<ts>_<kind>_<sha8>.<ext>` + sidecar `.meta.json`. Atomic write via tmp+rename.
- `list_artifacts` escaneia sidecars; respeita filtros session/kind.

### `src/storage/memory.rs` — sem mudança; usa defaults do trait (no-op).

### `src/script/runner.rs`
- `ArtifactRef` enriquecido: `step_kind`, `mime`, `selector` adicionados aos existentes.
- `ScriptRunner` novos builders: `with_storage(Arc<dyn Storage>)`, `with_url(Url)`.
- `exec_screenshot` / `exec_snapshot` agora:
  - mapeiam pra `ArtifactKind` correto,
  - chamam `save_artifact` com `step_id`, `step_kind`, `selector`, `name`,
  - emitem `EventKind::ArtifactSaved` via `ArtifactSavedData` (schema único).
- `persist_artifact` helper privado é no-op se storage/url não foram wired (safe default).

### `src/events/envelope.rs` + `events/mod.rs`
- Novo `ArtifactSavedData` public — struct serializável que todos emissores usam.
- Re-export em `events::ArtifactSavedData`.

### `src/render/pool.rs`
- `render_with_script` passa `self.storage.clone()` + `url.clone()` ao `ScriptRunner` pra que artifacts sejam persistidos através do trait.

### `src/hooks/lua.rs`
- `page_screenshot_save(mode?, name?)`: agora escreve tanto no wrapper legacy (`save_screenshot`) quanto no `save_artifact` com `ArtifactKind` correto (Viewport/FullPage/Element), `step_kind="lua"`, selector populado quando element.
- Novo `page_snapshot_save(kind, name?)` — kinds aceitos: `"html"` / `"post_js_html"`, `"state"`, `"ax_tree"`. Captura + persiste via `save_artifact`. `ax_tree` também atualiza o `ref_map` do host (paridade com `page_ax_snapshot`).

## Testes

### Unit novos — `tests/artifact_storage.rs` (4 tests, non-ignored)
1. `artifact_kind_wire_str_round_trip` — cobre todos variants + MIME/ext.
2. `sqlite_save_artifact_round_trips_and_filters` — 4 rows, filtro por session + kind.
3. `filesystem_save_artifact_writes_bytes_and_sidecar` — verifica 2 arquivos no disco + schema JSON do sidecar + list_artifacts.
4. `default_save_screenshot_lands_in_artifacts_table` — wrapper legacy atinge a nova tabela.

### Live estendido — `tests/spa_scriptspec_live.rs`
Pós-run do `RenderPool::render_with_script`, agora assert:
- `storage.list_artifacts(None, None)` contém `ScreenshotElement` + `SnapshotAxTree`.
- Row de element tem `step_kind="screenshot"`, `step_id` populado, `name="dashboard_box"`, `selector` populado.

## Backward compat
- `save_screenshot` / `save_state` preservados — continuam com a mesma assinatura.
- Lua `page_screenshot_save(mode?, name?)` idêntico em assinatura — só passou a popular o meta corretamente em vez de ignorar `name`.
- Nenhum caller interno foi renomeado; `crawler.rs::save_screenshot(&job.url, png)` e `pool.rs::save_state` mantidos.
- Patches Chrome 149 intocados. Licenças preservadas.

## Nota
- Backend `memory` não persiste artifacts (default Ok). É só in-memory raw/rendered mesmo.
- `ArtifactKind::SnapshotHtml` foi adicionado com wire `"snapshot.html"` — reservado pra saves Lua via `page_snapshot_save("html")` quando o usuário prefere tag "html" genérica; mapeamento atual usa `SnapshotPostJsHtml` pra refletir a realidade (post-JS). Distintos no wire pra filtragem futura.
