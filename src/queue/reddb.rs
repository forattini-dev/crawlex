use parking_lot::Mutex;
use reddb_client::types::{JsonValue, ValueOut};
use reddb_client::Reddb;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use url::Url;

use crate::queue::{FetchMethod, Job, JobQueue, QueueInsert};
use crate::{Error, Result};

const FRONTIER_QUEUE: &str = "crawlex_frontier";
const DLQ_QUEUE: &str = "crawlex_dead_letter";
const WORKERS_GROUP: &str = "workers";
const CONSUMER: &str = "crawlex";
const JOB_TABLE: &str = "crawlex_frontier_jobs";
const SEEN_KV: &str = "crawlex_frontier_seen";

pub struct ReddbQueue {
    db: Reddb,
    in_flight: Mutex<HashMap<u64, Job>>,
    in_flight_messages: Mutex<HashMap<u64, String>>,
}

impl ReddbQueue {
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
            .map_err(|e| Error::Queue(format!("open RedDB queue {uri}: {e}")))?;
        bootstrap_queue_schema(&db).await?;
        Ok(Self {
            db,
            in_flight: Mutex::new(HashMap::new()),
            in_flight_messages: Mutex::new(HashMap::new()),
        })
    }

    async fn push_with_delay(&self, job: Job, delay: Duration) -> Result<()> {
        let persisted = PersistedJob::from(job);
        let queue_id = persisted.id;
        let value = serde_json::to_value(&persisted)
            .map_err(|e| Error::Queue(format!("encode RedDB queue payload: {e}")))?;
        let reddb_value = serde_to_reddb_json(value);
        self.db
            .insert(JOB_TABLE, &reddb_value)
            .await
            .map_err(|e| Error::Queue(format!("RedDB job insert failed: {e}")))?;

        // Keep Queue payload tiny; larger JSON payloads are redacted by the
        // current RedDB queue read path (`<json N bytes>`). The full job is
        // stored in JOB_TABLE and the Queue carries only the stable id.
        let mut sql = format!("QUEUE PUSH {FRONTIER_QUEUE} {queue_id}");
        if !delay.is_zero() {
            sql.push_str(&format!(" DELAY {}ms", delay.as_millis()));
        }
        self.db
            .query(&sql)
            .await
            .map_err(|e| Error::Queue(format!("RedDB QUEUE PUSH failed: {e}")))?;
        Ok(())
    }
}

async fn bootstrap_queue_schema(db: &Reddb) -> Result<()> {
    // Queue: durable crawl frontier. DLQ is reserved for exhausted jobs.
    // Table/Document/KV/Graph/Timeseries/METRIC bootstrap establishes the
    // RedDB-native persistence envelope used by storage in follow-up slices.
    let statements = [
            format!("CREATE TABLE IF NOT EXISTS {JOB_TABLE}"),
            format!("CREATE KV IF NOT EXISTS {SEEN_KV}"),
            format!("CREATE QUEUE IF NOT EXISTS {DLQ_QUEUE}"),
        format!("CREATE QUEUE IF NOT EXISTS {FRONTIER_QUEUE} WORK WITH DLQ {DLQ_QUEUE} MAX_ATTEMPTS 3 RETRY_DELAY 1s"),
        format!("QUEUE GROUP CREATE {FRONTIER_QUEUE} {WORKERS_GROUP}"),
        "CREATE TABLE IF NOT EXISTS crawlex_pages".to_string(),
        "CREATE TABLE IF NOT EXISTS crawlex_crawl_attempts".to_string(),
        "CREATE TABLE IF NOT EXISTS crawlex_telemetry_events".to_string(),
        "CREATE DOCUMENT IF NOT EXISTS crawlex_artifacts".to_string(),
        "CREATE DOCUMENT IF NOT EXISTS crawlex_sessions".to_string(),
        "CREATE DOCUMENT IF NOT EXISTS crawlex_challenges".to_string(),
        "CREATE DOCUMENT IF NOT EXISTS crawlex_intel".to_string(),
        "CREATE GRAPH IF NOT EXISTS crawlex_web_graph".to_string(),
        "CREATE TIMESERIES IF NOT EXISTS crawlex_crawl_events".to_string(),
        "CREATE METRIC IF NOT EXISTS crawlex_success_rate TYPE gauge AS SELECT 1".to_string(),
    ];

    for statement in statements {
        let _ = db.query(&statement).await;
    }
    Ok(())
}

#[async_trait::async_trait]
impl JobQueue for ReddbQueue {
    async fn push(&self, job: Job) -> Result<()> {
        self.push_with_delay(job, Duration::ZERO).await
    }

    async fn push_unique(&self, job: Job, canonical_key: String) -> Result<QueueInsert> {
        let kv = self.db.kv_collection(SEEN_KV);
        if kv.get(&canonical_key).await.ok().flatten().is_some() {
            return Ok(QueueInsert::Duplicate);
        }
        self.push_with_delay(job, Duration::ZERO).await?;
        let _ = kv.set(&canonical_key, JsonValue::bool(true)).await;
        Ok(QueueInsert::Inserted)
    }

