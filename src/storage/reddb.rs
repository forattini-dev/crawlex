use base64::Engine as _;
use bytes::Bytes;
use http::HeaderMap;
use reddb_client::types::{JsonValue, ValueOut};
use reddb_client::Reddb;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

use crate::storage::{
    ArtifactKind, ArtifactMeta, ArtifactRow, ArtifactStorage, ChallengeStorage, IntelStorage,
    PageCacheMetadata, PageMetadata, StateStorage, Storage, TelemetryStorage,
};
use crate::{Error, Result};

const RAW_COLLECTION: &str = "crawlex_raw_responses";
const RAW_TABLE: &str = "crawlex_raw_response_rows";
const RENDERED_COLLECTION: &str = "crawlex_rendered_pages";
const RENDERED_TABLE: &str = "crawlex_rendered_page_rows";
const ARTIFACT_COLLECTION: &str = "crawlex_artifacts";
const STATE_COLLECTION: &str = "crawlex_session_state";
const EDGE_COLLECTION: &str = "crawlex_link_edges";
const TECH_FINGERPRINT_COLLECTION: &str = "crawlex_tech_fingerprints";
const HOST_TECH_COLLECTION: &str = "crawlex_host_tech";
const ASSET_REFS_COLLECTION: &str = "crawlex_asset_refs";
const HOST_FACTS_COLLECTION: &str = "crawlex_host_facts";
const CRAWL_STATS_COLLECTION: &str = "crawlex_crawl_stats";
const CRAWL_ATTEMPT_COLLECTION: &str = "crawlex_crawl_attempts";
const TELEMETRY_COLLECTION: &str = "crawlex_telemetry_events";
const METRICS_COLLECTION: &str = "crawlex_page_metrics";
const CHALLENGE_COLLECTION: &str = "crawlex_challenges";

// RedDB document inserts currently pass through the query/parser path and hit
// max_input_bytes at ~1MiB. Keep inline blobs comfortably below that so large
// pharmacy/SPA pages don't fail the crawl; the metadata still records the
// original size and whether the inline payload was truncated.
const REDDB_INLINE_BODY_LIMIT_BYTES: usize = 384 * 1024;
const REDDB_INLINE_HTML_LIMIT_BYTES: usize = 512 * 1024;

pub struct ReddbStorage {
    db: Reddb,
}

impl ReddbStorage {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let uri = crate::config::normalize_reddb_uri(path.as_ref().to_string_lossy());
        Self::open_uri(uri).await
    }

    pub async fn open_uri(uri: impl AsRef<str>) -> Result<Self> {
        Self::open_inner(uri.as_ref()).await
    }

    pub fn open_blocking(path: impl AsRef<Path>) -> Result<Self> {
        let uri = crate::config::normalize_reddb_uri(path.as_ref().to_string_lossy());
        Self::open_uri_blocking(uri)
    }

    pub fn open_uri_blocking(uri: impl AsRef<str>) -> Result<Self> {
        let uri = uri.as_ref().to_string();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(Self::open_inner(&uri))),
            Err(_) => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(Error::Io)?
                .block_on(Self::open_inner(&uri)),
        }
    }

    async fn open_inner(uri: &str) -> Result<Self> {
        reject_unsupported_red_wss(uri)?;
        let db = Reddb::connect(uri)
            .await
            .map_err(|e| Error::Storage(format!("open RedDB storage {uri}: {e}")))?;
        bootstrap_storage_schema(&db).await?;
        Ok(Self { db })
    }

    async fn insert_json(&self, collection: &str, value: JsonValue) -> Result<()> {
        self.db
            .documents()
            .insert(collection, &value)
            .await
            .map(|_| ())
            .map_err(|e| {
                Error::Storage(format!(
                    "RedDB document insert into {collection} failed: {e}"
                ))
            })
    }
}

