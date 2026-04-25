# Fase 4.4 — Download / MIME handling

HTTP path hoje salva só HTML. Adicionar classificador + pipeline pra baixar binários (PDF, imagem, ZIP, mídia, etc) com SHA256 e limite de tamanho.

## Checklist

- [ ] **Investigar storage atual**: ler `src/storage/mod.rs` (trait) e `src/storage/{sqlite,filesystem,memory}.rs`. Entender pattern `save_raw`, `save_screenshot`, `save_state`.

- [ ] **Criar `src/download/mod.rs`**:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  pub enum AssetKind {
      Html,
      Pdf,
      Image(ImageFmt),
      Media(MediaFmt),
      Archive(ArchiveFmt),
      Document(DocFmt),
      Font,
      JavaScript,
      Css,
      Json,
      Xml,
      Binary,
  }
  pub enum ImageFmt { Jpeg, Png, Webp, Gif, Svg, Avif }
  pub enum MediaFmt { Mp4, Webm, Mp3, Wav, Ogg }
  pub enum ArchiveFmt { Zip, Tar, Gz, SevenZ, Rar }
  pub enum DocFmt { Docx, Xlsx, Pptx, Odt }

  impl AssetKind {
      pub fn classify(mime: Option<&str>, url: &url::Url) -> Self;
      pub fn as_str(&self) -> &'static str;  // for logging/serialization
      pub fn extension(&self) -> Option<&'static str>;
  }

  pub struct DownloadPolicy {
      pub allowed_kinds: HashSet<String>,  // default: {"html"}
      pub max_size_bytes: u64,             // default: 50 MB
  }
  impl DownloadPolicy {
      pub fn should_store(&self, kind: AssetKind, size_hint: Option<u64>) -> bool;
  }
  ```
  Classify heurística: MIME primeiro → fallback extension da URL. Cobrir MIME types comuns (`application/pdf`, `image/*`, `video/*`, `audio/*`, `application/zip|x-tar|gzip|x-7z-compressed`, `application/vnd.openxmlformats-officedocument.*`).

- [ ] **Storage trait extend**: em `src/storage/mod.rs`:
  ```rust
  async fn save_asset(&self, url: &Url, kind: AssetKind, mime: &str, bytes: &[u8]) -> Result<()>;
  ```
  Default impl retorna `Err(Error::Unsupported("save_asset"))` pra Memory backend.

- [ ] **SQLite schema**: nova tabela:
  ```sql
  CREATE TABLE IF NOT EXISTS assets (
    url_hash TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    kind TEXT NOT NULL,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    bytes BLOB NOT NULL,
    fetched_at INTEGER NOT NULL
  );
  CREATE INDEX IF NOT EXISTS idx_assets_kind ON assets(kind);
  ```
  `url_hash` = sha256(url) pra dedupe mantendo tamanho fixo. Op::SaveAsset enqueued no writer thread pattern existente.

- [ ] **Filesystem impl**: arquivos vão pra `<root>/assets/<kind>/<sha256>.<ext>` + sidecar `.json` com `{url, mime, size, sha256, fetched_at}`.

- [ ] **CLI flags**: `src/cli/args.rs`:
  - `--download-kinds <csv>` default `"html"` — lista permitida: `html,pdf,image,media,archive,document,font,script,css,json,xml,binary,all`
  - `--max-asset-size-mb <u64>` default 50
  Parse em `src/cli/mod.rs`, popular `Config::download_policy: DownloadPolicy`.

- [ ] **Wire no HTTP path**: em `src/crawler.rs` (ou onde `process_job` faz fetch HTTP):
  - Após fetch, se `content-type` não é HTML E `DownloadPolicy::should_store(kind, size)`:
    - Computar sha256
    - `storage.save_asset(url, kind, mime, &bytes)` → emit `artifact.saved { kind: "pdf", sha256: ..., size: ... }` no NDJSON bus
  - Size check early (antes de baixar tudo): usar `Content-Length` header se presente + stream cut-off se passar do max.

- [ ] **Unit tests**: `tests/asset_classify.rs`:
  - MIME matrix: 20+ MIMEs → kind esperado
  - URL extension fallback (sem MIME)
  - Ambigous MIME + URL → MIME vence
  - Policy allow/deny
  - `"all"` wildcard funciona

- [ ] **Integration test**: `tests/http_binary_download.rs` com wiremock:
  - Servir `/doc.pdf` com `content-type: application/pdf` + 1KB de bytes PDF fake (magic `%PDF-`)
  - Crawler fetch → storage
  - Query SQLite: `SELECT kind, mime, size, sha256 FROM assets` → assert `kind="pdf", mime="application/pdf", size=N, sha256=valid_hex`

- [ ] **Emit `artifact.saved`**: conferir em `src/events/kinds.rs` se já existe variant `ArtifactSaved { kind, url, size, sha256 }` — se não, adicionar. NDJSON output deve incluir `kind`.

- [ ] **Feature gate**: download compila sem cdp-backend. Garante `cargo build --no-default-features --features cli,sqlite`.

- [ ] **Verify**: build all + mini + clippy + test + live HN (sem regressão).

- [ ] **Output**: `.dispatch/tasks/phase4-4-download/output.md`.

## Restrições

- Não mexer em proxy (4.3 está em paralelo).
- Não mexer em render path (follow-up). Basta HTTP por enquanto.
- Sem Lua binding novo (follow-up).
- Sem commits.
- Mini build obrigatório verde.
- URL normalizada antes de hash (usar `Dedupe::canonicalize` se existir, senão URL parsing padrão).
- Sha256 via `sha2` crate já presente no workspace.
- Sem panic em size overflow — erro controlado + skip.
