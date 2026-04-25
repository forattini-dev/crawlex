//! Single-writer SQLite storage.
//!
//! A dedicated OS thread owns the Connection and drains a bounded mpsc queue.
//! Tokio tasks post ops and never touch sqlite directly, so:
//! * No per-op mutex contention.
//! * Writes coalesce inside a single transaction per batch → 2-5x throughput.
//! * WAL + synchronous=NORMAL → durable across crashes, lock-free readers.

use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;
use url::Url;

use crate::config::ContentStoreConfig;
use crate::storage::{ArtifactKind, ArtifactMeta, ArtifactRow, HostFacts, PageMetadata, Storage};
use crate::{Error, Result};

enum Op {
    SaveRaw {
        url: String,
        final_url: String,
        status: i64,
        headers_json: String,
        body: Option<Vec<u8>>,
        body_size: i64,
        sha256: String,
        blob_path: Option<String>,
        mime: String,
        body_truncated: i64,
        kind: String,
    },
    SaveRendered {
        url: String,
        final_url: String,
        status: i64,
        bytes: i64,
        rendered: i64,
        sha256: String,
        html: Option<String>,
        blob_size: i64,
        blob_path: Option<String>,
        kind: String,
    },
    SaveEdge {
        src: String,
        dst: String,
    },
    SaveHostFacts {
        host: String,
        favicon_mmh3: Option<i32>,
        dns_json: Option<String>,
        robots_present: Option<i64>,
        manifest_present: Option<i64>,
        service_worker_present: Option<i64>,
        cert_sha256: Option<String>,
        cert_subject_cn: Option<String>,
        cert_issuer_cn: Option<String>,
        cert_not_before: Option<String>,
        cert_not_after: Option<String>,
        cert_sans_json: Option<String>,
        rdap_json: Option<String>,
        registrar: Option<String>,
        registrant_org: Option<String>,
        registration_created: Option<String>,
        registration_expires: Option<String>,
    },
    SaveMetrics(Box<MetricsRow>),
    SaveScreenshot {
        url: String,
        sha256: String,
        bytes: i64,
        png: Vec<u8>,
    },
    SaveState {
        session_id: String,
        state_json: String,
    },
    SaveProxyScores {
        rows: Vec<ProxyScoreRow>,
    },
    SaveHostAffinity {
        entries: Vec<(String, i64, String)>,
    },
    SaveArtifact {
        url: String,
        final_url: Option<String>,
        session_id: String,
        kind: String,
        name: Option<String>,
        step_id: Option<String>,
        step_kind: Option<String>,
        selector: Option<String>,
        mime: String,
        size: i64,
        sha256: String,
        bytes: Vec<u8>,
        created_at: i64,
    },
    RecordChallenge {
        session_id: String,
        vendor: String,
        level: String,
        url: String,
        origin: String,
        proxy: Option<String>,
        observed_at: i64,
        metadata: Option<String>,
    },
    SaveAssetRefs {
        refs: Vec<AssetRefRow>,
    },
    SaveTechFingerprint {
        url: String,
        final_url: String,
        host: String,
        report_json: String,
        generated_at: i64,
    },
    RecordTelemetry {
        session_id: String,
        vendor: String,
        endpoint: String,
        method: String,
        payload_size: i64,
        payload_shape: String,
        pattern_label: String,
        observed_at: i64,
    },
    ArchiveSession {
        session_id: String,
        scope: String,
        scope_key: String,
        state: String,
        bundle_id: Option<i64>,
        created_at: i64,
        ended_at: i64,
        urls_visited: i64,
        challenges: i64,
        final_proxy: Option<String>,
        reason: String,
    },
}

/// Wire-ready archive row. Mirrors the `sessions_archive` table.
#[derive(Debug, Clone)]
pub struct ArchivedSessionRow {
    pub session_id: String,
    pub scope: String,
    pub scope_key: String,
    pub state: String,
    pub bundle_id: Option<i64>,
    pub created_at: i64,
    pub ended_at: i64,
    pub urls_visited: i64,
    pub challenges: i64,
    pub final_proxy: Option<String>,
    pub reason: String,
}

/// Row shape pushed through the writer thread for each classified
/// outbound asset reference. Mirrors the `asset_refs` table columns
/// 1:1; the mpsc Op carries a `Vec<Self>` batch per page.
pub struct AssetRefRow {
    pub from_page_url: String,
    pub to_url: String,
    pub to_domain: String,
    pub kind: String,
    pub is_internal: bool,
}

pub struct ProxyScoreRow {
    pub url: String,
    pub success: i64,
    pub timeouts: i64,
    pub resets: i64,
    pub status_4xx: i64,
    pub status_5xx: i64,
    pub challenge_hits: i64,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    pub last_success_at: Option<i64>,
    pub quarantine_until: Option<i64>,
}

struct MetricsRow {
    url: String,
    dns_ms: Option<i64>,
    tcp_connect_ms: Option<i64>,
    tls_handshake_ms: Option<i64>,
    ttfb_ms: Option<i64>,
    download_ms: Option<i64>,
    total_ms: Option<i64>,
    status: Option<i64>,
    bytes: Option<i64>,
    alpn: Option<String>,
    tls_version: Option<String>,
    cipher: Option<String>,
    dom_content_loaded_ms: Option<f64>,
    load_event_ms: Option<f64>,
    first_paint_ms: Option<f64>,
    first_contentful_paint_ms: Option<f64>,
    largest_contentful_paint_ms: Option<f64>,
    cumulative_layout_shift: Option<f64>,
    total_blocking_time_ms: Option<f64>,
    longest_task_ms: Option<f64>,
    dom_nodes: Option<i64>,
    js_heap_used_bytes: Option<i64>,
    js_heap_total_bytes: Option<i64>,
    resource_count: Option<i64>,
    total_transfer_bytes: Option<i64>,
    total_decoded_bytes: Option<i64>,
    transfer_by_type_json: Option<String>,
    resources_json: Option<String>,
}

pub struct SqliteStorage {
    tx: mpsc::Sender<Op>,
    /// DB file path, retained so `load_state` (a concurrent-safe read
    /// path) can open its own read-only connection without going
    /// through the writer thread.
    path: PathBuf,
    blob_root: PathBuf,
    content_store_enabled: bool,
    inline_legacy_columns: bool,
}

fn default_blob_root(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| {
            let mut s = n.to_os_string();
            s.push(".blobs");
            s
        })
        .unwrap_or_else(|| "crawlex.sqlite.blobs".into());
    path.with_file_name(name)
}

