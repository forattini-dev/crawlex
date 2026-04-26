use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;
use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

use crate::config::ContentStoreConfig;
use crate::storage::{
    ArtifactKind, ArtifactMeta, ArtifactRow, ArtifactStorage, ChallengeStorage, IntelStorage,
    PageMetadata, StateStorage, Storage, TelemetryStorage,
};
use crate::{Error, Result};

pub struct FilesystemStorage {
    root: PathBuf,
    blob_root: PathBuf,
    meta_file: Mutex<File>,
    edges_file: Mutex<File>,
    tech_file: Mutex<File>,
}

#[derive(Serialize)]
struct MetaRecord<'a> {
    url: &'a str,
    final_url: &'a str,
    status: u16,
    bytes: u64,
    rendered: bool,
    sha256: &'a str,
    rel_path: &'a str,
    kind: &'a str,
    truncated: bool,
}

impl FilesystemStorage {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_content_store(root, &ContentStoreConfig::default())
    }

    pub fn open_with_content_store(
        root: impl AsRef<Path>,
        content_store: &ContentStoreConfig,
    ) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let blob_root = content_store
            .root
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("blobs"));
        fs::create_dir_all(&blob_root).map_err(|e| Error::Storage(format!("mkdir blobs: {e}")))?;
        fs::create_dir_all(root.join("state"))
            .map_err(|e| Error::Storage(format!("mkdir state: {e}")))?;
        fs::create_dir_all(root.join("artifacts"))
            .map_err(|e| Error::Storage(format!("mkdir artifacts: {e}")))?;
        let meta_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("metadata.jsonl"))
            .map_err(|e| Error::Storage(format!("open metadata: {e}")))?;
        let edges_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("edges.jsonl"))
            .map_err(|e| Error::Storage(format!("open edges: {e}")))?;
        let tech_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("tech_fingerprints.jsonl"))
            .map_err(|e| Error::Storage(format!("open tech fingerprints: {e}")))?;
        Ok(Self {
            root,
            blob_root,
            meta_file: Mutex::new(meta_file),
            edges_file: Mutex::new(edges_file),
            tech_file: Mutex::new(tech_file),
        })
    }

    fn write_sharded_at(
        storage_root: &Path,
        blob_root: &Path,
        subdir: &str,
        hash_hex: &str,
        data: &[u8],
    ) -> Result<String> {
        let shard = &hash_hex[..2.min(hash_hex.len())];
        let rel_in_blob = PathBuf::from(subdir).join(shard).join(hash_hex);
        let path = blob_root.join(&rel_in_blob);
        let Some(dir) = path.parent() else {
            return Err(Error::Storage("blob path has no parent".into()));
        };
        fs::create_dir_all(dir).map_err(|e| Error::Storage(format!("mkdir shard: {e}")))?;
        if !path.exists() {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
            fs::write(&tmp, data).map_err(|e| Error::Storage(format!("write: {e}")))?;
            if path.exists() {
                let _ = fs::remove_file(&tmp);
            } else if let Err(e) = fs::rename(&tmp, &path) {
                if path.exists() {
                    let _ = fs::remove_file(&tmp);
                } else {
                    return Err(Error::Storage(format!("rename: {e}")));
                }
            }
        }
        let display_path = path.strip_prefix(storage_root).unwrap_or(&path);
        Ok(display_path.to_string_lossy().to_string())
    }

    fn append_meta(&self, rec: &MetaRecord<'_>) -> Result<()> {
        let line = serde_json::to_string(rec).map_err(|e| Error::Storage(format!("json: {e}")))?;
        let mut f = self.meta_file.lock();
        writeln!(f, "{line}").map_err(|e| Error::Storage(format!("append: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl ArtifactStorage for FilesystemStorage {
    async fn save_raw(&self, url: &Url, headers: &HeaderMap, body: &Bytes) -> Result<()> {
        self.save_raw_response(url, url, 0, headers, body, false)
            .await
    }

    async fn save_raw_response(
        &self,
        url: &Url,
        final_url: &Url,
        status: u16,
        _headers: &HeaderMap,
        body: &Bytes,
        truncated: bool,
    ) -> Result<()> {
        let root = self.root.clone();
        let blob_root = self.blob_root.clone();
        let body = body.clone();
        let body_len = body.len() as u64;
        let (hash, rel) = tokio::task::spawn_blocking(move || -> Result<(String, String)> {
            let hash = hex::encode(Sha256::digest(&body));
            let rel = FilesystemStorage::write_sharded_at(&root, &blob_root, "raw", &hash, &body)?;
            Ok((hash, rel))
        })
        .await
        .map_err(|e| Error::Storage(format!("raw write join: {e}")))??;
        let url_s = url.to_string();
        let final_s = final_url.to_string();
        self.append_meta(&MetaRecord {
            url: &url_s,
            final_url: &final_s,
            status,
            bytes: body_len,
            rendered: false,
            sha256: &hash,
            rel_path: &rel,
            kind: "raw",
            truncated,
        })
    }

    async fn save_rendered(
        &self,
        url: &Url,
        html_post_js: &str,
        meta: &PageMetadata,
    ) -> Result<()> {
        let root = self.root.clone();
        let blob_root = self.blob_root.clone();
        let html = html_post_js.as_bytes().to_vec();
        let (hash, rel) = tokio::task::spawn_blocking(move || -> Result<(String, String)> {
            let hash = hex::encode(Sha256::digest(&html));
            let rel = FilesystemStorage::write_sharded_at(&root, &blob_root, "html", &hash, &html)?;
            Ok((hash, rel))
        })
        .await
        .map_err(|e| Error::Storage(format!("rendered write join: {e}")))??;
        let url_s = url.to_string();
        let final_s = meta.final_url.to_string();
        self.append_meta(&MetaRecord {
            url: &url_s,
            final_url: &final_s,
            status: meta.status,
            bytes: meta.bytes,
            rendered: meta.rendered,
            sha256: &hash,
            rel_path: &rel,
            kind: "rendered",
            truncated: false,
        })
    }

    async fn save_screenshot(&self, url: &Url, png: &[u8]) -> Result<Option<String>> {
        let root = self.root.clone();
        let blob_root = self.blob_root.clone();
        let png_owned = png.to_vec();
        let png_len = png_owned.len() as u64;
        let (hash, rel) = tokio::task::spawn_blocking(move || -> Result<(String, String)> {
            let hash = hex::encode(Sha256::digest(&png_owned));
            let rel = FilesystemStorage::write_sharded_at(
                &root,
                &blob_root,
                "screenshots",
                &hash,
                &png_owned,
            )?;
            Ok((hash, rel))
        })
        .await
        .map_err(|e| Error::Storage(format!("screenshot write join: {e}")))??;
        let url_s = url.to_string();
        self.append_meta(&MetaRecord {
            url: &url_s,
            final_url: &url_s,
            status: 200,
            bytes: png_len,
            rendered: true,
            sha256: &hash,
            rel_path: &rel,
            kind: "screenshot",
            truncated: false,
        })?;
        // Mirror into the unified artifacts layout so `list_artifacts`
        // surfaces the legacy per-URL saves alongside ScriptRunner-driven
        // artifacts. Both writes are content-addressed so a repeat save
        // of the same PNG is a cheap no-op on the second write.
        let session_id = crate::storage::session_id_for_url(url);
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

    async fn save_artifact(&self, meta: &ArtifactMeta<'_>, bytes: &[u8]) -> Result<Option<String>> {
        let session_id = meta.session_id;
        if session_id.contains('/') || session_id.contains('\\') || session_id.contains("..") {
            return Err(Error::Storage(format!(
                "invalid session_id: {session_id:?}"
            )));
        }
        #[derive(Serialize)]
        struct Sidecar {
            url: String,
            final_url: Option<String>,
            session_id: String,
            kind: &'static str,
            name: Option<String>,
            step_id: Option<String>,
            step_kind: Option<String>,
            selector: Option<String>,
            mime: String,
            size: u64,
            sha256: String,
            created_at_ms: u128,
        }
        let root = self.root.clone();
        let bytes = bytes.to_vec();
        let url_s = meta.url.to_string();
        let final_url_s = meta.final_url.map(|u| u.to_string());
        let session_id = session_id.to_string();
        let kind = meta.kind;
        let name = meta.name.map(str::to_string);
        let step_id = meta.step_id.map(str::to_string);
        let step_kind = meta.step_kind.map(str::to_string);
        let selector = meta.selector.map(str::to_string);
        let mime = meta.mime.unwrap_or(meta.kind.mime()).to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<String>> {
            // Disk layout:
            //   artifacts/<session>/<ts>_<kind>_<sha8>.<ext>     bytes
            //   artifacts/<session>/<ts>_<kind>_<sha8>.meta.json sidecar
            let sha = hex::encode(Sha256::digest(&bytes));
            let sha8 = &sha[..8];
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let kind_wire = kind.wire_str();
            // Sanitise the kind so `screenshot.full_page` maps cleanly to
            // a filename without creating a phantom extension.
            let kind_fs = kind_wire.replace('.', "_");
            let stem = format!("{ts}_{kind_fs}_{sha8}");
            let dir = root.join("artifacts").join(&session_id);
            fs::create_dir_all(&dir).map_err(|e| Error::Storage(format!("mkdir artifact: {e}")))?;

            let bytes_path = dir.join(format!("{stem}.{}", kind.extension()));
            let tmp = bytes_path.with_extension(format!("{}.tmp", kind.extension()));
            fs::write(&tmp, &bytes).map_err(|e| Error::Storage(format!("artifact write: {e}")))?;
            fs::rename(&tmp, &bytes_path)
                .map_err(|e| Error::Storage(format!("artifact rename: {e}")))?;

            let sidecar = Sidecar {
                url: url_s,
                final_url: final_url_s,
                session_id: session_id.clone(),
                kind: kind_wire,
                name,
                step_id,
                step_kind,
                selector,
                mime,
                size: bytes.len() as u64,
                sha256: sha,
                created_at_ms: ts,
            };
            let meta_path = dir.join(format!("{stem}.meta.json"));
            let meta_tmp = dir.join(format!("{stem}.meta.json.tmp"));
            let json =
                serde_json::to_vec(&sidecar).map_err(|e| Error::Storage(format!("json: {e}")))?;
            fs::write(&meta_tmp, &json)
                .map_err(|e| Error::Storage(format!("sidecar write: {e}")))?;
            fs::rename(&meta_tmp, &meta_path)
                .map_err(|e| Error::Storage(format!("sidecar rename: {e}")))?;
            // Return the path relative to the storage root so consumers
            // can join it with their known root without exposing absolute
            // host paths in the NDJSON stream.
            let rel = bytes_path
                .strip_prefix(&root)
                .unwrap_or(&bytes_path)
                .to_string_lossy()
                .to_string();
            Ok(Some(rel))
        })
        .await
        .map_err(|e| Error::Storage(format!("artifact write join: {e}")))?
    }

    async fn list_artifacts(
        &self,
        session_id: Option<&str>,
        kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRow>> {
        // Scan every sidecar in `artifacts/*/*meta.json`, deserialise,
        // filter. Not an ideal primary-key query but the filesystem backend
        // is dev/debug oriented; sqlite is the "real" indexed store.
        let root = self.root.join("artifacts");
        let sid_filter = session_id.map(|s| s.to_string());
        let kind_filter = kind;
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<ArtifactRow>> {
            let mut out = Vec::new();
            let session_iter = match fs::read_dir(&root) {
                Ok(it) => it,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
                Err(e) => return Err(Error::Storage(format!("artifacts scan: {e}"))),
            };
            for session_ent in session_iter.flatten() {
                let session_path = session_ent.path();
                if !session_path.is_dir() {
                    continue;
                }
                let session_name = session_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(f) = &sid_filter {
                    if session_name != *f {
                        continue;
                    }
                }
                let Ok(file_iter) = fs::read_dir(&session_path) else {
                    continue;
                };
                for ent in file_iter.flatten() {
                    let path = ent.path();
                    let is_sidecar = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(|s| s.ends_with(".meta.json"))
                        .unwrap_or(false);
                    if !is_sidecar {
                        continue;
                    }
                    let Ok(bytes) = fs::read(&path) else { continue };
                    let Ok(v): std::result::Result<serde_json::Value, _> =
                        serde_json::from_slice(&bytes)
                    else {
                        continue;
                    };
                    let kind_s = v.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                    let Some(k) = ArtifactKind::from_wire(kind_s) else {
                        continue;
                    };
                    if let Some(kf) = kind_filter {
                        if kf != k {
                            continue;
                        }
                    }
                    let url_s = v.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let Ok(url) = url::Url::parse(url_s) else {
                        continue;
                    };
                    let final_url = v
                        .get("final_url")
                        .and_then(|v| v.as_str())
                        .and_then(|s| url::Url::parse(s).ok());
                    let name = v
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let step_id = v
                        .get("step_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let step_kind = v
                        .get("step_kind")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let selector = v
                        .get("selector")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let mime = v
                        .get("mime")
                        .and_then(|v| v.as_str())
                        .unwrap_or(k.mime())
                        .to_string();
                    let size = v.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                    let sha256 = v
                        .get("sha256")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let created_at_ms =
                        v.get("created_at_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                    let created_at =
                        std::time::UNIX_EPOCH + std::time::Duration::from_millis(created_at_ms);
                    out.push(ArtifactRow {
                        id: 0,
                        url,
                        final_url,
                        session_id: session_name.clone(),
                        kind: k,
                        name,
                        step_id,
                        step_kind,
                        selector,
                        mime,
                        sha256,
                        size,
                        created_at,
                    });
                }
            }
            // Stable order: created_at_ms ascending, best-effort.
            out.sort_by_key(|r| r.created_at);
            Ok(out)
        })
        .await
        .map_err(|e| Error::Storage(format!("artifacts join: {e}")))??;
        Ok(rows)
    }

    async fn save_edge(&self, from: &Url, to: &Url) -> Result<()> {
        #[derive(Serialize)]
        struct Edge<'a> {
            src: &'a str,
            dst: &'a str,
        }
        let e = Edge {
            src: from.as_str(),
            dst: to.as_str(),
        };
        let line = serde_json::to_string(&e).map_err(|e| Error::Storage(format!("json: {e}")))?;
        let mut f = self.edges_file.lock();
        writeln!(f, "{line}").map_err(|e| Error::Storage(format!("append: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl StateStorage for FilesystemStorage {
    /// Write the state JSON as `state/<session_id>.json`. Overwrites on
    /// each call — state progresses, we don't keep a history here.
    /// Fancier retention (SQLite + cleanup) is the persistent-sessions
    /// job in Phase 3.
    async fn save_state(&self, session_id: &str, state_json: &str) -> Result<()> {
        // Session IDs are UUID-ish; reject anything containing a path
        // separator to stop a malicious `session_id` from writing
        // outside the state dir.
        if session_id.contains('/') || session_id.contains('\\') || session_id.contains("..") {
            return Err(Error::Storage(format!(
                "invalid session_id: {session_id:?}"
            )));
        }
        let root = self.root.clone();
        let session_id = session_id.to_string();
        let state_json = state_json.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let path = root.join("state").join(format!("{session_id}.json"));
            // Atomic write: write-to-tmp, rename. Prevents a crash mid-write
            // from corrupting a session's state file and poisoning resume.
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, state_json)
                .map_err(|e| Error::Storage(format!("state write: {e}")))?;
            std::fs::rename(&tmp, &path)
                .map_err(|e| Error::Storage(format!("state rename: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Storage(format!("state write join: {e}")))?
    }

    async fn load_state(&self, session_id: &str) -> Result<Option<String>> {
        if session_id.contains('/') || session_id.contains('\\') || session_id.contains("..") {
            return Err(Error::Storage(format!(
                "invalid session_id: {session_id:?}"
            )));
        }
        let root = self.root.clone();
        let session_id = session_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<String>> {
            let path = root.join("state").join(format!("{session_id}.json"));
            match std::fs::read_to_string(&path) {
                Ok(s) => Ok(Some(s)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(Error::Storage(format!("state read: {e}"))),
            }
        })
        .await
        .map_err(|e| Error::Storage(format!("state read join: {e}")))?
    }
}

impl ChallengeStorage for FilesystemStorage {}
impl TelemetryStorage for FilesystemStorage {}

#[async_trait]
impl IntelStorage for FilesystemStorage {
    async fn save_tech_fingerprint(
        &self,
        report: &crate::discovery::tech_fingerprint::TechFingerprintReport,
    ) -> Result<()> {
        let line = serde_json::to_string(report)
            .map_err(|e| Error::Storage(format!("tech fingerprint json: {e}")))?;
        let mut f = self.tech_file.lock();
        writeln!(f, "{line}")
            .map_err(|e| Error::Storage(format!("append tech fingerprint: {e}")))?;
        Ok(())
    }
}

impl Storage for FilesystemStorage {}
