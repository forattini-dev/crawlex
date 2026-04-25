pub mod filesystem;
pub mod memory;
#[cfg(feature = "sqlite")]
pub mod sqlite;

use bytes::Bytes;
use http::HeaderMap;
use std::time::SystemTime;
use url::Url;

use crate::Result;

/// Structured label for every artifact kind the pipeline can persist.
///
/// `wire_str()` is the stable string consumers (SQL column values,
/// `ArtifactSaved` event payload, filesystem directory names) rely on.
/// Add a new variant only after confirming no existing consumer filters
/// on the current set — it is part of the public contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// Viewport-sized PNG screenshot.
    ScreenshotViewport,
    /// Full-page PNG screenshot (captures beyond the viewport).
    ScreenshotFullPage,
    /// PNG of a single element bounded rect.
    ScreenshotElement,
    /// HTML post-JS (serialised `document.documentElement.outerHTML`).
    SnapshotHtml,
    /// Raw DOM snapshot taken before any user JS runs — currently a
    /// fallback to post-JS HTML when we cannot rewind.
    SnapshotDom,
    /// Same as `SnapshotHtml` but from the `post_js_html` pipeline —
    /// kept distinct so consumers can filter between the runner's
    /// snapshot and the renderer's final output.
    SnapshotPostJsHtml,
    /// Original upstream response body bytes.
    SnapshotResponseBody,
    /// Session state dump: cookies + localStorage + sessionStorage +
    /// service worker registrations.
    SnapshotState,
    /// Accessibility tree rendered as text.
    SnapshotAxTree,
    /// Runtime routes observed via the History API / popstate /
    /// hashchange wrappers. JSON array of `{type, url, at}`.
    SnapshotRuntimeRoutes,
    /// Runtime endpoints observed via the fetch/XHR wrappers. JSON
    /// array of `{kind, method, url, started_at, status?, ok?, duration_ms?, error?}`.
    SnapshotNetworkEndpoints,
    /// IndexedDB inventory per origin. JSON array of
    /// `{db_name, version, stores:[{name, key_path, indexes:[...]}]}`.
    SnapshotIndexedDb,
    /// Cache Storage inventory per origin. JSON array of
    /// `{cache_name, keys:[url]}`.
    SnapshotCacheStorage,
    /// Parsed Web App Manifest JSON (whatever the `<link rel=manifest>`
    /// pointed at).
    SnapshotManifest,
    /// Service worker registration bundle. JSON array of
    /// `{scope, active_script_url?, waiting_script_url?, installing_script_url?}`.
    SnapshotServiceWorkers,
    /// Unified SPA/PWA state bundle combining manifest, service workers,
    /// storage, observed routes/endpoints, and optional IndexedDB/Cache Storage.
    SnapshotPwaState,
}