    async fn push_after(&self, job: Job, delay: Duration) -> Result<()> {
        self.push_with_delay(job, delay).await
    }

    async fn requeue_after(&self, original_id: u64, job: Job, delay: Duration) -> Result<()> {
        self.complete(original_id).await?;
        self.push_after(job, delay).await
    }

    async fn pop(&self) -> Result<Option<Job>> {
        let result = self
            .db
            .query(&format!(
                "QUEUE READ {FRONTIER_QUEUE} GROUP {WORKERS_GROUP} CONSUMER {CONSUMER} COUNT 1"
            ))
            .await
            .map_err(|e| Error::Queue(format!("RedDB QUEUE READ failed: {e}")))?;
        let Some(row) = result.rows.into_iter().next() else {
            return Ok(None);
        };
        let message_id = row_string(&row, "message_id");
        let persisted = match queue_row_job_id(&row) {
            Some(id) => self.load_job(id).await?,
            None => persisted_job_from_row(&row)?,
        };
        let job = persisted.into_job()?;
        self.in_flight.lock().insert(job.id, job.clone());
        if let Some(message_id) = message_id {
            self.in_flight_messages.lock().insert(job.id, message_id);
        }
        Ok(Some(job))
    }

    async fn complete(&self, id: u64) -> Result<()> {
        self.in_flight.lock().remove(&id);
        let message_id = { self.in_flight_messages.lock().remove(&id) };
        if let Some(message_id) = message_id {
            self.db
                .query(&format!(
                    "QUEUE ACK {FRONTIER_QUEUE} GROUP {WORKERS_GROUP} {}",
                    sql_string_literal(&message_id)
                ))
                .await
                .map_err(|e| Error::Queue(format!("RedDB QUEUE ACK failed: {e}")))?;
        }
        Ok(())
    }

    async fn fail(&self, id: u64, err: &str, retry_after_secs: u64) -> Result<()> {
        let Some(mut job) = self.in_flight.lock().remove(&id) else {
            return Ok(());
        };
        job.attempts = job.attempts.saturating_add(1);
        job.last_error = Some(err.to_string());
        self.push_after(job, Duration::from_secs(retry_after_secs))
            .await
    }

    async fn fail_permanently(&self, id: u64, err: &str) -> Result<()> {
        let Some(mut job) = self.in_flight.lock().remove(&id) else {
            return Ok(());
        };
        job.last_error = Some(err.to_string());
        let persisted = PersistedJob::from(job);
        let queue_id = persisted.id;
        let value = serde_json::to_value(&persisted)
            .map_err(|e| Error::Queue(format!("encode RedDB DLQ payload: {e}")))?;
        let reddb_value = serde_to_reddb_json(value);
        self.db
            .insert(JOB_TABLE, &reddb_value)
            .await
            .map_err(|e| Error::Queue(format!("RedDB DLQ job insert failed: {e}")))?;
        self.db
            .query(&format!("QUEUE PUSH {DLQ_QUEUE} {queue_id}"))
            .await
            .map_err(|e| Error::Queue(format!("RedDB DLQ push failed: {e}")))?;
        Ok(())
    }

    async fn len(&self) -> Result<usize> {
        self.db
            .queue()
            .len(FRONTIER_QUEUE)
            .await
            .map(|n| n as usize)
            .map_err(|e| Error::Queue(format!("RedDB QUEUE LEN failed: {e}")))
    }

    async fn pending_count(&self) -> Result<usize> {
        self.len().await
    }

    async fn peek_pending_urls(&self) -> Result<Vec<Url>> {
        let result = self
            .db
            .queue()
            .peek(FRONTIER_QUEUE, Some(10_000))
            .await
            .map_err(|e| Error::Queue(format!("RedDB QUEUE PEEK failed: {e}")))?;
        let mut urls = Vec::new();
        for row in result.items {
            let job = match queue_row_job_id(&row) {
                Some(id) => self.load_job(id).await,
                None => persisted_job_from_row(&row),
            };
            if let Ok(job) = job {
                if let Ok(url) = Url::parse(&job.url) {
                    urls.push(url);
                }
            }
        }
        Ok(urls)
    }

