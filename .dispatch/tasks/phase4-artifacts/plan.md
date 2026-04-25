# Fase 4 — Artifacts/Screenshots como produto

Meta: fechar screenshot/snapshot como feature de produto, não primitive interna. Metadados consistentes, unificação de tipos de artifact, consulta via storage estruturada.

## Estado atual

Já existe:
- `ScreenshotMode::{Viewport, FullPage, Element}` em `src/script/spec.rs`
- `RenderPool::capture_screenshot_mode` implementando os 3 modos
- `SnapshotKind::{ResponseBody, DomSnapshot, PostJsHtml, State, AxTree}` em `src/script/spec.rs`
- `ScriptRunner::exec_screenshot` / `exec_snapshot` emitindo `ArtifactRef` + eventos
- `Storage::save_screenshot` (binário por URL)
- Tabela `screenshots` no SQLite

Falta: **metadados estruturados por artifact**, unificação screenshot+snapshot+state+AX numa tabela só com consulta por session/url/step/kind, Lua também emitindo via mesmo pipeline.

## Entregáveis

### 1. Tabela unificada `artifacts`

Schema novo em `src/storage/sqlite.rs`:

```sql
CREATE TABLE IF NOT EXISTS artifacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL,
    final_url TEXT,
    session_id TEXT NOT NULL,
    kind TEXT NOT NULL,            -- "screenshot.viewport" | "screenshot.fullpage" | "screenshot.element" | "snapshot.html" | "snapshot.state" | "snapshot.ax_tree" | "snapshot.post_js_html"
    name TEXT,                     -- operator-provided label or auto "step_<id>_<kind>"
    step_id TEXT,                  -- populated when emitted from ScriptRunner
    step_kind TEXT,
    selector TEXT,                 -- populated when kind=screenshot.element
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    bytes BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_artifacts_session ON artifacts(session_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_kind ON artifacts(kind);
CREATE INDEX IF NOT EXISTS idx_artifacts_step ON artifacts(step_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_url ON artifacts(url);
```

### 2. `Storage::save_artifact` trait method

```rust
pub struct ArtifactMeta<'a> {
    pub url: &'a Url,
    pub final_url: Option<&'a Url>,
    pub session_id: &'a str,
    pub kind: ArtifactKind,
    pub name: Option<&'a str>,
    pub step_id: Option<&'a str>,
    pub step_kind: Option<&'a str>,
    pub selector: Option<&'a str>,
    pub mime: &'a str,
}

pub enum ArtifactKind {
    ScreenshotViewport,
    ScreenshotFullPage,
    ScreenshotElement,
    SnapshotHtml,        // html_post_js
    SnapshotState,       // cookies + localStorage + sessionStorage
    SnapshotAxTree,      // AX tree rendering
    SnapshotDom,         // raw DOM before JS
}

impl ArtifactKind {
    pub fn wire_str(&self) -> &'static str;  // for SQL/events
    pub fn mime(&self) -> &'static str;      // default MIME
    pub fn extension(&self) -> &'static str;
}

#[async_trait]
pub trait Storage {
    async fn save_artifact(&self, meta: &ArtifactMeta<'_>, bytes: &[u8]) -> Result<()>;
    async fn list_artifacts(&self, session_id: Option<&str>, kind: Option<ArtifactKind>) -> Result<Vec<ArtifactRow>>;
    // ... existing methods
}

pub struct ArtifactRow {
    pub id: i64,
    pub url: Url,
    pub final_url: Option<Url>,
    pub session_id: String,
    pub kind: ArtifactKind,
    pub name: Option<String>,
    pub step_id: Option<String>,
    pub step_kind: Option<String>,
    pub selector: Option<String>,
    pub sha256: String,
    pub size: u64,
    pub created_at: SystemTime,
}
```

### 3. Filesystem backend

`src/storage/filesystem.rs`: artifacts salvos como:
```
<root>/
  artifacts/
    <session_id>/
      <timestamp>_<kind>_<sha8>.<ext>       # bytes
      <timestamp>_<kind>_<sha8>.meta.json   # sidecar metadata
```

### 4. Migração `save_screenshot` → `save_artifact`

Callers atuais de `save_screenshot` migram pra `save_artifact(ArtifactKind::ScreenshotFullPage, ...)`. Manter `save_screenshot` como wrapper deprecated por 1 ciclo se quebrar demais.

### 5. ScriptRunner emite via save_artifact

`src/script/runner.rs::exec_screenshot` / `exec_snapshot`:
- `ArtifactRef` gerado hoje carrega sha256+size — adicionar `kind`, `name`, `step_id`, `selector`.
- `storage.save_artifact(meta, bytes).await`
- Emit `EventKind::ArtifactSaved` com campos enriquecidos (já existe — confirmar metadata fields).

### 6. Lua bridge `page_screenshot_save` / `page_snapshot_save`