impl ArtifactKind {
    /// Stable wire string: the value stored in `artifacts.kind`, shipped
    /// in `ArtifactSaved` events, and used as the filesystem segment.
    pub fn wire_str(&self) -> &'static str {
        match self {
            ArtifactKind::ScreenshotViewport => "screenshot.viewport",
            ArtifactKind::ScreenshotFullPage => "screenshot.full_page",
            ArtifactKind::ScreenshotElement => "screenshot.element",
            ArtifactKind::SnapshotHtml => "snapshot.html",
            ArtifactKind::SnapshotDom => "snapshot.dom_snapshot",
            ArtifactKind::SnapshotPostJsHtml => "snapshot.post_js_html",
            ArtifactKind::SnapshotResponseBody => "snapshot.response_body",
            ArtifactKind::SnapshotState => "snapshot.state",
            ArtifactKind::SnapshotAxTree => "snapshot.ax_tree",
            ArtifactKind::SnapshotRuntimeRoutes => "snapshot.runtime_routes",
            ArtifactKind::SnapshotNetworkEndpoints => "snapshot.network_endpoints",
            ArtifactKind::SnapshotIndexedDb => "snapshot.indexeddb",
            ArtifactKind::SnapshotCacheStorage => "snapshot.cache_storage",
            ArtifactKind::SnapshotManifest => "snapshot.manifest",
            ArtifactKind::SnapshotServiceWorkers => "snapshot.service_workers",
            ArtifactKind::SnapshotPwaState => "snapshot.pwa_state",
        }
    }

    /// Default MIME for the kind; callers may override via
    /// [`ArtifactMeta::mime`] when the payload is encoded differently
    /// (e.g. WebP instead of PNG for screenshots).
    pub fn mime(&self) -> &'static str {
        match self {
            ArtifactKind::ScreenshotViewport
            | ArtifactKind::ScreenshotFullPage
            | ArtifactKind::ScreenshotElement => "image/png",
            ArtifactKind::SnapshotHtml
            | ArtifactKind::SnapshotDom
            | ArtifactKind::SnapshotPostJsHtml => "text/html",
            ArtifactKind::SnapshotResponseBody => "application/octet-stream",
            ArtifactKind::SnapshotState => "application/json",
            ArtifactKind::SnapshotAxTree => "text/plain",
            ArtifactKind::SnapshotRuntimeRoutes
            | ArtifactKind::SnapshotNetworkEndpoints
            | ArtifactKind::SnapshotIndexedDb
            | ArtifactKind::SnapshotCacheStorage
            | ArtifactKind::SnapshotManifest
            | ArtifactKind::SnapshotServiceWorkers
            | ArtifactKind::SnapshotPwaState => "application/json",
        }
    }

    /// Default filesystem extension matching [`Self::mime`].
    pub fn extension(&self) -> &'static str {
        match self {
            ArtifactKind::ScreenshotViewport
            | ArtifactKind::ScreenshotFullPage
            | ArtifactKind::ScreenshotElement => "png",
            ArtifactKind::SnapshotHtml
            | ArtifactKind::SnapshotDom
            | ArtifactKind::SnapshotPostJsHtml => "html",
            ArtifactKind::SnapshotResponseBody => "bin",
            ArtifactKind::SnapshotState => "json",
            ArtifactKind::SnapshotAxTree => "txt",
            ArtifactKind::SnapshotRuntimeRoutes
            | ArtifactKind::SnapshotNetworkEndpoints
            | ArtifactKind::SnapshotIndexedDb
            | ArtifactKind::SnapshotCacheStorage
            | ArtifactKind::SnapshotManifest
            | ArtifactKind::SnapshotServiceWorkers
            | ArtifactKind::SnapshotPwaState => "json",
        }
    }

    /// Inverse of [`Self::wire_str`]; returns `None` on unknown input.
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "screenshot.viewport" => ArtifactKind::ScreenshotViewport,
            "screenshot.full_page" => ArtifactKind::ScreenshotFullPage,
            "screenshot.element" => ArtifactKind::ScreenshotElement,
            "snapshot.html" => ArtifactKind::SnapshotHtml,
            "snapshot.dom_snapshot" => ArtifactKind::SnapshotDom,
            "snapshot.post_js_html" => ArtifactKind::SnapshotPostJsHtml,
            "snapshot.response_body" => ArtifactKind::SnapshotResponseBody,
            "snapshot.state" => ArtifactKind::SnapshotState,
            "snapshot.ax_tree" => ArtifactKind::SnapshotAxTree,
            "snapshot.runtime_routes" => ArtifactKind::SnapshotRuntimeRoutes,
            "snapshot.network_endpoints" => ArtifactKind::SnapshotNetworkEndpoints,
            "snapshot.indexeddb" => ArtifactKind::SnapshotIndexedDb,
            "snapshot.cache_storage" => ArtifactKind::SnapshotCacheStorage,
            "snapshot.manifest" => ArtifactKind::SnapshotManifest,
            "snapshot.service_workers" => ArtifactKind::SnapshotServiceWorkers,
            "snapshot.pwa_state" => ArtifactKind::SnapshotPwaState,
            _ => return None,
        })
    }
}

/// Metadata bundle passed to [`Storage::save_artifact`]. Borrowed so
/// callers don't have to allocate for every save; the backend copies
/// what it needs into its own row/record.
#[derive(Debug, Clone)]
pub struct ArtifactMeta<'a> {
    pub url: &'a Url,
    pub final_url: Option<&'a Url>,
    pub session_id: &'a str,
    pub kind: ArtifactKind,
    /// Operator-provided label or auto-generated `step_<id>_<kind>`.
    pub name: Option<&'a str>,
    /// Populated when the artifact is produced by a ScriptSpec step.
    pub step_id: Option<&'a str>,
    pub step_kind: Option<&'a str>,
    /// Populated when `kind == ScreenshotElement` or similar.
    pub selector: Option<&'a str>,
    /// Override the default MIME implied by `kind`.
    pub mime: Option<&'a str>,
}

/// Row returned from [`Storage::list_artifacts`].
#[derive(Debug, Clone)]
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
    pub mime: String,
    pub sha256: String,
    pub size: u64,
    pub created_at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct PageMetadata {
    pub final_url: Url,
    pub status: u16,
    pub bytes: u64,
    pub rendered: bool,
    pub kind: crate::discovery::AssetKind,
}

#[derive(Debug, Clone, Default)]
pub struct HostFacts {
    pub favicon_mmh3: Option<i32>,
    pub dns_json: Option<String>,
    pub robots_present: Option<bool>,
    pub manifest_present: Option<bool>,
    pub service_worker_present: Option<bool>,
    pub cert_sha256: Option<String>,
    pub cert_subject_cn: Option<String>,
    pub cert_issuer_cn: Option<String>,
    pub cert_not_before: Option<String>,
    pub cert_not_after: Option<String>,
    pub cert_sans_json: Option<String>,
    pub rdap_json: Option<String>,
    pub registrar: Option<String>,
    pub registrant_org: Option<String>,
    pub registration_created: Option<String>,
    pub registration_expires: Option<String>,
}