    async fn has_pending_render_jobs(&self) -> Result<bool> {
        let result = self
            .db
            .queue()
            .peek(FRONTIER_QUEUE, Some(10_000))
            .await
            .map_err(|e| Error::Queue(format!("RedDB QUEUE PEEK failed: {e}")))?;
        for row in result.items {
            let job = match queue_row_job_id(&row) {
                Some(id) => self.load_job(id).await,
                None => persisted_job_from_row(&row),
            };
            if job
                .ok()
                .is_some_and(|job| matches!(job.method, FetchMethod::Render))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl ReddbQueue {
    async fn load_job(&self, id: u64) -> Result<PersistedJob> {
        let result = self
            .db
            .query(&format!(
                "SELECT * FROM {JOB_TABLE} WHERE id = {id} LIMIT 1"
            ))
            .await
            .map_err(|e| Error::Queue(format!("RedDB job lookup failed: {e}")))?;
        let Some(row) = result.rows.into_iter().next() else {
            return Err(Error::Queue(format!("RedDB queue job {id} not found")));
        };
        persisted_job_from_row(&row)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedJob {
    id: u64,
    crawl_id: u64,
    url: String,
    depth: u32,
    priority: i32,
    method: FetchMethod,
    attempts: u32,
    last_error: Option<String>,
}

impl From<Job> for PersistedJob {
    fn from(job: Job) -> Self {
        Self {
            id: job.id,
            crawl_id: job.crawl_id,
            url: job.url.to_string(),
            depth: job.depth,
            priority: job.priority,
            method: job.method,
            attempts: job.attempts,
            last_error: job.last_error,
        }
    }
}

impl PersistedJob {
    fn into_job(self) -> Result<Job> {
        Ok(Job {
            id: self.id,
            crawl_id: self.crawl_id,
            url: Url::parse(&self.url)?,
            depth: self.depth,
            priority: self.priority,
            method: self.method,
            attempts: self.attempts,
            last_error: self.last_error,
        })
    }
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

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

fn queue_row_job_id(row: &[(String, ValueOut)]) -> Option<u64> {
    row_u64(row, "payload")
        .or_else(|_| row_u64(row, "value"))
        .ok()
}

fn persisted_job_from_row(row: &[(String, ValueOut)]) -> Result<PersistedJob> {
    // Queue reads can return either a single payload/value column or a JSON
    // object flattened into columns. Support both so RedDB queue internals can
    // evolve without breaking Crawlex persistence.
    if let Some(payload) = row_string(row, "payload").or_else(|| row_string(row, "value")) {
        if let Ok(job) = serde_json::from_str::<PersistedJob>(&payload) {
            return Ok(job);
        }
    }

    let id = row_u64(row, "id")?;
    let crawl_id = row_u64(row, "crawl_id").unwrap_or_default();
    let url = row_string(row, "url")
        .ok_or_else(|| Error::Queue(format!("RedDB queue row missing url: {row:?}")))?;
    let depth = row_u64(row, "depth")? as u32;
    let priority = row_i64(row, "priority")? as i32;
    let method = row_string(row, "method")
        .and_then(|s| serde_json::from_str::<FetchMethod>(&format!("\"{s}\"")).ok())
        .unwrap_or(FetchMethod::Auto);
    let attempts = row_u64(row, "attempts").unwrap_or_default() as u32;
    let last_error = row_string(row, "last_error");

    Ok(PersistedJob {
        id,
        crawl_id,
        url,
        depth,
        priority,
        method,
        attempts,
        last_error,
    })
}

fn row_u64(row: &[(String, ValueOut)], name: &str) -> Result<u64> {
    row_i64(row, name).and_then(|n| {
        u64::try_from(n).map_err(|_| Error::Queue(format!("RedDB queue row {name} is negative")))
    })
}

fn row_i64(row: &[(String, ValueOut)], name: &str) -> Result<i64> {
    row.iter()
        .find_map(|(column, value)| {
            if column != name {
                return None;
            }
            match value {
                ValueOut::Integer(n) => Some(Ok(*n)),
                ValueOut::Float(n) => Some(Ok(*n as i64)),
                ValueOut::String(s) => Some(
                    s.parse::<i64>()
                        .map_err(|e| Error::Queue(format!("parse RedDB queue {name}: {e}"))),
                ),
                ValueOut::Bool(_) | ValueOut::Null => Some(Err(Error::Queue(format!(
                    "RedDB queue row {name} has invalid type"
                )))),
            }
        })
        .unwrap_or_else(|| {
            Err(Error::Queue(format!(
                "RedDB queue row missing {name}: {row:?}"
            )))
        })
}

#[allow(dead_code)]
fn serde_to_reddb_json(value: serde_json::Value) -> JsonValue {
    match value {
        serde_json::Value::Null => JsonValue::Null,
        serde_json::Value::Bool(b) => JsonValue::Bool(b),
        serde_json::Value::Number(n) => JsonValue::Number(n.as_f64().unwrap_or_default()),
        serde_json::Value::String(s) => JsonValue::String(s),
        serde_json::Value::Array(items) => {
            JsonValue::Array(items.into_iter().map(serde_to_reddb_json).collect())
        }
        serde_json::Value::Object(map) => JsonValue::Object(
            map.into_iter()
                .map(|(key, value)| (key, serde_to_reddb_json(value)))
                .collect(),
        ),
    }
}

fn reject_unsupported_red_wss(uri: &str) -> Result<()> {
    if uri.to_ascii_lowercase().starts_with("red+wss://") {
        return Err(Error::Queue(
            "red+wss:// RedWire-over-WebSocket is documented but not yet wired in reddb-client Rust; use red://, reds://, grpc://, http://, file://, or memory://".into(),
        ));
    }
    Ok(())
}