`src/hooks/lua.rs`:
- `page_screenshot_save(mode?, name?)` hoje já persiste — atualizar pra usar `save_artifact` com `ArtifactKind` correto
- Adicionar `page_snapshot_save(kind, name?)` — kinds: `"html"`, `"state"`, `"ax_tree"`
- Retornos: caminho/id do artifact (string)

### 7. Render path unifica

`src/render/pool.rs::render_inner` (existe pós-Fase 2): screenshot final continua, mas agora via `save_artifact` com meta completo (session_id, step_id=None legado, kind=ScreenshotFullPage default). `state` snapshot também usa save_artifact (hoje usa `save_state` — migrar).

### 8. Event metadata

`EventKind::ArtifactSaved` precisa ter:
```rust
ArtifactSaved {
    url: Url,
    final_url: Option<Url>,
    session_id: String,
    kind: String,          // wire_str
    name: Option<String>,
    step_id: Option<String>,
    step_kind: Option<String>,
    selector: Option<String>,
    mime: String,
    size: u64,
    sha256: String,
}
```

Confirma estrutura atual em `src/events/envelope.rs` e estende.

## Checklist

- [x] **Schema + Storage trait**:
  - Tabela `artifacts` em SQLite + index
  - `Storage::save_artifact` + `list_artifacts` no trait
  - `ArtifactKind`, `ArtifactMeta`, `ArtifactRow` types
  - Writer-thread Op + handler

- [x] **SQLite impl**: save + list funcionando. Memory backend: `save_artifact` → no-op ou Err(Unsupported).

- [x] **Filesystem impl**: bytes + .meta.json sidecar atomic write.

- [x] **Migrar callers internos**:
  - `save_screenshot` → `save_artifact(ArtifactKind::ScreenshotFullPage, ...)` nos call sites em `src/crawler.rs` + `src/render/pool.rs`
  - `save_state` → `save_artifact(ArtifactKind::SnapshotState, ...)` nos call sites relevantes
  - Deprecar trait methods antigos (mantém impl como wrapper) ou remover se não houver callers externos

- [x] **ScriptRunner**: `exec_screenshot`/`exec_snapshot` chamam `save_artifact` com `step_id` + `step_kind` + `name` populados. Update `ArtifactRef` struct se necessário.

- [x] **Lua bridge**: `page_screenshot_save` usa `save_artifact`; novo `page_snapshot_save(kind, name?)` com 3 kinds (html/state/ax_tree).

- [x] **Render path**: screenshot final + state via `save_artifact`.

- [x] **Event metadata**: `ArtifactSaved` enriquecido. Emit consistente em todos caminhos.

- [x] **Testes unit** `tests/artifact_storage.rs`:
  - Round-trip SQLite (save + list por session, filtro por kind)
  - SHA256 correto
  - Filesystem atomic write (tmp+rename)
  - ArtifactKind serialization wire_str

- [x] **Testes live** — `spa_scriptspec_live` já cobre exec_screenshot/exec_snapshot via ScriptRunner; extender assert: após run, `storage.list_artifacts(session_id)` retorna rows esperados com kinds corretos.

- [x] **Gates verdes**:
  - `cargo build --all-features`
  - `cargo build --no-default-features --features cli,sqlite`
  - `cargo clippy --all-features --all-targets -- -D warnings`
  - `cargo test --all-features` non-ignored
  - `cargo test --all-features --test live_news_navigation -- --ignored` PASS
  - `cargo test --all-features --test spa_scriptspec_live -- --ignored` PASS

- [x] **Output** `.dispatch/tasks/phase4-artifacts/output.md`.

## Restrições
- Trilho: **Artifacts/Screenshots**. Foco exclusivo.
- Não tocar Fase 1 antibot (fechada), Fase 2 runtime (fechada), Fase 3 SPA (próxima), Fase 5 scale.
- Não criar tabela nova além de `artifacts` — reusar `sessions`, `challenge_events`, `proxy_scores`, `host_affinity` existentes.
- Não breaking-change API externa (CLI existente funciona idêntico; só aumenta output).
- Patches Chrome 149 intocados.
- Licenças preservadas em `src/render/LICENSES/`.
- Mini build obrigatório verde.
- Live HN test sem regressão.
- Sem commits.
- Lua continua compatível — script existente do usuário não quebra.
- AX tree rendering usa `render_tree()` existente de `src/render/ax_snapshot.rs`.

## Arquivos críticos
- `src/storage/mod.rs` — trait + types (ArtifactKind, ArtifactMeta, ArtifactRow)
- `src/storage/sqlite.rs` — schema + save/list
- `src/storage/filesystem.rs` — disk layout
- `src/storage/memory.rs` — no-op
- `src/script/runner.rs` — exec_screenshot/exec_snapshot refactor
- `src/render/pool.rs` — callers migrados
- `src/crawler.rs` — callers migrados
- `src/hooks/lua.rs` — bridge updates
- `src/events/envelope.rs` — ArtifactSaved enriched
- `tests/artifact_storage.rs` — novo
- `tests/spa_scriptspec_live.rs` — assert estendido
