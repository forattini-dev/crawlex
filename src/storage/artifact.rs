//! `ArtifactStorage` — blob writes and metadata reads.
//!
//! Carries every method that persists raw upstream bytes, post-JS HTML,
//! screenshots, and ScriptSpec-driven artifacts; plus the read-side
//! [`list_artifacts`] view that powers `crawlex artifacts list`.
//!
//! Default bodies are conservative: `save_raw_response` delegates to
//! [`save_raw`] for backends that don't track final URL / status,
//! `save_screenshot` delegates to [`save_artifact`] using
//! `ScreenshotFullPage`, and the rest no-op for backends that don't
//! persist the kind. Only `save_raw`, `save_rendered`, and `save_edge`
//! are required — they are the minimum a backend must implement to be
//! a useful artifact sink.

use bytes::Bytes;
use http::HeaderMap;
use url::Url;

use crate::storage::{session_id_for_url, ArtifactKind, ArtifactMeta, ArtifactRow, PageMetadata};
use crate::Result;

/// Blob persistence + artifact read API.
///
/// Implementors are typically the on-disk filesystem layout, the SQLite
/// row writer, or an in-memory test double. New backends only need to
/// satisfy this trait when callers take an `Arc<dyn ArtifactStorage>`
/// directly — see `script::runner::ScriptRunner` for that pattern.
#[async_trait::async_trait]
pub trait ArtifactStorage: Send + Sync {
    /// Persist the upstream response body keyed by request URL.
    async fn save_raw(&self, url: &Url, headers: &HeaderMap, body: &Bytes) -> Result<()>;

    /// Richer variant carrying redirect chain + status + truncation flag.
    /// Default delegates to [`save_raw`] so older backends compile
    /// without change; rich backends override.
    async fn save_raw_response(
        &self,
        url: &Url,
        _final_url: &Url,
        _status: u16,
        headers: &HeaderMap,
        body: &Bytes,
        _truncated: bool,
    ) -> Result<()> {
        self.save_raw(url, headers, body).await
    }

    /// Persist the post-JS HTML produced by the renderer.
    async fn save_rendered(&self, url: &Url, html_post_js: &str, meta: &PageMetadata)
        -> Result<()>;

    /// Persist a single edge of the link graph.
    async fn save_edge(&self, from: &Url, to: &Url) -> Result<()>;

    /// Persist a per-URL screenshot. Default delegates to [`save_artifact`]
    /// using a synthetic `ArtifactMeta { kind: ScreenshotFullPage }`.
    /// Backends that maintain a legacy per-URL screenshot table override.
    ///
    /// Returns the storage location (path or URI) when the backend
    /// actually persisted the bytes; `None` for sinks that no-op (e.g.
    /// in-memory tests).
    async fn save_screenshot(&self, url: &Url, png: &[u8]) -> Result<Option<String>> {
        let session_id = session_id_for_url(url);
        let meta = ArtifactMeta {
            url,
            final_url: None,
            session_id: &session_id,
            kind: ArtifactKind::ScreenshotFullPage,
            name: None,
            step_id: None,
            step_kind: None,
            selector: None,
            mime: None,
        };
        self.save_artifact(&meta, png).await
    }

    /// Persist a single artifact (screenshot, snapshot, state dump) with
    /// the unified `ArtifactMeta`. Returns the storage location (path or
    /// URI) when the backend actually persisted the bytes; `Ok(None)`
    /// for sinks that no-op.
    async fn save_artifact(
        &self,
        _meta: &ArtifactMeta<'_>,
        _bytes: &[u8],
    ) -> Result<Option<String>> {
        Ok(None)
    }

    /// Read-side: list persisted artifacts filtered by optional
    /// `session_id` and optional `kind`. Default returns empty.
    async fn list_artifacts(
        &self,
        _session_id: Option<&str>,
        _kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRow>> {
        Ok(Vec::new())
    }
}