impl SqliteStorage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_content_store(path, &ContentStoreConfig::default())
    }

    pub fn open_with_content_store(
        path: impl AsRef<Path>,
        content_store: &ContentStoreConfig,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let blob_root = content_store
            .root
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| default_blob_root(&path));
        let content_store_enabled = content_store.enabled;
        let inline_legacy_columns = content_store.inline_legacy_columns || !content_store_enabled;
        let (ready_tx, ready_rx) = std_mpsc::channel::<std::result::Result<(), String>>();
        let (tx, mut rx) = mpsc::channel::<Op>(4096);

        let writer_path = path.clone();
        thread::Builder::new()
            .name("crawlex-sqlite".into())
            .spawn(move || {
                let conn = match Self::init_db(&writer_path) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                Self::run(conn, &mut rx);
            })
            .map_err(|e| Error::Storage(format!("spawn writer: {e}")))?;

        ready_rx
            .recv()
            .map_err(|e| Error::Storage(format!("writer handshake: {e}")))?
            .map_err(Error::Storage)?;

        Ok(Self {
            tx,
            path,
            blob_root,
            content_store_enabled,
            inline_legacy_columns,
        })
    }

    fn init_db(path: &Path) -> std::result::Result<Connection, String> {
        let conn = Connection::open(path).map_err(|e| format!("open: {e}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("wal: {e}"))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| format!("sync: {e}"))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS pages (
                url TEXT PRIMARY KEY,
                final_url TEXT NOT NULL,
                status INTEGER NOT NULL,
                bytes INTEGER NOT NULL,
                rendered INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                body BLOB,
                html TEXT,
                body_sha256 TEXT,
                html_sha256 TEXT,
                body_blob_path TEXT,
                html_blob_path TEXT,
                body_size INTEGER,
                html_size INTEGER,
                body_mime TEXT,
                html_mime TEXT,
                body_truncated INTEGER NOT NULL DEFAULT 0,
                headers_json TEXT,
                kind TEXT,
                favicon_mmh3 INTEGER,
                saved_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_pages_kind ON pages(kind);
            CREATE TABLE IF NOT EXISTS content_blobs (
                sha256 TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                mime TEXT NOT NULL,
                size INTEGER NOT NULL,
                path TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_content_blobs_kind ON content_blobs(kind);
            CREATE TABLE IF NOT EXISTS host_facts (
                host TEXT PRIMARY KEY,
                favicon_mmh3 INTEGER,
                dns_json TEXT,
                robots_present INTEGER,
                manifest_present INTEGER,
                service_worker_present INTEGER,
                cert_sha256 TEXT,
                cert_subject_cn TEXT,
                cert_issuer_cn TEXT,
                cert_not_before TEXT,
                cert_not_after TEXT,
                cert_sans_json TEXT,
                rdap_json TEXT,
                registrar TEXT,
                registrant_org TEXT,
                registration_created TEXT,
                registration_expires TEXT,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_host_facts_cert_sha256 ON host_facts(cert_sha256);
            CREATE INDEX IF NOT EXISTS idx_host_facts_favicon ON host_facts(favicon_mmh3);
            CREATE TABLE IF NOT EXISTS page_metrics (
                url TEXT PRIMARY KEY,
                dns_ms INTEGER, tcp_connect_ms INTEGER, tls_handshake_ms INTEGER,
                ttfb_ms INTEGER, download_ms INTEGER, total_ms INTEGER,
                status INTEGER, bytes INTEGER, alpn TEXT, tls_version TEXT, cipher TEXT,
                dom_content_loaded_ms REAL, load_event_ms REAL,
                first_paint_ms REAL, first_contentful_paint_ms REAL,
                largest_contentful_paint_ms REAL, cumulative_layout_shift REAL,
                total_blocking_time_ms REAL, longest_task_ms REAL,
                dom_nodes INTEGER, js_heap_used_bytes INTEGER,
                js_heap_total_bytes INTEGER, resource_count INTEGER,
                total_transfer_bytes INTEGER, total_decoded_bytes INTEGER,
                transfer_by_type_json TEXT,
                resources_json TEXT,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE TABLE IF NOT EXISTS screenshots (
                url TEXT PRIMARY KEY,
                sha256 TEXT NOT NULL,
                bytes INTEGER NOT NULL,
                png BLOB NOT NULL,
                saved_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_screenshots_sha256 ON screenshots(sha256);
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                state_json TEXT NOT NULL,
                saved_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE TABLE IF NOT EXISTS edges (
                src TEXT NOT NULL,
                dst TEXT NOT NULL,
                weight INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY (src, dst)
            );
            CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(src);
            CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst);
            CREATE TABLE IF NOT EXISTS proxy_scores (
                url TEXT PRIMARY KEY,
                success INTEGER NOT NULL DEFAULT 0,
                timeouts INTEGER NOT NULL DEFAULT 0,
                resets INTEGER NOT NULL DEFAULT 0,
                status_4xx INTEGER NOT NULL DEFAULT 0,
                status_5xx INTEGER NOT NULL DEFAULT 0,
                challenge_hits INTEGER NOT NULL DEFAULT 0,
                latency_p50_ms REAL,
                latency_p95_ms REAL,
                last_success_at INTEGER,
                quarantine_until INTEGER,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE TABLE IF NOT EXISTS challenge_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                vendor TEXT NOT NULL,
                level TEXT NOT NULL,
                url TEXT NOT NULL,
                origin TEXT NOT NULL,
                proxy TEXT,
                observed_at INTEGER NOT NULL,
                metadata TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_challenge_session ON challenge_events(session_id);
            CREATE INDEX IF NOT EXISTS idx_challenge_vendor ON challenge_events(vendor);
            CREATE INDEX IF NOT EXISTS idx_challenge_observed ON challenge_events(observed_at);
            -- Operator dashboards. `strftime('%s','now') - 86400` is the cutoff
            -- for "last 24h" — computed at query time so the views stay honest
            -- as the clock advances. Proxy bucketing collapses NULL to 'direct'
            -- so the aggregation key is always non-null and printable.
            CREATE VIEW IF NOT EXISTS v_challenge_rate_by_vendor AS
                SELECT
                    vendor,
                    COUNT(*) AS total,
                    SUM(CASE WHEN observed_at >= strftime('%s','now') - 86400 THEN 1 ELSE 0 END) AS last_24h
                FROM challenge_events
                GROUP BY vendor
                ORDER BY total DESC;
            CREATE VIEW IF NOT EXISTS v_challenge_rate_by_proxy AS
                SELECT
                    COALESCE(origin, 'direct') AS proxy,
                    COUNT(*) AS total,
                    SUM(CASE WHEN observed_at >= strftime('%s','now') - 86400 THEN 1 ELSE 0 END) AS last_24h
                FROM (
                    SELECT
                        CASE
                            WHEN proxy IS NULL OR proxy = '' THEN NULL
                            WHEN instr(substr(proxy, instr(proxy, '://') + 3), '/') > 0
                                THEN substr(proxy, 1, instr(proxy, '://') + 2
                                    + instr(substr(proxy, instr(proxy, '://') + 3), '/') - 1)
                            ELSE proxy
                        END AS origin,
                        observed_at
                    FROM challenge_events
                )
                GROUP BY proxy
                ORDER BY total DESC;
            CREATE VIEW IF NOT EXISTS v_challenge_rate_by_session AS
                SELECT
                    session_id,
                    COUNT(*) AS total,
                    SUM(CASE WHEN observed_at >= strftime('%s','now') - 86400 THEN 1 ELSE 0 END) AS last_24h
                FROM challenge_events
                GROUP BY session_id
                ORDER BY total DESC;
            CREATE TABLE IF NOT EXISTS vendor_telemetry (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                vendor TEXT NOT NULL,
                endpoint TEXT NOT NULL,
                method TEXT NOT NULL,
                payload_size INTEGER,
                payload_shape TEXT,
                pattern_label TEXT,
                observed_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_vendor_telem_session ON vendor_telemetry(session_id);
            CREATE INDEX IF NOT EXISTS idx_vendor_telem_vendor ON vendor_telemetry(vendor);
            CREATE INDEX IF NOT EXISTS idx_vendor_telem_observed ON vendor_telemetry(observed_at);
            CREATE TABLE IF NOT EXISTS artifacts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT NOT NULL,
                final_url TEXT,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT,
                step_id TEXT,
                step_kind TEXT,
                selector TEXT,
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
            CREATE TABLE IF NOT EXISTS sessions_archive (
                id TEXT PRIMARY KEY,
                scope TEXT NOT NULL,
                scope_key TEXT NOT NULL,
                state TEXT NOT NULL,
                bundle_id INTEGER,
                created_at INTEGER NOT NULL,
                ended_at INTEGER NOT NULL,
                urls_visited INTEGER NOT NULL,
                challenges INTEGER NOT NULL,
                final_proxy TEXT,
                reason TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_archive_state ON sessions_archive(state);
            CREATE INDEX IF NOT EXISTS idx_sessions_archive_ended ON sessions_archive(ended_at);
            CREATE TABLE IF NOT EXISTS host_affinity (
                host TEXT NOT NULL,
                bundle_id INTEGER NOT NULL,
                proxy_url TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                PRIMARY KEY (host, bundle_id)
            );

            -- Antibot cookie pinning (vendor-specific replay).
            -- Populated by `crate::antibot::bypass::pin_captured` when the
            -- operator opts into `--antibot-bypass replay`/`aggressive`.
            -- The matching Rust-side store lives at
            -- `crate::antibot::cookie_pin::SqliteCookiePinStore`; both
            -- sides create the table idempotently so order does not
            -- matter.
            CREATE TABLE IF NOT EXISTS antibot_cookie_cache (
                vendor      TEXT NOT NULL,
                origin      TEXT NOT NULL,
                cookie_name TEXT NOT NULL,
                value       TEXT NOT NULL,
                pinned_at   INTEGER NOT NULL,
                ttl_secs    INTEGER NOT NULL,
                PRIMARY KEY (vendor, origin, cookie_name)
            );
            CREATE INDEX IF NOT EXISTS idx_antibot_cookie_cache_origin
                ON antibot_cookie_cache(origin);

            -- ===== Infra-fingerprinting schema (Fase A) =====
            -- Target-scoped recon tables. `target_root` is the registrable
            -- domain the operator asked us to investigate; every row is
            -- associated so a single DB can carry multiple concurrent
            -- recon targets side-by-side.

            CREATE TABLE IF NOT EXISTS domains (
                domain TEXT PRIMARY KEY,
                target_root TEXT NOT NULL,
                is_subdomain INTEGER NOT NULL DEFAULT 0,
                is_wildcard_dns INTEGER NOT NULL DEFAULT 0,
                server_fp_json TEXT,
                first_seen INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                last_probed INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_domains_target_root ON domains(target_root);

            CREATE TABLE IF NOT EXISTS dns_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                domain TEXT NOT NULL,
                record_type TEXT NOT NULL,
                rdata TEXT NOT NULL,
                ttl INTEGER,
                observed_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_dns_records_domain ON dns_records(domain);
            CREATE INDEX IF NOT EXISTS idx_dns_records_type ON dns_records(record_type);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_dns_records_uniq
                ON dns_records(domain, record_type, rdata);

            CREATE TABLE IF NOT EXISTS ip_addresses (
                ip TEXT PRIMARY KEY,
                asn INTEGER,
                asn_name TEXT,
                reverse_ptr TEXT,
                cloud_provider TEXT,
                cdn TEXT,
                country TEXT,
                first_seen INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                last_updated INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_ip_asn ON ip_addresses(asn);
            CREATE INDEX IF NOT EXISTS idx_ip_cdn ON ip_addresses(cdn);

            CREATE TABLE IF NOT EXISTS domain_ips (
                domain TEXT NOT NULL,
                ip TEXT NOT NULL,
                observed_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                PRIMARY KEY (domain, ip)
            );
            CREATE INDEX IF NOT EXISTS idx_domain_ips_ip ON domain_ips(ip);

            CREATE TABLE IF NOT EXISTS certs (
                sha256_fingerprint TEXT PRIMARY KEY,
                subject_cn TEXT,
                issuer_cn TEXT,
                issuer_org TEXT,
                serial_number TEXT,
                not_before INTEGER,
                not_after INTEGER,
                sans_json TEXT,
                is_wildcard INTEGER NOT NULL DEFAULT 0,
                is_self_signed INTEGER NOT NULL DEFAULT 0,
                sig_algo TEXT,
                pubkey_algo TEXT,
                pubkey_bits INTEGER,
                source TEXT NOT NULL, -- 'tls_handshake' | 'ct_log' | 'manual'
                first_seen INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_certs_issuer ON certs(issuer_cn);
            CREATE INDEX IF NOT EXISTS idx_certs_subject ON certs(subject_cn);

            CREATE TABLE IF NOT EXISTS cert_seen_on (
                cert_sha256 TEXT NOT NULL,
                domain TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 443,
                observed_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                PRIMARY KEY (cert_sha256, domain, port)
            );
            CREATE INDEX IF NOT EXISTS idx_cert_seen_domain ON cert_seen_on(domain);

            CREATE TABLE IF NOT EXISTS port_probes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ip TEXT NOT NULL,
                port INTEGER NOT NULL,
                state TEXT NOT NULL, -- 'open' | 'closed' | 'filtered' | 'open_filtered'
                banner TEXT,
                service TEXT,
                service_version TEXT,
                observed_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_port_probes_ip ON port_probes(ip);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_port_probes_uniq
                ON port_probes(ip, port, observed_at);

            CREATE TABLE IF NOT EXISTS whois_records (
                domain TEXT PRIMARY KEY,
                registrar TEXT,
                registrant_org TEXT,
                created_at INTEGER,
                expires_at INTEGER,
                updated_at INTEGER,
                nameservers_json TEXT,
                status_json TEXT,
                abuse_email TEXT,
                raw_json TEXT,
                observed_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );

            CREATE TABLE IF NOT EXISTS asset_refs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                from_page_url TEXT NOT NULL,
                to_url TEXT NOT NULL,
                to_domain TEXT NOT NULL,
                kind TEXT NOT NULL, -- 'script'|'style'|'font'|'image'|'video'|'audio'|'iframe'|'link'|'turnstile'|'xhr'|'websocket'|'other'
                is_internal INTEGER NOT NULL DEFAULT 0,
                first_seen INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_asset_refs_from ON asset_refs(from_page_url);
            CREATE INDEX IF NOT EXISTS idx_asset_refs_to_domain ON asset_refs(to_domain);
            CREATE INDEX IF NOT EXISTS idx_asset_refs_kind ON asset_refs(kind);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_asset_refs_uniq
                ON asset_refs(from_page_url, to_url, kind);

            CREATE TABLE IF NOT EXISTS external_domains (
                domain TEXT PRIMARY KEY,
                first_seen_from TEXT,
                ref_count INTEGER NOT NULL DEFAULT 1,
                categories_json TEXT,
                first_seen INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE TABLE IF NOT EXISTS tech_fingerprints (
                url TEXT PRIMARY KEY,
                final_url TEXT NOT NULL,
                host TEXT NOT NULL,
                report_json TEXT NOT NULL,
                generated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tech_fingerprints_host
                ON tech_fingerprints(host);

            -- Operator summary of a recon run, aggregated per target_root.
            CREATE VIEW IF NOT EXISTS v_target_intel AS
                SELECT
                    d.target_root,
                    COUNT(DISTINCT d.domain) AS domains,
                    SUM(d.is_subdomain) AS subdomains,
                    SUM(d.is_wildcard_dns) AS wildcard_dns,
                    (SELECT COUNT(DISTINCT ip) FROM domain_ips WHERE domain IN
                        (SELECT domain FROM domains WHERE target_root = d.target_root)) AS unique_ips,
                    (SELECT COUNT(*) FROM certs c INNER JOIN cert_seen_on cso
                        ON cso.cert_sha256 = c.sha256_fingerprint
                        WHERE cso.domain IN (SELECT domain FROM domains WHERE target_root = d.target_root)) AS certs_seen
                FROM domains d
                GROUP BY d.target_root;
            "#,
        )
        .map_err(|e| format!("schema: {e}"))?;
        for sql in [
            "ALTER TABLE pages ADD COLUMN body_sha256 TEXT",
            "ALTER TABLE pages ADD COLUMN html_sha256 TEXT",
            "ALTER TABLE pages ADD COLUMN body_blob_path TEXT",
            "ALTER TABLE pages ADD COLUMN html_blob_path TEXT",
            "ALTER TABLE pages ADD COLUMN body_size INTEGER",
            "ALTER TABLE pages ADD COLUMN html_size INTEGER",
            "ALTER TABLE pages ADD COLUMN body_mime TEXT",
            "ALTER TABLE pages ADD COLUMN html_mime TEXT",
            "ALTER TABLE pages ADD COLUMN body_truncated INTEGER NOT NULL DEFAULT 0",
        ] {
            let _ = conn.execute(sql, []);
        }
        Ok(conn)
    }

    fn run(mut conn: Connection, rx: &mut mpsc::Receiver<Op>) {
        const MAX_BATCH: usize = 256;
        const BATCH_MS: u64 = 25;
        let mut batch: Vec<Op> = Vec::with_capacity(MAX_BATCH);
        loop {
            // Block until at least one op.
            let Some(op) = rx.blocking_recv() else {
                break;
            };
            batch.push(op);
            let deadline = std::time::Instant::now() + Duration::from_millis(BATCH_MS);
            while batch.len() < MAX_BATCH {
                match rx.try_recv() {
                    Ok(op) => batch.push(op),
                    Err(mpsc::error::TryRecvError::Empty) => {
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
            let tx = match conn.transaction() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(?e, "sqlite begin failed");
                    batch.clear();
                    continue;
                }
            };
            for op in batch.drain(..) {
                if let Err(e) = apply_op(&tx, op) {
                    tracing::warn!(?e, "sqlite op failed");
                }
            }
            if let Err(e) = tx.commit() {
                tracing::warn!(?e, "sqlite commit failed");
            }
        }
    }

    async fn send(&self, op: Op) -> Result<()> {
        self.tx
            .send(op)
            .await
            .map_err(|_| Error::Storage("sqlite writer disconnected".into()))
    }

    /// Persist a batch of proxy score rows via the writer thread.
    pub async fn save_proxy_scores(&self, rows: Vec<ProxyScoreRow>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        self.send(Op::SaveProxyScores { rows }).await
    }

    /// Persist a batch of `(host, bundle_id, proxy_url)` affinity rows via
    /// the writer thread.
    pub async fn save_host_affinity(&self, entries: Vec<(String, i64, String)>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.send(Op::SaveHostAffinity { entries }).await
    }

    /// Snapshot every persisted proxy score row. Runs on a read-only
    /// connection so we don't block the writer.
    pub async fn load_proxy_scores(&self) -> Result<Vec<ProxyScoreRow>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<ProxyScoreRow>> {
            let conn = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| Error::Storage(format!("proxy_scores read open: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT url, success, timeouts, resets, status_4xx, status_5xx,
                            challenge_hits, latency_p50_ms, latency_p95_ms,
                            last_success_at, quarantine_until FROM proxy_scores",
                )
                .map_err(|e| Error::Storage(format!("proxy_scores prepare: {e}")))?;
            let iter = stmt
                .query_map([], |r| {
                    Ok(ProxyScoreRow {
                        url: r.get(0)?,
                        success: r.get(1)?,
                        timeouts: r.get(2)?,
                        resets: r.get(3)?,
                        status_4xx: r.get(4)?,
                        status_5xx: r.get(5)?,
                        challenge_hits: r.get(6)?,
                        latency_p50_ms: r.get(7)?,
                        latency_p95_ms: r.get(8)?,
                        last_success_at: r.get(9)?,
                        quarantine_until: r.get(10)?,
                    })
                })
                .map_err(|e| Error::Storage(format!("proxy_scores query: {e}")))?;
            let mut out = Vec::new();
            for r in iter.flatten() {
                out.push(r);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Storage(format!("proxy_scores join: {e}")))?
    }

    /// Archive a finished session row. Called from the Fase 6 cleanup
    /// task and from the run-ended flush. Idempotent — repeated calls
    /// for the same id upsert, so a TTL drop followed by a run-end
    /// flush cannot duplicate.
    pub async fn archive_session_row(&self, row: ArchivedSessionRow) -> Result<()> {
        self.send(Op::ArchiveSession {
            session_id: row.session_id,
            scope: row.scope,
            scope_key: row.scope_key,
            state: row.state,
            bundle_id: row.bundle_id,
            created_at: row.created_at,
            ended_at: row.ended_at,
            urls_visited: row.urls_visited,
            challenges: row.challenges,
            final_proxy: row.final_proxy,
            reason: row.reason,
        })
        .await
    }

    /// List archived sessions, optionally filtered by state.
    pub async fn list_archived_sessions(
        &self,
        state_filter: Option<String>,
    ) -> Result<Vec<ArchivedSessionRow>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<ArchivedSessionRow>> {
            let conn = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| Error::Storage(format!("sessions_archive open: {e}")))?;
            let mut sql = String::from(
                "SELECT id, scope, scope_key, state, bundle_id, created_at, ended_at,
                        urls_visited, challenges, final_proxy, reason
                 FROM sessions_archive",
            );
            if state_filter.is_some() {
                sql.push_str(" WHERE state = ?");
            }
            sql.push_str(" ORDER BY ended_at DESC");
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| Error::Storage(format!("sessions_archive prepare: {e}")))?;
            let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::new();
            if let Some(s) = &state_filter {
                params_vec.push(s);
            }
            let rows = stmt
                .query_map(params_vec.as_slice(), |r| {
                    Ok(ArchivedSessionRow {
                        session_id: r.get(0)?,
                        scope: r.get(1)?,
                        scope_key: r.get(2)?,
                        state: r.get(3)?,
                        bundle_id: r.get(4)?,
                        created_at: r.get(5)?,
                        ended_at: r.get(6)?,
                        urls_visited: r.get(7)?,
                        challenges: r.get(8)?,
                        final_proxy: r.get(9)?,
                        reason: r.get(10)?,
                    })
                })
                .map_err(|e| Error::Storage(format!("sessions_archive query: {e}")))?;
            let mut out = Vec::new();
            for r in rows.flatten() {
                out.push(r);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Storage(format!("sessions_archive join: {e}")))?
    }

    /// Snapshot every `(host, bundle_id, proxy_url)` affinity pin.
    pub async fn load_host_affinity(&self) -> Result<Vec<(String, i64, String)>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<(String, i64, String)>> {
            let conn = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| Error::Storage(format!("host_affinity read open: {e}")))?;
            let mut stmt = conn
                .prepare("SELECT host, bundle_id, proxy_url FROM host_affinity")
                .map_err(|e| Error::Storage(format!("host_affinity prepare: {e}")))?;
            let iter = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| Error::Storage(format!("host_affinity query: {e}")))?;
            let mut out = Vec::new();
            for r in iter.flatten() {
                out.push(r);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Storage(format!("host_affinity join: {e}")))?
    }
}

fn apply_op(tx: &rusqlite::Transaction<'_>, op: Op) -> rusqlite::Result<()> {
    match op {
        Op::SaveRaw {
            url,
            final_url,
            status,
            headers_json,
            body,
            body_size,
            sha256,
            blob_path,
            mime,
            body_truncated,
            kind,
        } => {
            if let Some(blob_path) = blob_path.as_ref() {
                tx.execute(
                    "INSERT INTO content_blobs (sha256, kind, mime, size, path)
                     VALUES (?,?,?,?,?)
                     ON CONFLICT(sha256) DO UPDATE SET
                        kind=excluded.kind,
                        mime=excluded.mime,
                        size=excluded.size,
                        path=excluded.path",
                    params![sha256, "raw", mime, body_size, blob_path],
                )?;
            }
            tx.execute(
                "INSERT INTO pages (url, final_url, status, bytes, rendered, sha256, body,
                        body_sha256, body_blob_path, body_size, body_mime, body_truncated,
                        headers_json, kind)
                 VALUES (?,?,?,?,0,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(url) DO UPDATE SET
                    final_url=excluded.final_url, status=excluded.status,
                    bytes=excluded.bytes, sha256=excluded.sha256, body=excluded.body,
                    body_sha256=excluded.body_sha256,
                    body_blob_path=excluded.body_blob_path,
                    body_size=excluded.body_size,
                    body_mime=excluded.body_mime,
                    body_truncated=excluded.body_truncated,
                    headers_json=excluded.headers_json, kind=excluded.kind,
                    saved_at=strftime('%s','now')",
                params![
                    url,
                    final_url,
                    status,
                    body_size,
                    sha256,
                    body,
                    sha256,
                    blob_path,
                    body_size,
                    mime,
                    body_truncated,
                    headers_json,
                    kind
                ],
            )?;
        }
        Op::SaveRendered {
            url,
            final_url,
            status,
            bytes,
            rendered,
            sha256,
            html,
            blob_size,
            blob_path,
            kind,
        } => {
            if let Some(blob_path) = blob_path.as_ref() {
                tx.execute(
                    "INSERT INTO content_blobs (sha256, kind, mime, size, path)
                     VALUES (?,?,?,?,?)
                     ON CONFLICT(sha256) DO UPDATE SET
                        kind=excluded.kind,
                        mime=excluded.mime,
                        size=excluded.size,
                        path=excluded.path",
                    params![sha256, "html", "text/html", blob_size, blob_path],
                )?;
            }
            tx.execute(
                "INSERT INTO pages (url, final_url, status, bytes, rendered, sha256, html,
                        html_sha256, html_blob_path, html_size, html_mime, kind)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(url) DO UPDATE SET
                    final_url=excluded.final_url, status=excluded.status, bytes=excluded.bytes,
                    rendered=excluded.rendered, sha256=excluded.sha256, html=excluded.html,
                    html_sha256=excluded.html_sha256,
                    html_blob_path=excluded.html_blob_path,
                    html_size=excluded.html_size,
                    html_mime=excluded.html_mime,
                    kind=excluded.kind, saved_at=strftime('%s','now')",
                params![
                    url,
                    final_url,
                    status,
                    bytes,
                    rendered,
                    sha256,
                    html,
                    sha256,
                    blob_path,
                    blob_size,
                    "text/html",
                    kind
                ],
            )?;
        }
        Op::SaveEdge { src, dst } => {
            tx.execute(
                "INSERT INTO edges (src, dst, weight) VALUES (?,?,1)
                 ON CONFLICT(src, dst) DO UPDATE SET weight=weight+1",
                params![src, dst],
            )?;
        }
        Op::SaveHostFacts {
            host,
            favicon_mmh3,
            dns_json,
            robots_present,
            manifest_present,
            service_worker_present,
            cert_sha256,
            cert_subject_cn,
            cert_issuer_cn,
            cert_not_before,
            cert_not_after,
            cert_sans_json,
            rdap_json,
            registrar,
            registrant_org,
            registration_created,
            registration_expires,
        } => {
            tx.execute(
                "INSERT INTO host_facts (host, favicon_mmh3, dns_json, robots_present,
                                         manifest_present, service_worker_present,
                                         cert_sha256, cert_subject_cn, cert_issuer_cn,
                                         cert_not_before, cert_not_after, cert_sans_json,
                                         rdap_json, registrar, registrant_org,
                                         registration_created, registration_expires)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(host) DO UPDATE SET
                    favicon_mmh3=COALESCE(excluded.favicon_mmh3, host_facts.favicon_mmh3),
                    dns_json=COALESCE(excluded.dns_json, host_facts.dns_json),
                    robots_present=COALESCE(excluded.robots_present, host_facts.robots_present),
                    manifest_present=COALESCE(excluded.manifest_present, host_facts.manifest_present),
                    service_worker_present=COALESCE(excluded.service_worker_present, host_facts.service_worker_present),
                    cert_sha256=COALESCE(excluded.cert_sha256, host_facts.cert_sha256),
                    cert_subject_cn=COALESCE(excluded.cert_subject_cn, host_facts.cert_subject_cn),
                    cert_issuer_cn=COALESCE(excluded.cert_issuer_cn, host_facts.cert_issuer_cn),
                    cert_not_before=COALESCE(excluded.cert_not_before, host_facts.cert_not_before),
                    cert_not_after=COALESCE(excluded.cert_not_after, host_facts.cert_not_after),
                    cert_sans_json=COALESCE(excluded.cert_sans_json, host_facts.cert_sans_json),
                    rdap_json=COALESCE(excluded.rdap_json, host_facts.rdap_json),
                    registrar=COALESCE(excluded.registrar, host_facts.registrar),
                    registrant_org=COALESCE(excluded.registrant_org, host_facts.registrant_org),
                    registration_created=COALESCE(excluded.registration_created, host_facts.registration_created),
                    registration_expires=COALESCE(excluded.registration_expires, host_facts.registration_expires),
                    updated_at=strftime('%s','now')",
                params![host, favicon_mmh3, dns_json, robots_present, manifest_present, service_worker_present,
                        cert_sha256, cert_subject_cn, cert_issuer_cn, cert_not_before, cert_not_after,
                        cert_sans_json, rdap_json, registrar, registrant_org,
                        registration_created, registration_expires],
            )?;
        }
        Op::SaveMetrics(m) => {
            tx.execute(
                "INSERT INTO page_metrics (
                    url, dns_ms, tcp_connect_ms, tls_handshake_ms, ttfb_ms,
                    download_ms, total_ms, status, bytes, alpn, tls_version, cipher,
                    dom_content_loaded_ms, load_event_ms, first_paint_ms,
                    first_contentful_paint_ms, largest_contentful_paint_ms,
                    cumulative_layout_shift, total_blocking_time_ms, longest_task_ms,
                    dom_nodes, js_heap_used_bytes, js_heap_total_bytes, resource_count,
                    total_transfer_bytes, total_decoded_bytes, transfer_by_type_json,
                    resources_json
                 ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(url) DO UPDATE SET
                    dns_ms=excluded.dns_ms, tcp_connect_ms=excluded.tcp_connect_ms,
                    tls_handshake_ms=excluded.tls_handshake_ms, ttfb_ms=excluded.ttfb_ms,
                    download_ms=excluded.download_ms, total_ms=excluded.total_ms,
                    status=excluded.status, bytes=excluded.bytes, alpn=excluded.alpn,
                    tls_version=excluded.tls_version, cipher=excluded.cipher,
                    dom_content_loaded_ms=excluded.dom_content_loaded_ms,
                    load_event_ms=excluded.load_event_ms,
                    first_paint_ms=excluded.first_paint_ms,
                    first_contentful_paint_ms=excluded.first_contentful_paint_ms,
                    largest_contentful_paint_ms=excluded.largest_contentful_paint_ms,
                    cumulative_layout_shift=excluded.cumulative_layout_shift,
                    total_blocking_time_ms=excluded.total_blocking_time_ms,
                    longest_task_ms=excluded.longest_task_ms,
                    dom_nodes=excluded.dom_nodes,
                    js_heap_used_bytes=excluded.js_heap_used_bytes,
                    js_heap_total_bytes=excluded.js_heap_total_bytes,
                    resource_count=excluded.resource_count,
                    total_transfer_bytes=excluded.total_transfer_bytes,
                    total_decoded_bytes=excluded.total_decoded_bytes,
                    transfer_by_type_json=excluded.transfer_by_type_json,
                    resources_json=excluded.resources_json,
                    updated_at=strftime('%s','now')",
                params![
                    m.url,
                    m.dns_ms,
                    m.tcp_connect_ms,
                    m.tls_handshake_ms,
                    m.ttfb_ms,
                    m.download_ms,
                    m.total_ms,
                    m.status,
                    m.bytes,
                    m.alpn,
                    m.tls_version,
                    m.cipher,
                    m.dom_content_loaded_ms,
                    m.load_event_ms,
                    m.first_paint_ms,
                    m.first_contentful_paint_ms,
                    m.largest_contentful_paint_ms,
                    m.cumulative_layout_shift,
                    m.total_blocking_time_ms,
                    m.longest_task_ms,
                    m.dom_nodes,
                    m.js_heap_used_bytes,
                    m.js_heap_total_bytes,
                    m.resource_count,
                    m.total_transfer_bytes,
                    m.total_decoded_bytes,
                    m.transfer_by_type_json,
                    m.resources_json,
                ],
            )?;
        }
        Op::SaveScreenshot {
            url,
            sha256,
            bytes,
            png,
        } => {
            tx.execute(
                "INSERT INTO screenshots (url, sha256, bytes, png)
                 VALUES (?,?,?,?)
                 ON CONFLICT(url) DO UPDATE SET
                    sha256=excluded.sha256,
                    bytes=excluded.bytes,
                    png=excluded.png,
                    saved_at=strftime('%s','now')",
                params![url, sha256, bytes, png],
            )?;
        }
        Op::SaveState {
            session_id,
            state_json,
        } => {
            tx.execute(
                "INSERT INTO sessions (session_id, state_json)
                 VALUES (?,?)
                 ON CONFLICT(session_id) DO UPDATE SET
                    state_json=excluded.state_json,
                    saved_at=strftime('%s','now')",
                params![session_id, state_json],
            )?;
        }
        Op::SaveProxyScores { rows } => {
            for r in rows {
                tx.execute(
                    "INSERT INTO proxy_scores (url, success, timeouts, resets,
                        status_4xx, status_5xx, challenge_hits,
                        latency_p50_ms, latency_p95_ms, last_success_at, quarantine_until)
                     VALUES (?,?,?,?,?,?,?,?,?,?,?)
                     ON CONFLICT(url) DO UPDATE SET
                        success=excluded.success,
                        timeouts=excluded.timeouts,
                        resets=excluded.resets,
                        status_4xx=excluded.status_4xx,
                        status_5xx=excluded.status_5xx,
                        challenge_hits=excluded.challenge_hits,
                        latency_p50_ms=excluded.latency_p50_ms,
                        latency_p95_ms=excluded.latency_p95_ms,
                        last_success_at=excluded.last_success_at,
                        quarantine_until=excluded.quarantine_until,
                        updated_at=strftime('%s','now')",
                    params![
                        r.url,
                        r.success,
                        r.timeouts,
                        r.resets,
                        r.status_4xx,
                        r.status_5xx,
                        r.challenge_hits,
                        r.latency_p50_ms,
                        r.latency_p95_ms,
                        r.last_success_at,
                        r.quarantine_until,
                    ],
                )?;
            }
        }
        Op::SaveArtifact {
            url,
            final_url,
            session_id,
            kind,
            name,
            step_id,
            step_kind,
            selector,
            mime,
            size,
            sha256,
            bytes,
            created_at,
        } => {
            tx.execute(
                "INSERT INTO artifacts (url, final_url, session_id, kind, name, step_id,
                        step_kind, selector, mime, size, sha256, bytes, created_at)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    url, final_url, session_id, kind, name, step_id, step_kind, selector, mime,
                    size, sha256, bytes, created_at
                ],
            )?;
        }
        Op::RecordChallenge {
            session_id,
            vendor,
            level,
            url,
            origin,
            proxy,
            observed_at,
            metadata,
        } => {
            tx.execute(
                "INSERT INTO challenge_events
                    (session_id, vendor, level, url, origin, proxy, observed_at, metadata)
                 VALUES (?,?,?,?,?,?,?,?)",
                params![
                    session_id,
                    vendor,
                    level,
                    url,
                    origin,
                    proxy,
                    observed_at,
                    metadata
                ],
            )?;
        }
        Op::SaveAssetRefs { refs } => {
            for r in refs {
                // Empty to_domain means the URL had no host (shouldn't
                // happen because `extract_asset_refs` filters those out,
                // but a caller could bypass the helper — skip defensively
                // so the UNIQUE index doesn't choke on a blank key).
                if r.to_domain.is_empty() {
                    continue;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO asset_refs
                        (from_page_url, to_url, to_domain, kind, is_internal)
                     VALUES (?,?,?,?,?)",
                    params![
                        r.from_page_url,
                        r.to_url,
                        r.to_domain,
                        r.kind,
                        r.is_internal as i64,
                    ],
                )?;
                if !r.is_internal {
                    // External domain bookkeeping: first insert seeds
                    // the row (with categories computed via the heuristic
                    // table in `discovery::asset_refs::categorise`);
                    // subsequent sightings only bump ref_count so we
                    // don't churn the categories column on every visit.
                    let cats = crate::discovery::asset_refs::categorise(&r.to_domain);
                    let cats_json: Option<String> = if cats.is_empty() {
                        // Keep the column NULL when nothing matched —
                        // `[]` would be a lie ("we classified it as
                        // nothing") whereas NULL means "no opinion".
                        None
                    } else {
                        let slugs: Vec<&str> = cats.iter().map(|c| c.as_str()).collect();
                        serde_json::to_string(&slugs).ok()
                    };
                    tx.execute(
                        "INSERT INTO external_domains
                            (domain, first_seen_from, ref_count, categories_json)
                         VALUES (?1, ?2, 1, ?3)
                         ON CONFLICT(domain) DO UPDATE SET
                             ref_count = ref_count + 1,
                             -- backfill categories on existing rows that
                             -- predate the heuristic table; never clobber
                             -- a non-null value with NULL.
                             categories_json = COALESCE(external_domains.categories_json, excluded.categories_json)",
                        params![r.to_domain, r.from_page_url, cats_json],
                    )?;
                }
            }
        }
        Op::SaveTechFingerprint {
            url,
            final_url,
            host,
            report_json,
            generated_at,
        } => {
            tx.execute(
                "INSERT INTO tech_fingerprints
                    (url, final_url, host, report_json, generated_at)
                 VALUES (?,?,?,?,?)
                 ON CONFLICT(url) DO UPDATE SET
                    final_url=excluded.final_url,
                    host=excluded.host,
                    report_json=excluded.report_json,
                    generated_at=excluded.generated_at",
                params![url, final_url, host, report_json, generated_at],
            )?;
            let target_root = crate::discovery::subdomains::registrable_domain(&host)
                .unwrap_or_else(|| host.clone());
            let existing_rollup = tx
                .query_row(
                    "SELECT server_fp_json FROM domains WHERE domain=?1",
                    params![host],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten();
            let rollup_json = merge_tech_rollup(existing_rollup, &report_json);
            tx.execute(
                "INSERT INTO domains (domain, target_root, is_subdomain, server_fp_json, last_probed)
                 VALUES (?1, ?2, CASE WHEN ?1 <> ?2 THEN 1 ELSE 0 END, ?3, ?4)
                 ON CONFLICT(domain) DO UPDATE SET
                    server_fp_json=excluded.server_fp_json,
                    last_probed=excluded.last_probed",
                params![host, target_root, rollup_json, generated_at],
            )?;
        }
        Op::RecordTelemetry {
            session_id,
            vendor,
            endpoint,
            method,
            payload_size,
            payload_shape,
            pattern_label,
            observed_at,
        } => {
            tx.execute(
                "INSERT INTO vendor_telemetry
                    (session_id, vendor, endpoint, method, payload_size,
                     payload_shape, pattern_label, observed_at)
                 VALUES (?,?,?,?,?,?,?,?)",
                params![
                    session_id,
                    vendor,
                    endpoint,
                    method,
                    payload_size,
                    payload_shape,
                    pattern_label,
                    observed_at,
                ],
            )?;
        }
        Op::ArchiveSession {
            session_id,
            scope,
            scope_key,
            state,
            bundle_id,
            created_at,
            ended_at,
            urls_visited,
            challenges,
            final_proxy,
            reason,
        } => {
            tx.execute(
                "INSERT INTO sessions_archive (id, scope, scope_key, state, bundle_id,
                        created_at, ended_at, urls_visited, challenges, final_proxy, reason)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(id) DO UPDATE SET
                    scope=excluded.scope,
                    scope_key=excluded.scope_key,
                    state=excluded.state,
                    bundle_id=excluded.bundle_id,
                    ended_at=excluded.ended_at,
                    urls_visited=excluded.urls_visited,
                    challenges=excluded.challenges,
                    final_proxy=excluded.final_proxy,
                    reason=excluded.reason",
                params![
                    session_id,
                    scope,
                    scope_key,
                    state,
                    bundle_id,
                    created_at,
                    ended_at,
                    urls_visited,
                    challenges,
                    final_proxy,
                    reason,
                ],
            )?;
        }
        Op::SaveHostAffinity { entries } => {
            for (host, bundle_id, proxy_url) in entries {
                tx.execute(
                    "INSERT INTO host_affinity (host, bundle_id, proxy_url)
                     VALUES (?,?,?)
                     ON CONFLICT(host, bundle_id) DO UPDATE SET
                        proxy_url=excluded.proxy_url,
                        updated_at=strftime('%s','now')",
                    params![host, bundle_id, proxy_url],
                )?;
            }
        }
    }
    Ok(())
}

fn merge_tech_rollup(existing_json: Option<String>, current_json: &str) -> String {
    use crate::discovery::tech_fingerprint::{TechFingerprintReport, TechMatch};

    let Ok(current) = serde_json::from_str::<TechFingerprintReport>(current_json) else {
        return current_json.to_string();
    };
    let current_url = current.url.clone();
    let current_final_url = current.final_url.clone();
    let current_host = current.host.clone();
    let current_generated_at = current.generated_at;
    let mut rollup = existing_json
        .and_then(|json| serde_json::from_str::<TechFingerprintReport>(&json).ok())
        .unwrap_or_else(|| TechFingerprintReport {
            url: current_url,
            final_url: current_final_url.clone(),
            host: current_host.clone(),
            technologies: Vec::new(),
            generated_at: current_generated_at,
        });

    let mut by_slug: BTreeMap<String, TechMatch> = BTreeMap::new();
    for tech in rollup.technologies.drain(..) {
        by_slug.insert(tech.slug.clone(), tech);
    }
    for tech in current.technologies {
        by_slug
            .entry(tech.slug.clone())
            .and_modify(|existing| merge_tech_match(existing, &tech))
            .or_insert(tech);
    }

    let mut technologies: Vec<TechMatch> = by_slug.into_values().collect();
    technologies.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| a.slug.cmp(&b.slug))
    });
    rollup.host = current_host;
    rollup.final_url = current_final_url;
    rollup.generated_at = rollup.generated_at.max(current_generated_at);
    rollup.technologies = technologies;
    serde_json::to_string(&rollup).unwrap_or_else(|_| current_json.to_string())
}

fn merge_tech_match(
    existing: &mut crate::discovery::tech_fingerprint::TechMatch,
    incoming: &crate::discovery::tech_fingerprint::TechMatch,
) {
    if incoming.confidence > existing.confidence {
        existing.name = incoming.name.clone();
        existing.category = incoming.category.clone();
        existing.confidence = incoming.confidence;
    }
    for evidence in &incoming.evidence {
        if !existing
            .evidence
            .iter()
            .any(|e| same_tech_evidence(e, evidence))
        {
            existing.evidence.push(evidence.clone());
        }
    }
    existing
        .evidence
        .sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.value.cmp(&b.value)));
}