#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    async fn save_raw(&self, url: &Url, headers: &HeaderMap, body: &Bytes) -> Result<()>;
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
    async fn save_rendered(&self, url: &Url, html_post_js: &str, meta: &PageMetadata)
        -> Result<()>;
    async fn save_edge(&self, from: &Url, to: &Url) -> Result<()>;

    async fn save_host_facts(&self, _host: &str, _facts: &HostFacts) -> Result<()> {
        Ok(())
    }

    async fn save_metrics(&self, _url: &Url, _metrics: &crate::metrics::PageMetrics) -> Result<()> {
        Ok(())
    }

    async fn save_screenshot(&self, url: &Url, png: &[u8]) -> Result<()> {
        // Default: delegate through `save_artifact` using FullPage as the
        // historical contract — callers that knew the old API wrote
        // whole-page PNGs. Backends that want the per-URL legacy behaviour
        // override this.
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

    /// Persist a single artifact (screenshot, snapshot, state dump).
    /// Backends that don't track artifacts return `Ok(())` by default.
    async fn save_artifact(&self, _meta: &ArtifactMeta<'_>, _bytes: &[u8]) -> Result<()> {
        Ok(())
    }

    /// List persisted artifacts filtered by optional `session_id` and
    /// optional `kind`. Default returns empty — backends that maintain
    /// the `artifacts` table override.
    async fn list_artifacts(
        &self,
        _session_id: Option<&str>,
        _kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRow>> {
        Ok(Vec::new())
    }

    /// Persist a browser-session state snapshot.
    ///
    /// `session_id` is the stable handle the crawler uses to reuse the
    /// same BrowserContext across pages; `state_json` is the opaque
    /// payload produced by `render::session::capture_state` — typically
    /// `{cookies, localStorage, sessionStorage, serviceWorkerRegs}`.
    /// Backends that don't care about stateful crawls return `Ok(())` by
    /// default; the SQLite backend persists for resume, and filesystem
    /// writes one JSON file per session_id.
    async fn save_state(&self, _session_id: &str, _state_json: &str) -> Result<()> {
        Ok(())
    }

    /// Load a previously saved state snapshot. Returns `None` when the
    /// session_id is unknown or the backend doesn't persist state.
    async fn load_state(&self, _session_id: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Persist a detected antibot challenge. Backends that don't care
    /// about challenge telemetry return `Ok(())`; the SQLite backend
    /// writes a row to `challenge_events`.
    async fn record_challenge(&self, _signal: &crate::antibot::ChallengeSignal) -> Result<()> {
        Ok(())
    }

    /// Persist a batch of classified `AssetRef`s extracted from a page.
    /// Backends that don't care return `Ok(())`; the SQLite backend
    /// upserts `asset_refs` and increments `external_domains.ref_count`.
    async fn save_asset_refs(
        &self,
        _refs: &[crate::discovery::asset_refs::AssetRef],
    ) -> Result<()> {
        Ok(())
    }

    /// Persist a per-page technology fingerprint report and update any
    /// backend-specific host/domain rollups.
    async fn save_tech_fingerprint(
        &self,
        _report: &crate::discovery::tech_fingerprint::TechFingerprintReport,
    ) -> Result<()> {
        Ok(())
    }

    /// Persist a passive vendor-telemetry observation (P0-9). Backends
    /// that don't track telemetry return `Ok(())` by default; the
    /// SQLite backend writes a row to `vendor_telemetry`.
    async fn record_telemetry(
        &self,
        _telem: &crate::antibot::telemetry::VendorTelemetry,
    ) -> Result<()> {
        Ok(())
    }

    /// Load every challenge observed for a given session_id, ordered by
    /// observed_at ascending. Default empty.
    async fn session_challenges(
        &self,
        _session_id: &str,
    ) -> Result<Vec<crate::antibot::ChallengeSignal>> {
        Ok(Vec::new())
    }

    /// Persist an archived session entry (Fase 6). Default no-op; the
    /// SQLite backend writes to `sessions_archive`. Backends that don't
    /// care still compile cleanly without needing to know about the
    /// `SessionEntry` shape.
    async fn archive_session(
        &self,
        _entry: &crate::identity::SessionEntry,
        _reason: crate::identity::EvictionReason,
    ) -> Result<()> {
        Ok(())
    }

    fn as_any_ref(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// Fallback session id for legacy `save_screenshot` calls: derive a
/// stable host-scoped token so old callers that never carried a
/// session_id still get grouped consistently in the artifacts table.
pub(crate) fn session_id_for_url(url: &Url) -> String {
    url.host_str()
        .map(|h| format!("legacy:{h}"))
        .unwrap_or_else(|| "legacy:unknown".to_string())
}