async fn bootstrap_storage_schema(db: &Reddb) -> Result<()> {
    let statements = [
        format!("CREATE DOCUMENT IF NOT EXISTS {RAW_COLLECTION}"),
        format!("CREATE TABLE IF NOT EXISTS {RAW_TABLE}"),
        format!("CREATE DOCUMENT IF NOT EXISTS {RENDERED_COLLECTION}"),
        format!("CREATE TABLE IF NOT EXISTS {RENDERED_TABLE}"),
        format!("CREATE DOCUMENT IF NOT EXISTS {ARTIFACT_COLLECTION}"),
        format!("CREATE DOCUMENT IF NOT EXISTS {EDGE_COLLECTION}"),
        format!("CREATE DOCUMENT IF NOT EXISTS {TECH_FINGERPRINT_COLLECTION}"),
        format!("CREATE DOCUMENT IF NOT EXISTS {HOST_TECH_COLLECTION}"),
        "CREATE TABLE IF NOT EXISTS crawlex_pages".to_string(),
        format!("CREATE DOCUMENT IF NOT EXISTS {CRAWL_STATS_COLLECTION}"),
        format!("CREATE DOCUMENT IF NOT EXISTS {CRAWL_ATTEMPT_COLLECTION}"),
        format!("CREATE DOCUMENT IF NOT EXISTS {TELEMETRY_COLLECTION}"),
        "CREATE DOCUMENT IF NOT EXISTS crawlex_sessions".to_string(),
        "CREATE DOCUMENT IF NOT EXISTS crawlex_challenges".to_string(),
        "CREATE DOCUMENT IF NOT EXISTS crawlex_intel".to_string(),
        "CREATE TIMESERIES IF NOT EXISTS crawlex_crawl_events".to_string(),
        "CREATE METRIC IF NOT EXISTS crawlex_success_rate TYPE gauge AS SELECT 1".to_string(),
    ];
    for statement in statements {
        let _ = db.query(&statement).await;
    }
    Ok(())
}

fn crawl_attempt_document(attempt: &crate::crawl_stats::CrawlAttemptRecord) -> Result<JsonValue> {
    let attempt_json = serde_json::to_string(attempt)
        .map_err(|e| Error::Storage(format!("RedDB crawl attempt json failed: {e}")))?;
    let mut entries = vec![
        ("crawl_id", JsonValue::number(attempt.crawl_id as f64)),
        ("url", JsonValue::string(attempt.url.to_string())),
        (
            "attempt_index",
            JsonValue::number(attempt.attempt_index as f64),
        ),
        ("engine", JsonValue::string(attempt.engine.as_str())),
        (
            "success",
            JsonValue::bool(!attempt.blocked && attempt.error.is_none()),
        ),
        ("blocked", JsonValue::bool(attempt.blocked)),
        (
            "status",
            attempt
                .status
                .map(|v| JsonValue::number(v as f64))
                .unwrap_or_else(JsonValue::null),
        ),
        ("latency_ms", JsonValue::number(attempt.latency_ms as f64)),
        ("observed_at", JsonValue::number(attempt.observed_at as f64)),
        ("attempt_json", JsonValue::string(attempt_json)),
    ];

    if let Some(identity) = &attempt.access_identity {
        entries.extend([
            (
                "access_identity_key",
                JsonValue::string(
                    attempt
                        .access_identity_key
                        .clone()
                        .unwrap_or_else(|| identity.access_identity_key()),
                ),
            ),
            (
                "target_host",
                JsonValue::string(identity.target_host.clone()),
            ),
            (
                "endpoint_class",
                JsonValue::string(identity.endpoint_class.clone()),
            ),
            (
                "proxy_provider",
                identity
                    .proxy_provider
                    .clone()
                    .map(JsonValue::string)
                    .unwrap_or_else(JsonValue::null),
            ),
            (
                "proxy_profile_id",
                identity
                    .proxy_profile_id
                    .clone()
                    .map(JsonValue::string)
                    .unwrap_or_else(JsonValue::null),
            ),
            (
                "sticky_identity_id",
                identity
                    .sticky_identity_id
                    .clone()
                    .map(JsonValue::string)
                    .unwrap_or_else(JsonValue::null),
            ),
            (
                "exit_ip",
                identity
                    .exit_ip
                    .clone()
                    .map(JsonValue::string)
                    .unwrap_or_else(JsonValue::null),
            ),
            ("persona_id", JsonValue::string(identity.persona_id.clone())),
            (
                "tls_profile_name",
                JsonValue::string(identity.tls_profile_name.clone()),
            ),
            (
                "headers_profile_hash",
                identity
                    .headers_profile_hash
                    .clone()
                    .map(JsonValue::string)
                    .unwrap_or_else(JsonValue::null),
            ),
            (
                "ua_hash",
                identity
                    .ua_hash
                    .clone()
                    .map(JsonValue::string)
                    .unwrap_or_else(JsonValue::null),
            ),
        ]);
    }

    Ok(JsonValue::object(entries))
}