fn same_tech_evidence(
    a: &crate::discovery::tech_fingerprint::TechEvidence,
    b: &crate::discovery::tech_fingerprint::TechEvidence,
) -> bool {
    a.source == b.source && a.key == b.key && a.value == b.value
}

fn headers_to_json(h: &HeaderMap) -> String {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (k, v) in h.iter() {
        if let Ok(s) = v.to_str() {
            pairs.push((k.as_str().to_string(), s.to_string()));
        }
    }
    serde_json::to_string(&pairs).unwrap_or_else(|_| "[]".into())
}

async fn write_blob(
    root: PathBuf,
    kind: &'static str,
    sha256: String,
    bytes: Vec<u8>,
) -> Result<String> {
    tokio::task::spawn_blocking(move || -> Result<String> {
        let shard = &sha256[..2.min(sha256.len())];
        let rel = PathBuf::from(kind).join(shard).join(&sha256);
        let path = root.join(&rel);
        if path.exists() {
            return Ok(rel.to_string_lossy().to_string());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Storage(format!("blob mkdir: {e}")))?;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
        std::fs::write(&tmp, &bytes).map_err(|e| Error::Storage(format!("blob write: {e}")))?;
        if path.exists() {
            let _ = std::fs::remove_file(&tmp);
            return Ok(rel.to_string_lossy().to_string());
        }
        match std::fs::rename(&tmp, &path) {
            Ok(_) => {}
            Err(e) if path.exists() => {
                let _ = std::fs::remove_file(&tmp);
                tracing::debug!(?e, "blob rename lost race; existing blob kept");
            }
            Err(e) => return Err(Error::Storage(format!("blob rename: {e}"))),
        }
        Ok(rel.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| Error::Storage(format!("blob join: {e}")))?
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn save_raw(&self, url: &Url, headers: &HeaderMap, body: &Bytes) -> Result<()> {
        self.save_raw_response(url, url, 0, headers, body, false)
            .await
    }

    async fn save_raw_response(
        &self,
        url: &Url,
        final_url: &Url,
        status: u16,
        headers: &HeaderMap,
        body: &Bytes,
        truncated: bool,
    ) -> Result<()> {
        let hash = hex::encode(Sha256::digest(body));
        let body_vec = body.to_vec();
        let body_size = body_vec.len() as i64;
        let blob_path = if self.content_store_enabled {
            Some(
                write_blob(
                    self.blob_root.clone(),
                    "raw",
                    hash.clone(),
                    body_vec.clone(),
                )
                .await?,
            )
        } else {
            None
        };
        let body = if self.inline_legacy_columns {
            Some(body_vec)
        } else {
            None
        };
        let hdrs = headers_to_json(headers);
        let ct = headers.get("content-type").and_then(|v| v.to_str().ok());
        let mime = ct.unwrap_or("application/octet-stream").to_string();
        let kind = crate::discovery::classify_with_mime(url, ct)
            .as_str()
            .to_string();
        self.send(Op::SaveRaw {
            url: url.to_string(),
            final_url: final_url.to_string(),
            status: status as i64,
            headers_json: hdrs,
            body,
            body_size,
            sha256: hash,
            blob_path,
            mime,
            body_truncated: if truncated { 1 } else { 0 },
            kind,
        })
        .await
    }

    async fn save_rendered(
        &self,
        url: &Url,
        html_post_js: &str,
        meta: &PageMetadata,
    ) -> Result<()> {
        let hash = hex::encode(Sha256::digest(html_post_js.as_bytes()));
        let html_bytes = html_post_js.as_bytes().to_vec();
        let blob_size = html_bytes.len() as i64;
        let blob_path = if self.content_store_enabled {
            Some(write_blob(self.blob_root.clone(), "html", hash.clone(), html_bytes).await?)
        } else {
            None
        };
        let html = if self.inline_legacy_columns {
            Some(html_post_js.to_string())
        } else {
            None
        };
        self.send(Op::SaveRendered {
            url: url.to_string(),
            final_url: meta.final_url.to_string(),
            status: meta.status as i64,
            bytes: meta.bytes as i64,
            rendered: if meta.rendered { 1 } else { 0 },
            sha256: hash,
            html,
            blob_size,
            blob_path,
            kind: meta.kind.as_str().to_string(),
        })
        .await
    }

    async fn save_edge(&self, from: &Url, to: &Url) -> Result<()> {
        self.send(Op::SaveEdge {
            src: from.to_string(),
            dst: to.to_string(),
        })
        .await
    }

    async fn save_host_facts(&self, host: &str, f: &HostFacts) -> Result<()> {
        self.send(Op::SaveHostFacts {
            host: host.to_string(),
            favicon_mmh3: f.favicon_mmh3,
            dns_json: f.dns_json.clone(),
            robots_present: f.robots_present.map(|v| if v { 1 } else { 0 }),
            manifest_present: f.manifest_present.map(|v| if v { 1 } else { 0 }),
            service_worker_present: f.service_worker_present.map(|v| if v { 1 } else { 0 }),
            cert_sha256: f.cert_sha256.clone(),
            cert_subject_cn: f.cert_subject_cn.clone(),
            cert_issuer_cn: f.cert_issuer_cn.clone(),
            cert_not_before: f.cert_not_before.clone(),
            cert_not_after: f.cert_not_after.clone(),
            cert_sans_json: f.cert_sans_json.clone(),
            rdap_json: f.rdap_json.clone(),
            registrar: f.registrar.clone(),
            registrant_org: f.registrant_org.clone(),
            registration_created: f.registration_created.clone(),
            registration_expires: f.registration_expires.clone(),
        })
        .await
    }

    async fn save_metrics(&self, url: &Url, m: &crate::metrics::PageMetrics) -> Result<()> {
        let by_type_json = m
            .vitals
            .transfer_by_type
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let resources_json = if m.resources.is_empty() {
            None
        } else {
            serde_json::to_string(&m.resources).ok()
        };
        self.send(Op::SaveMetrics(Box::new(MetricsRow {
            url: url.to_string(),
            dns_ms: m.net.dns_ms.map(|v| v as i64),
            tcp_connect_ms: m.net.tcp_connect_ms.map(|v| v as i64),
            tls_handshake_ms: m.net.tls_handshake_ms.map(|v| v as i64),
            ttfb_ms: m.net.ttfb_ms.map(|v| v as i64),
            download_ms: m.net.download_ms.map(|v| v as i64),
            total_ms: m.net.total_ms.map(|v| v as i64),
            status: m.net.status.map(|v| v as i64),
            bytes: m.net.bytes.map(|v| v as i64),
            alpn: m.net.alpn.clone(),
            tls_version: m.net.tls_version.clone(),
            cipher: m.net.cipher.clone(),
            dom_content_loaded_ms: m.vitals.dom_content_loaded_ms,
            load_event_ms: m.vitals.load_event_ms,
            first_paint_ms: m.vitals.first_paint_ms,
            first_contentful_paint_ms: m.vitals.first_contentful_paint_ms,
            largest_contentful_paint_ms: m.vitals.largest_contentful_paint_ms,
            cumulative_layout_shift: m.vitals.cumulative_layout_shift,
            total_blocking_time_ms: m.vitals.total_blocking_time_ms,
            longest_task_ms: m.vitals.longest_task_ms,
            dom_nodes: m.vitals.dom_nodes.map(|v| v as i64),
            js_heap_used_bytes: m.vitals.js_heap_used_bytes.map(|v| v as i64),
            js_heap_total_bytes: m.vitals.js_heap_total_bytes.map(|v| v as i64),
            resource_count: m.vitals.resource_count.map(|v| v as i64),
            total_transfer_bytes: m.vitals.total_transfer_bytes.map(|v| v as i64),
            total_decoded_bytes: m.vitals.total_decoded_bytes.map(|v| v as i64),
            transfer_by_type_json: by_type_json,
            resources_json,
        })))
        .await
    }

    async fn save_screenshot(&self, url: &Url, png: &[u8]) -> Result<()> {
        let hash = hex::encode(Sha256::digest(png));
        // Legacy per-URL screenshots table: one-row-per-url keyed output
        // some consumers still scrape. Keep populated.
        self.send(Op::SaveScreenshot {
            url: url.to_string(),
            sha256: hash.clone(),
            bytes: png.len() as i64,
            png: png.to_vec(),
        })
        .await?;
        // New unified artifacts table — make `save_screenshot` a wrapper
        // over `save_artifact` so old callers automatically land in both
        // places with zero churn at the call site.
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

    async fn save_artifact(&self, meta: &ArtifactMeta<'_>, bytes: &[u8]) -> Result<()> {
        let hash = hex::encode(Sha256::digest(bytes));
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mime = meta.mime.unwrap_or(meta.kind.mime()).to_string();
        self.send(Op::SaveArtifact {
            url: meta.url.to_string(),
            final_url: meta.final_url.map(|u| u.to_string()),
            session_id: meta.session_id.to_string(),
            kind: meta.kind.wire_str().to_string(),
            name: meta.name.map(|s| s.to_string()),
            step_id: meta.step_id.map(|s| s.to_string()),
            step_kind: meta.step_kind.map(|s| s.to_string()),
            selector: meta.selector.map(|s| s.to_string()),
            mime,
            size: bytes.len() as i64,
            sha256: hash,
            bytes: bytes.to_vec(),
            created_at,
        })
        .await
    }

    async fn list_artifacts(
        &self,
        session_id: Option<&str>,
        kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRow>> {
        let path = self.path.clone();
        let sid = session_id.map(|s| s.to_string());
        let kind_str = kind.map(|k| k.wire_str().to_string());
        tokio::task::spawn_blocking(move || -> Result<Vec<ArtifactRow>> {
            let conn = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| Error::Storage(format!("artifacts read open: {e}")))?;
            // Build the WHERE clause with up to two dynamic filters; keep
            // parameter binding positional so we don't risk SQL injection.
            let mut sql = String::from(
                "SELECT id, url, final_url, session_id, kind, name, step_id, step_kind,
                        selector, mime, size, sha256, created_at
                 FROM artifacts",
            );
            let mut clauses: Vec<&str> = Vec::new();
            if sid.is_some() {
                clauses.push("session_id = ?");
            }
            if kind_str.is_some() {
                clauses.push("kind = ?");
            }
            if !clauses.is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(&clauses.join(" AND "));
            }
            sql.push_str(" ORDER BY id ASC");
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| Error::Storage(format!("artifacts prepare: {e}")))?;
            let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::new();
            if let Some(s) = &sid {
                params_vec.push(s);
            }
            if let Some(k) = &kind_str {
                params_vec.push(k);
            }
            let rows = stmt
                .query_map(params_vec.as_slice(), |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, i64>(10)?,
                        r.get::<_, String>(11)?,
                        r.get::<_, i64>(12)?,
                    ))
                })
                .map_err(|e| Error::Storage(format!("artifacts query: {e}")))?;
            let mut out = Vec::new();
            for row in rows.flatten() {
                let (
                    id,
                    url_s,
                    final_url_s,
                    session_id,
                    kind_s,
                    name,
                    step_id,
                    step_kind,
                    selector,
                    mime,
                    size,
                    sha256,
                    created_at,
                ) = row;
                let Ok(url) = url::Url::parse(&url_s) else {
                    continue;
                };
                let final_url = final_url_s.and_then(|s| url::Url::parse(&s).ok());
                let Some(k) = ArtifactKind::from_wire(&kind_s) else {
                    continue;
                };
                let ts = std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(created_at.max(0) as u64);
                out.push(ArtifactRow {
                    id,
                    url,
                    final_url,
                    session_id,
                    kind: k,
                    name,
                    step_id,
                    step_kind,
                    selector,
                    mime,
                    sha256,
                    size: size.max(0) as u64,
                    created_at: ts,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Storage(format!("artifacts join: {e}")))?
    }

    async fn save_state(&self, session_id: &str, state_json: &str) -> Result<()> {
        self.send(Op::SaveState {
            session_id: session_id.to_string(),
            state_json: state_json.to_string(),
        })
        .await
    }

    fn as_any_ref(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    async fn record_challenge(&self, signal: &crate::antibot::ChallengeSignal) -> Result<()> {
        let observed_at = signal
            .first_seen
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let metadata = if signal.metadata.is_null() {
            None
        } else {
            serde_json::to_string(&signal.metadata).ok()
        };
        self.send(Op::RecordChallenge {
            session_id: signal.session_id.clone(),
            vendor: signal.vendor.as_str().to_string(),
            level: signal.level.as_str().to_string(),
            url: signal.url.to_string(),
            origin: signal.origin.clone(),
            proxy: signal.proxy.as_ref().map(|p| p.to_string()),
            observed_at,
            metadata,
        })
        .await
    }

    async fn save_asset_refs(&self, refs: &[crate::discovery::asset_refs::AssetRef]) -> Result<()> {
        if refs.is_empty() {
            return Ok(());
        }
        let rows: Vec<AssetRefRow> = refs
            .iter()
            .map(|r| AssetRefRow {
                from_page_url: r.from_page_url.clone(),
                to_url: r.to_url.clone(),
                to_domain: r.to_domain.clone(),
                kind: r.kind.as_str().to_string(),
                is_internal: r.is_internal,
            })
            .collect();
        self.send(Op::SaveAssetRefs { refs: rows }).await
    }

    async fn save_tech_fingerprint(
        &self,
        report: &crate::discovery::tech_fingerprint::TechFingerprintReport,
    ) -> Result<()> {
        if report.host.is_empty() {
            return Ok(());
        }
        let report_json = serde_json::to_string(report)
            .map_err(|e| Error::Storage(format!("tech fingerprint json: {e}")))?;
        self.send(Op::SaveTechFingerprint {
            url: report.url.clone(),
            final_url: report.final_url.clone(),
            host: report.host.clone(),
            report_json,
            generated_at: report.generated_at,
        })
        .await
    }

    async fn record_telemetry(
        &self,
        telem: &crate::antibot::telemetry::VendorTelemetry,
    ) -> Result<()> {
        let observed_at = telem
            .observed_at
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let shape_json = serde_json::to_string(&telem.payload_shape)
            .unwrap_or_else(|_| "\"unknown\"".to_string());
        self.send(Op::RecordTelemetry {
            session_id: telem.session_id.clone(),
            vendor: telem.vendor.as_str().to_string(),
            endpoint: telem.endpoint.to_string(),
            method: telem.method.clone(),
            payload_size: telem.payload_size as i64,
            payload_shape: shape_json,
            pattern_label: telem.pattern_label.to_string(),
            observed_at,
        })
        .await
    }

    async fn session_challenges(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::antibot::ChallengeSignal>> {
        let path = self.path.clone();
        let sid = session_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<crate::antibot::ChallengeSignal>> {
            let conn = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| Error::Storage(format!("challenge read open: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT session_id, vendor, level, url, origin, proxy, observed_at, metadata
                     FROM challenge_events WHERE session_id=?1 ORDER BY observed_at ASC",
                )
                .map_err(|e| Error::Storage(format!("challenge prepare: {e}")))?;
            let rows = stmt
                .query_map(params![sid], |r| {
                    let session_id: String = r.get(0)?;
                    let vendor: String = r.get(1)?;
                    let level: String = r.get(2)?;
                    let url: String = r.get(3)?;
                    let origin: String = r.get(4)?;
                    let proxy: Option<String> = r.get(5)?;
                    let observed_at: i64 = r.get(6)?;
                    let metadata: Option<String> = r.get(7)?;
                    Ok((
                        session_id,
                        vendor,
                        level,
                        url,
                        origin,
                        proxy,
                        observed_at,
                        metadata,
                    ))
                })
                .map_err(|e| Error::Storage(format!("challenge query: {e}")))?;
            let mut out = Vec::new();
            for row in rows.flatten() {
                let (session_id, vendor, level, url, origin, proxy, observed_at, metadata) = row;
                let vendor = match vendor.as_str() {
                    "cloudflare_js_challenge" => {
                        crate::antibot::ChallengeVendor::CloudflareJsChallenge
                    }
                    "cloudflare_turnstile" => crate::antibot::ChallengeVendor::CloudflareTurnstile,
                    "recaptcha" => crate::antibot::ChallengeVendor::Recaptcha,
                    "recaptcha_enterprise" => crate::antibot::ChallengeVendor::RecaptchaEnterprise,
                    "hcaptcha" => crate::antibot::ChallengeVendor::HCaptcha,
                    "datadome" => crate::antibot::ChallengeVendor::DataDome,
                    "perimeterx" => crate::antibot::ChallengeVendor::PerimeterX,
                    "akamai" => crate::antibot::ChallengeVendor::Akamai,
                    "generic_captcha" => crate::antibot::ChallengeVendor::GenericCaptcha,
                    _ => crate::antibot::ChallengeVendor::AccessDenied,
                };
                let level = match level.as_str() {
                    "suspected" => crate::antibot::ChallengeLevel::Suspected,
                    "challenge_page" => crate::antibot::ChallengeLevel::ChallengePage,
                    "widget_present" => crate::antibot::ChallengeLevel::WidgetPresent,
                    _ => crate::antibot::ChallengeLevel::HardBlock,
                };
                let Ok(url_parsed) = url::Url::parse(&url) else {
                    continue;
                };
                let proxy_parsed = proxy.and_then(|p| url::Url::parse(&p).ok());
                let metadata_json = metadata
                    .and_then(|m| serde_json::from_str(&m).ok())
                    .unwrap_or(serde_json::Value::Null);
                let first_seen = std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs(observed_at.max(0) as u64);
                out.push(crate::antibot::ChallengeSignal {
                    vendor,
                    level,
                    url: url_parsed,
                    origin,
                    proxy: proxy_parsed,
                    session_id,
                    first_seen,
                    metadata: metadata_json,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Storage(format!("challenge join: {e}")))?
    }

    async fn archive_session(
        &self,
        entry: &crate::identity::SessionEntry,
        reason: crate::identity::EvictionReason,
    ) -> Result<()> {
        let scope = serde_json::to_string(&entry.scope)
            .unwrap_or_else(|_| "\"unknown\"".to_string())
            .trim_matches('"')
            .to_string();
        let final_proxy = entry.proxy_history.last().map(|u| u.to_string());
        let row = ArchivedSessionRow {
            session_id: entry.id.clone(),
            scope,
            scope_key: entry.scope_key.clone(),
            state: entry.state.as_str().to_string(),
            bundle_id: entry.bundle_id.map(|v| v as i64),
            created_at: entry.created_unix,
            ended_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            urls_visited: entry.urls_visited as i64,
            challenges: entry.challenges_seen as i64,
            final_proxy,
            reason: reason.as_str().to_string(),
        };
        self.archive_session_row(row).await
    }

    async fn load_state(&self, session_id: &str) -> Result<Option<String>> {
        // Bypass the writer thread — reads are safe concurrent and we'd
        // rather not pay batching latency on resume. Opening a fresh
        // read-only Connection is cheap on SQLite (WAL mode allows
        // readers during writes).
        let path = self.path.clone();
        let sid = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| Error::Storage(format!("state read open: {e}")))?;
            let out: Option<String> = conn
                .query_row(
                    "SELECT state_json FROM sessions WHERE session_id=?1",
                    params![sid],
                    |r| r.get::<_, String>(0),
                )
                .ok();
            Ok(out)
        })
        .await
        .map_err(|e| Error::Storage(format!("state read join: {e}")))?
    }
}

#[cfg(test)]
mod challenge_rate_view_tests {
    //! Drive the `v_challenge_rate_*` views against a real SQLite file.
    //! We go through `SqliteStorage::record_challenge` so the test also
    //! covers the write path; reads open a fresh read-only connection to
    //! match the CLI pattern.
    use super::*;
    use crate::antibot::{ChallengeLevel, ChallengeSignal, ChallengeVendor};
    use crate::storage::Storage;
    use std::time::{Duration, SystemTime};

    fn mk_signal(
        vendor: ChallengeVendor,
        session: &str,
        proxy: Option<&str>,
        age: Duration,
    ) -> ChallengeSignal {
        let url = url::Url::parse("https://target.example/page").unwrap();
        ChallengeSignal {
            vendor,
            level: ChallengeLevel::ChallengePage,
            url,
            origin: "https://target.example".to_string(),
            proxy: proxy.map(|p| url::Url::parse(p).unwrap()),
            session_id: session.to_string(),
            // SystemTime::now() - age so we can forge 24h+ old rows without
            // sleeping or messing with the clock.
            first_seen: SystemTime::now().checked_sub(age).unwrap(),
            metadata: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn views_aggregate_by_vendor_proxy_and_session() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let storage = SqliteStorage::open(&path).unwrap();

        // Synthetic mix: 2 cloudflare recent, 1 cloudflare old (>24h),
        // 1 akamai recent. Two sessions, one proxy + direct.
        let rows = vec![
            mk_signal(
                ChallengeVendor::CloudflareJsChallenge,
                "s1",
                Some("http://px.test:8080"),
                Duration::from_secs(60),
            ),
            mk_signal(
                ChallengeVendor::CloudflareJsChallenge,
                "s1",
                Some("http://px.test:8080"),
                Duration::from_secs(120),
            ),
            mk_signal(
                ChallengeVendor::CloudflareJsChallenge,
                "s2",
                None,
                Duration::from_secs(86400 * 3),
            ),
            mk_signal(ChallengeVendor::Akamai, "s2", None, Duration::from_secs(30)),
        ];
        for r in &rows {
            storage.record_challenge(r).await.unwrap();
        }

        // Sync with the writer thread before reading: the `Op::*` channel
        // is fire-and-forget, so we poll the `sessions` table (via
        // `load_state`) for a sentinel we wrote last. When it appears, the
        // mpsc has drained at least up to RecordChallenge + save_state.
        storage.save_state("__sync__", "{}").await.unwrap();
        for _ in 0..200 {
            if storage.load_state("__sync__").await.unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();

        // By vendor: cloudflare total=3 / last_24h=2, akamai total=1 / 1.
        let mut stmt = conn
            .prepare(
                "SELECT vendor, total, last_24h FROM v_challenge_rate_by_vendor ORDER BY vendor",
            )
            .unwrap();
        let got: Vec<(String, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(got.len(), 2, "two vendors expected, got {got:?}");
        let cf = got
            .iter()
            .find(|(v, _, _)| v == "cloudflare_js_challenge")
            .unwrap();
        assert_eq!(cf.1, 3, "cloudflare total");
        assert_eq!(cf.2, 2, "cloudflare last_24h");
        let ak = got.iter().find(|(v, _, _)| v == "akamai").unwrap();
        assert_eq!(ak.1, 1);
        assert_eq!(ak.2, 1);

        // By proxy: 'direct' for NULL rows, the URL origin for the proxied ones.
        let mut stmt = conn
            .prepare("SELECT proxy, total, last_24h FROM v_challenge_rate_by_proxy ORDER BY proxy")
            .unwrap();
        let got: Vec<(String, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let direct = got.iter().find(|(k, _, _)| k == "direct").unwrap();
        assert_eq!(direct.1, 2, "direct total (both NULL-proxy rows)");
        assert_eq!(direct.2, 1, "direct last_24h (one is >24h old)");
        let origin = got
            .iter()
            .find(|(k, _, _)| k.starts_with("http://px.test"))
            .unwrap();
        assert_eq!(origin.1, 2);
        assert_eq!(origin.2, 2);

        // By session: s1 total=2, s2 total=2 (one old, one recent).
        let mut stmt = conn
            .prepare("SELECT session_id, total, last_24h FROM v_challenge_rate_by_session ORDER BY session_id")
            .unwrap();
        let got: Vec<(String, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(got.len(), 2, "two sessions, got {got:?}");
        let s1 = got.iter().find(|(k, _, _)| k == "s1").unwrap();
        assert_eq!(s1.1, 2);
        assert_eq!(s1.2, 2);
        let s2 = got.iter().find(|(k, _, _)| k == "s2").unwrap();
        assert_eq!(s2.1, 2);
        assert_eq!(s2.2, 1);
    }
}