#[async_trait::async_trait]
impl ArtifactStorage for ReddbStorage {
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
        let inline_body = truncate_bytes(body.as_ref(), REDDB_INLINE_BODY_LIMIT_BYTES);
        let inline_truncated = truncated || inline_body.len() < body.len();
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(inline_body);
        self.db
            .insert(
                RAW_TABLE,
                &JsonValue::object([
                    ("url", JsonValue::string(url.to_string())),
                    (
                        "canonical_url",
                        JsonValue::string(crate::url_util::canonicalize(url)),
                    ),
                    ("final_url", JsonValue::string(final_url.to_string())),
                    ("status", JsonValue::number(status)),
                    ("headers_json", JsonValue::string(headers_to_json(headers))),
                    ("body_base64", JsonValue::string(body_base64)),
                    ("body_bytes", JsonValue::number(body.len() as f64)),
                    (
                        "inline_body_bytes",
                        JsonValue::number(inline_body.len() as f64),
                    ),
                    ("truncated", JsonValue::bool(inline_truncated)),
                    ("saved_at_unix", JsonValue::number(now_unix())),
                ]),
            )
            .await
            .map(|_| ())
            .map_err(|e| Error::Storage(format!("RedDB raw table insert failed: {e}")))
    }

    async fn save_rendered(
        &self,
        url: &Url,
        html_post_js: &str,
        meta: &PageMetadata,
    ) -> Result<()> {
        let inline_html = truncate_str(html_post_js, REDDB_INLINE_HTML_LIMIT_BYTES);
        let inline_truncated = inline_html.len() < html_post_js.len();
        self.db
            .insert(
                RENDERED_TABLE,
                &JsonValue::object([
                    ("url", JsonValue::string(url.to_string())),
                    (
                        "canonical_url",
                        JsonValue::string(crate::url_util::canonicalize(url)),
                    ),
                    ("final_url", JsonValue::string(meta.final_url.to_string())),
                    ("status", JsonValue::number(meta.status)),
                    ("bytes", JsonValue::number(meta.bytes as f64)),
                    ("rendered", JsonValue::bool(meta.rendered)),
                    ("kind", JsonValue::string(format!("{:?}", meta.kind))),
                    ("html", JsonValue::string(inline_html)),
                    ("html_bytes", JsonValue::number(html_post_js.len() as f64)),
                    (
                        "inline_html_bytes",
                        JsonValue::number(inline_html.len() as f64),
                    ),
                    ("truncated", JsonValue::bool(inline_truncated)),
                    ("saved_at_unix", JsonValue::number(now_unix())),
                ]),
            )
            .await
            .map(|_| ())
            .map_err(|e| Error::Storage(format!("RedDB rendered table insert failed: {e}")))
    }

    async fn save_edge(&self, from: &Url, to: &Url) -> Result<()> {
        self.insert_json(
            EDGE_COLLECTION,
            JsonValue::object([
                ("from", JsonValue::string(from.to_string())),
                ("to", JsonValue::string(to.to_string())),
                ("saved_at_unix", JsonValue::number(now_unix())),
            ]),
        )
        .await
    }

    async fn save_artifact(&self, meta: &ArtifactMeta<'_>, bytes: &[u8]) -> Result<Option<String>> {
        let location = format!(
            "reddb://{ARTIFACT_COLLECTION}/{}:{}",
            meta.session_id,
            meta.name.unwrap_or(meta.kind.wire_str())
        );
        self.insert_json(
            ARTIFACT_COLLECTION,
            JsonValue::object([
                ("url", JsonValue::string(meta.url.to_string())),
                (
                    "final_url",
                    meta.final_url
                        .map(|u| JsonValue::string(u.to_string()))
                        .unwrap_or_else(JsonValue::null),
                ),
                ("session_id", JsonValue::string(meta.session_id)),
                ("kind", JsonValue::string(meta.kind.wire_str())),
                (
                    "name",
                    meta.name
                        .map(JsonValue::string)
                        .unwrap_or_else(JsonValue::null),
                ),
                (
                    "step_id",
                    meta.step_id
                        .map(JsonValue::string)
                        .unwrap_or_else(JsonValue::null),
                ),
                (
                    "step_kind",
                    meta.step_kind
                        .map(JsonValue::string)
                        .unwrap_or_else(JsonValue::null),
                ),
                (
                    "selector",
                    meta.selector
                        .map(JsonValue::string)
                        .unwrap_or_else(JsonValue::null),
                ),
                (
                    "mime",
                    JsonValue::string(meta.mime.unwrap_or(meta.kind.mime())),
                ),
                ("size", JsonValue::number(bytes.len() as f64)),
                (
                    "bytes_base64",
                    JsonValue::string(base64::engine::general_purpose::STANDARD.encode(bytes)),
                ),
                ("location", JsonValue::string(location.clone())),
                ("created_at_unix", JsonValue::number(now_unix())),
            ]),
        )
        .await?;
        Ok(Some(location))
    }

    async fn list_artifacts(
        &self,
        session_id: Option<&str>,
        kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRow>> {
        let mut sql = format!("SELECT * FROM {ARTIFACT_COLLECTION}");
        let mut clauses = Vec::new();
        if let Some(session_id) = session_id {
            clauses.push(format!("session_id = {}", sql_string_literal(session_id)));
        }
        if let Some(kind) = kind {
            clauses.push(format!("kind = {}", sql_string_literal(kind.wire_str())));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        let result = self
            .db
            .query(&sql)
            .await
            .map_err(|e| Error::Storage(format!("RedDB artifact list failed: {e}")))?;
        let mut rows = Vec::new();
        for (idx, row) in result.rows.into_iter().enumerate() {
            if let Some(parsed) = artifact_row_from_values(idx as i64, &row) {
                rows.push(parsed);
            }
        }
        Ok(rows)
    }

    async fn page_cache_metadata(&self, url: &Url) -> Result<Option<PageCacheMetadata>> {
        let canonical_url = crate::url_util::canonicalize(url);
        let sql = format!(
            "SELECT * FROM {RAW_COLLECTION} WHERE canonical_url = {} OR url = {} LIMIT 1",
            sql_string_literal(&canonical_url),
            sql_string_literal(url.as_str())
        );
        let result = self
            .db
            .query(&sql)
            .await
            .map_err(|e| Error::Storage(format!("RedDB page cache metadata failed: {e}")))?;
        let Some(row) = result.rows.into_iter().next() else {
            return Ok(None);
        };
        let final_url = row_string(&row, "final_url")
            .and_then(|s| Url::parse(&s).ok())
            .unwrap_or_else(|| url.clone());
        let headers_json = row_string(&row, "headers_json")
            .or_else(|| row_string(&row, "headers"))
            .unwrap_or_else(|| "{}".to_string());
        let headers: serde_json::Value = serde_json::from_str(&headers_json).unwrap_or_default();
        let header = |name: &str| -> Option<String> {
            headers
                .get(name)
                .or_else(|| headers.get(&name.to_ascii_lowercase()))
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        };
        Ok(Some(PageCacheMetadata {
            url: url.clone(),
            final_url,
            status: row_string(&row, "status")
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            etag: header("etag"),
            last_modified: header("last-modified"),
            head_fingerprint: None,
            saved_at_unix: row_string(&row, "saved_at_unix")
                .and_then(|s| s.parse::<f64>().ok())
                .map(|n| n as u64)
                .unwrap_or_default(),
        }))
    }
}

#[async_trait::async_trait]
impl StateStorage for ReddbStorage {
    async fn save_state(&self, session_id: &str, state_json: &str) -> Result<()> {
        self.db
            .kv_collection(STATE_COLLECTION)
            .set(&state_key(session_id), JsonValue::string(state_json))
            .await
            .map(|_| ())
            .map_err(|e| Error::Storage(format!("RedDB session state save failed: {e}")))
    }

    async fn load_state(&self, session_id: &str) -> Result<Option<String>> {
        self.db
            .kv_collection(STATE_COLLECTION)
            .get(&state_key(session_id))
            .await
            .map(|item| item.and_then(|item| value_to_string(&item.value)))
            .map_err(|e| Error::Storage(format!("RedDB session state load failed: {e}")))
    }

    async fn archive_session(
        &self,
        entry: &crate::identity::SessionEntry,
        reason: crate::identity::EvictionReason,
    ) -> Result<()> {
        self.insert_json(
            "crawlex_sessions",
            JsonValue::object([
                ("session_id", JsonValue::string(entry.id.clone())),
                ("reason", JsonValue::string(format!("{:?}", reason))),
                ("archived_at_unix", JsonValue::number(now_unix())),
            ]),
        )
        .await
    }
}

#[async_trait::async_trait]
impl ChallengeStorage for ReddbStorage {
    async fn record_challenge(&self, signal: &crate::antibot::ChallengeSignal) -> Result<()> {
        self.insert_json(
            CHALLENGE_COLLECTION,
            JsonValue::object([
                ("session_id", JsonValue::string(signal.session_id.clone())),
                ("url", JsonValue::string(signal.url.to_string())),
                ("vendor", JsonValue::string(format!("{:?}", signal.vendor))),
                ("level", JsonValue::string(format!("{:?}", signal.level))),
                (
                    "observed_at",
                    JsonValue::number(
                        signal
                            .first_seen
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs_f64())
                            .unwrap_or_default(),
                    ),
                ),
                (
                    "signal_json",
                    JsonValue::string(serde_json::to_string(signal).unwrap_or_default()),
                ),
            ]),
        )
        .await
    }

    async fn session_challenges(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::antibot::ChallengeSignal>> {
        let sql = format!(
            "SELECT signal_json FROM {CHALLENGE_COLLECTION} WHERE session_id = {}",
            sql_string_literal(session_id)
        );
        let result = self
            .db
            .query(&sql)
            .await
            .map_err(|e| Error::Storage(format!("RedDB challenge load failed: {e}")))?;
        let mut out: Vec<crate::antibot::ChallengeSignal> = Vec::new();
        for row in result.rows {
            if let Some(json) = row_string(&row, "signal_json") {
                if let Ok(signal) = serde_json::from_str(&json) {
                    out.push(signal);
                }
            }
        }
        out.sort_by_key(|signal| signal.first_seen);
        Ok(out)
    }
}

#[async_trait::async_trait]
impl TelemetryStorage for ReddbStorage {
    async fn save_metrics(&self, url: &Url, metrics: &crate::metrics::PageMetrics) -> Result<()> {
        self.insert_json(
            METRICS_COLLECTION,
            JsonValue::object([
                ("url", JsonValue::string(url.to_string())),
                (
                    "metrics_json",
                    JsonValue::string(serde_json::to_string(metrics).unwrap_or_default()),
                ),
                ("saved_at_unix", JsonValue::number(now_unix())),
            ]),
        )
        .await
    }

    async fn record_telemetry(
        &self,
        telem: &crate::antibot::telemetry::VendorTelemetry,
    ) -> Result<()> {
        self.insert_json(
            TELEMETRY_COLLECTION,
            JsonValue::object([
                ("vendor", JsonValue::string(format!("{:?}", telem.vendor))),
                ("endpoint", JsonValue::string(telem.endpoint.to_string())),
                (
                    "telemetry_json",
                    JsonValue::string(serde_json::to_string(telem).unwrap_or_default()),
                ),
                ("saved_at_unix", JsonValue::number(now_unix())),
            ]),
        )
        .await
    }

    async fn record_crawl_attempt(
        &self,
        attempt: &crate::crawl_stats::CrawlAttemptRecord,
    ) -> Result<()> {
        self.insert_json(CRAWL_ATTEMPT_COLLECTION, crawl_attempt_document(attempt)?)
            .await
    }

    async fn record_crawl_stats(&self, stats: &crate::crawl_stats::CrawlStats) -> Result<()> {
        self.insert_json(
            CRAWL_STATS_COLLECTION,
            JsonValue::object([
                ("crawl_id", JsonValue::number(stats.crawl_id as f64)),
                ("url", JsonValue::string(stats.url.to_string())),
                (
                    "stats_json",
                    JsonValue::string(serde_json::to_string(stats).unwrap_or_default()),
                ),
            ]),
        )
        .await
    }
}
#[async_trait::async_trait]
impl IntelStorage for ReddbStorage {
    async fn save_tech_fingerprint(
        &self,
        report: &crate::discovery::tech_fingerprint::TechFingerprintReport,
    ) -> Result<()> {
        if report.host.is_empty() {
            return Ok(());
        }

        let report_json = serde_json::to_string(report)
            .map_err(|e| Error::Storage(format!("RedDB tech fingerprint json failed: {e}")))?;
        self.insert_json(
            TECH_FINGERPRINT_COLLECTION,
            JsonValue::object([
                ("url", JsonValue::string(report.url.clone())),
                ("final_url", JsonValue::string(report.final_url.clone())),
                ("host", JsonValue::string(report.host.clone())),
                (
                    "generated_at",
                    JsonValue::number(report.generated_at as f64),
                ),
                ("report_json", JsonValue::string(report_json.clone())),
                (
                    "technology_count",
                    JsonValue::number(report.technologies.len() as f64),
                ),
            ]),
        )
        .await?;

        for tech in &report.technologies {
            self.insert_json(
                HOST_TECH_COLLECTION,
                JsonValue::object([
                    ("host", JsonValue::string(report.host.clone())),
                    ("slug", JsonValue::string(tech.slug.clone())),
                    ("name", JsonValue::string(tech.name.clone())),
                    ("category", JsonValue::string(tech.category.clone())),
                    ("seen_count", JsonValue::number(1)),
                    ("confidence_max", JsonValue::number(tech.confidence)),
                    ("confidence_avg", JsonValue::number(tech.confidence)),
                    ("first_seen", JsonValue::number(report.generated_at as f64)),
                    ("last_seen", JsonValue::number(report.generated_at as f64)),
                    ("sample_url", JsonValue::string(report.url.clone())),
                    (
                        "evidence_json",
                        JsonValue::string(serde_json::to_string(&tech.evidence).map_err(|e| {
                            Error::Storage(format!("RedDB tech evidence json failed: {e}"))
                        })?),
                    ),
                ]),
            )
            .await?;
        }

        Ok(())
    }
}

impl Storage for ReddbStorage {
    fn as_any_ref(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

fn artifact_row_from_values(id: i64, row: &[(String, ValueOut)]) -> Option<ArtifactRow> {
    let url = Url::parse(&row_string(row, "url")?).ok()?;
    let final_url = row_string(row, "final_url").and_then(|s| Url::parse(&s).ok());
    let kind = row_string(row, "kind")
        .and_then(|s| ArtifactKind::from_wire(&s))
        .unwrap_or(ArtifactKind::SnapshotResponseBody);
    Some(ArtifactRow {
        id,
        url,
        final_url,
        session_id: row_string(row, "session_id").unwrap_or_default(),
        kind,
        name: row_string(row, "name"),
        step_id: row_string(row, "step_id"),
        step_kind: row_string(row, "step_kind"),
        selector: row_string(row, "selector"),
        mime: row_string(row, "mime").unwrap_or_else(|| kind.mime().to_string()),
        sha256: row_string(row, "sha256").unwrap_or_default(),
        size: row_string(row, "size")
            .and_then(|s| s.parse().ok())
            .unwrap_or_default(),
        created_at: row_string(row, "created_at_unix")
            .and_then(|s| s.parse::<u64>().ok())
            .map(|s| UNIX_EPOCH + std::time::Duration::from_secs(s))
            .unwrap_or(SystemTime::UNIX_EPOCH),
    })
}

fn row_string(row: &[(String, ValueOut)], name: &str) -> Option<String> {
    row.iter().find_map(|(column, value)| {
        if column == name {
            value_to_string(value)
        } else {
            None
        }
    })
}

fn value_to_string(value: &ValueOut) -> Option<String> {
    match value {
        ValueOut::String(s) => Some(s.clone()),
        ValueOut::Integer(n) => Some(n.to_string()),
        ValueOut::Float(n) => Some(n.to_string()),
        ValueOut::Bool(b) => Some(b.to_string()),
        ValueOut::Null => None,
    }
}

fn headers_to_json(headers: &HeaderMap) -> String {
    let mut map = serde_json::Map::new();
    for (key, value) in headers {
        if let Ok(value) = value.to_str() {
            map.insert(
                key.as_str().to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }
    serde_json::Value::Object(map).to_string()
}

fn truncate_bytes(bytes: &[u8], limit: usize) -> &[u8] {
    if bytes.len() <= limit {
        bytes
    } else {
        &bytes[..limit]
    }
}

fn truncate_str(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn state_key(session_id: &str) -> String {
    format!(
        "s_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(session_id)
    )
}

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

fn reject_unsupported_red_wss(uri: &str) -> Result<()> {
    if uri.to_ascii_lowercase().starts_with("red+wss://") {
        return Err(Error::Storage(
            "red+wss:// RedWire-over-WebSocket is documented but not yet wired in reddb-client Rust; use red://, reds://, grpc://, http://, file://, or memory://".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crawl_stats::{AccessIdentity, AttemptEngine, CrawlAttemptRecord};

    fn object_value<'a>(doc: &'a JsonValue, key: &str) -> Option<&'a JsonValue> {
        doc.as_object()?
            .iter()
            .find_map(|(k, v)| if k == key { Some(v) } else { None })
    }

    #[test]
    fn crawl_attempt_document_exposes_access_identity_for_reddb_rollups() {
        let mut identity = AccessIdentity::new(
            "www.drogariasaopaulo.com.br",
            "vtex_api",
            "pixel",
            "chrome_99.0.4844.73_android12-pixel6",
        );
        identity.proxy_provider = Some("packetstream".into());
        identity.proxy_profile_id = Some("packetstream-default".into());
        identity.sticky_identity_id =
            Some("packetstream:www.drogariasaopaulo.com.br:vtex_api:pixel".into());
        identity.exit_ip = Some("192.0.2.10".into());
        identity.headers_profile_hash = Some("headers-a".into());
        identity.ua_hash = Some("ua-a".into());

        let attempt = CrawlAttemptRecord::new(
            7,
            "https://www.drogariasaopaulo.com.br/api/catalog_system/pub/products/search/paracetamol"
                .parse()
                .unwrap(),
            1,
            AttemptEngine::HttpSpoof,
            None,
            None,
            Some(206),
            false,
            None,
            None,
            None,
            42,
            None,
        )
        .with_access_identity(identity);

        let doc = crawl_attempt_document(&attempt).unwrap();
        assert!(matches!(
            object_value(&doc, "access_identity_key"),
            Some(JsonValue::String(value)) if value.contains("packetstream")
        ));
        assert_eq!(
            object_value(&doc, "target_host"),
            Some(&JsonValue::String("www.drogariasaopaulo.com.br".into()))
        );
        assert_eq!(
            object_value(&doc, "endpoint_class"),
            Some(&JsonValue::String("vtex_api".into()))
        );
        assert_eq!(
            object_value(&doc, "sticky_identity_id"),
            Some(&JsonValue::String(
                "packetstream:www.drogariasaopaulo.com.br:vtex_api:pixel".into()
            ))
        );
        assert_eq!(
            object_value(&doc, "exit_ip"),
            Some(&JsonValue::String("192.0.2.10".into()))
        );
        assert_eq!(
            object_value(&doc, "tls_profile_name"),
            Some(&JsonValue::String(
                "chrome_99.0.4844.73_android12-pixel6".into()
            ))
        );

        let json = doc.to_json_string();
        assert!(!json.contains("user:pass"));
        assert!(!json.contains("password"));
    }
}
