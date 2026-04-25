//! Persistent SQLite-backed job queue.
//!
//! All DB access runs on a single dedicated OS thread via an mpsc channel;
//! async callers send `Op`s and await the per-op `oneshot` response. Three
//! wins vs. the previous `Mutex<Connection>` design:
//!
//!   1. No `Mutex` contention in the tokio runtime. Callers never block a
//!      worker thread waiting for the DB.
//!   2. No lock-ordering surprises if a future refactor adds a second
//!      Connection — the writer thread is the only consumer.
//!   3. Push ops batch naturally: when the channel has a backlog, we drain
//!      it into a single transaction. Under push amplification (discovery
//!      adds 5-50 children per page) this cuts DB commits by ~30×.
//!
//! The thread also runs a periodic `PRAGMA wal_checkpoint(TRUNCATE)` to
//! keep the WAL file from growing unboundedly on long runs.

use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

use crate::queue::{FetchMethod, Job, JobQueue};
use crate::{Error, Result};

/// Ops the writer thread executes. Each carries a oneshot channel so the
/// async caller can await the result.
enum Op {
    Push(Job, i64, oneshot::Sender<Result<()>>),
    Requeue(u64, Job, i64, oneshot::Sender<Result<()>>),
    Pop(oneshot::Sender<Result<Option<Job>>>),
    Complete(u64, oneshot::Sender<Result<()>>),
    Fail(u64, String, u64, oneshot::Sender<Result<()>>),
    FailPermanently(u64, String, oneshot::Sender<Result<()>>),
    Len(oneshot::Sender<Result<usize>>),
    PendingCount(oneshot::Sender<Result<usize>>),
    NextReadyDelay(oneshot::Sender<Result<Option<Duration>>>),
    HasPendingRender(oneshot::Sender<Result<bool>>),
    PeekPendingUrls(oneshot::Sender<Result<Vec<url::Url>>>),
}

/// SQLite queue with a single writer thread owning the Connection.
pub struct SqliteQueue {
    tx: mpsc::Sender<Op>,
    retry_max: Arc<AtomicU32>,
    // Keep the writer-thread JoinHandle alive for the lifetime of the
    // queue; when the queue is dropped, tx drops, writer loop exits, the
    // thread joins cleanly via this handle being dropped.
    _writer: Arc<std::thread::JoinHandle<()>>,
}

/// How often (ops or wall time) the writer auto-runs
/// `PRAGMA wal_checkpoint(TRUNCATE)`. Tuning knobs rather than tests'
/// knobs because they're operational, not behavioural.
const CHECKPOINT_EVERY_N_OPS: u64 = 1_000;
const CHECKPOINT_EVERY: Duration = Duration::from_secs(60);

impl SqliteQueue {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|e| Error::Queue(format!("open: {e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| Error::Queue(format!("wal: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| Error::Queue(format!("sync: {e}")))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS jobs (
                id INTEGER PRIMARY KEY,
                url TEXT NOT NULL,
                canonical_url TEXT NOT NULL DEFAULT '',
                depth INTEGER NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                method TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                state TEXT NOT NULL DEFAULT 'pending',
                not_before INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );
            CREATE INDEX IF NOT EXISTS idx_jobs_ready
                ON jobs(state, not_before, priority DESC, id);
            "#,
        )
        .map_err(|e| Error::Queue(format!("schema: {e}")))?;
        // Existing databases from older builds may not have canonical_url.
        let _ = conn.execute("ALTER TABLE jobs ADD COLUMN canonical_url TEXT", []);
        migrate_canonical_urls(&conn)
            .map_err(|e| Error::Queue(format!("migrate canonical_url: {e}")))?;
        conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_canonical_url
             ON jobs(canonical_url)
             WHERE canonical_url IS NOT NULL AND canonical_url <> ''",
            [],
        )
        .map_err(|e| Error::Queue(format!("canonical_url unique index: {e}")))?;
        // Reclaim any jobs stuck as `in_flight` from a previous crash.
        conn.execute(
            "UPDATE jobs SET state='pending' WHERE state='in_flight'",
            [],
        )
        .map_err(|e| Error::Queue(format!("reclaim: {e}")))?;

        let retry_max = Arc::new(AtomicU32::new(5));
        // Bounded to give backpressure; 4096 ops in-flight is plenty for
        // the ~1k rps worst-case scale target.
        let (tx, rx) = mpsc::channel::<Op>(4096);
        let writer_retry_max = retry_max.clone();
        let writer = std::thread::Builder::new()
            .name("crawlex-sqlitequeue".into())
            .spawn(move || writer_loop(conn, rx, writer_retry_max))
            .map_err(|e| Error::Queue(format!("writer spawn: {e}")))?;

        Ok(Self {
            tx,
            retry_max,
            _writer: Arc::new(writer),
        })
    }

    pub fn set_retry_max(&self, n: u32) {
        self.retry_max.store(n.max(1), Ordering::Relaxed);
    }

    fn method_to_str(m: FetchMethod) -> &'static str {
        match m {
            FetchMethod::Auto => "auto",
            FetchMethod::HttpSpoof => "spoof",
            FetchMethod::Render => "render",
        }
    }

    fn str_to_method(s: &str) -> FetchMethod {
        match s {
            "render" => FetchMethod::Render,
            "spoof" => FetchMethod::HttpSpoof,
            _ => FetchMethod::Auto,
        }
    }

    async fn send(&self, op: Op) -> Result<()> {
        self.tx
            .send(op)
            .await
            .map_err(|_| Error::Queue("writer thread terminated".into()))
    }
}

fn migrate_canonical_urls(conn: &Connection) -> rusqlite::Result<()> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut updates: Vec<(i64, String)> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, url FROM jobs ORDER BY id ASC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, raw_url) = row?;
            let canonical = url::Url::parse(&raw_url)
                .map(|u| crate::url_util::canonicalize(&u))
                .unwrap_or(raw_url);
            let n = seen.entry(canonical.clone()).or_insert(0);
            let value = if *n == 0 {
                canonical
            } else {
                format!("{canonical}#legacy-dup-{id}")
            };
            *n += 1;
            updates.push((id, value));
        }
    }

    let tx = conn.unchecked_transaction()?;
    {
        let mut update = tx.prepare("UPDATE jobs SET canonical_url=?2 WHERE id=?1")?;
        for (id, canonical) in updates {
            update.execute(params![id, canonical])?;
        }
    }
    tx.commit()
}

/// Writer loop: owns the `Connection`. Batches push ops into single
/// transactions when the channel backs up; other ops are
/// single-statement. Runs `PRAGMA wal_checkpoint(TRUNCATE)` after
/// `CHECKPOINT_EVERY_N_OPS` writes OR `CHECKPOINT_EVERY` elapsed.
fn writer_loop(mut conn: Connection, mut rx: mpsc::Receiver<Op>, retry_max: Arc<AtomicU32>) {
    let mut ops_since_checkpoint: u64 = 0;
    let mut last_checkpoint = Instant::now();

    loop {
        // Wait for at least one op; if the sender side has all been
        // dropped, exit cleanly.
        let Some(first) = rx.blocking_recv() else {
            break;
        };
        // Drain up to N consecutive push ops into a single transaction so
        // the common discovery burst ("page yielded 32 child URLs")
        // doesn't pay 32 × (begin+commit+fsync).
        let mut batch: Vec<(Job, i64, oneshot::Sender<Result<()>>)> = Vec::new();
        if let Op::Push(job, not_before, reply) = first {
            batch.push((job, not_before, reply));
            while let Ok(op) = rx.try_recv() {
                match op {
                    Op::Push(j, not_before, r) => batch.push((j, not_before, r)),
                    other => {
                        // Non-push op drained; flush batch first, then
                        // handle the op.
                        apply_push_batch(&mut conn, &mut batch);
                        ops_since_checkpoint += 1;
                        handle_op(&mut conn, other, &retry_max);
                        ops_since_checkpoint += 1;
                        maybe_checkpoint(&conn, &mut ops_since_checkpoint, &mut last_checkpoint);
                        continue;
                    }
                }
                if batch.len() >= 256 {
                    break;
                }
            }
            if !batch.is_empty() {
                let n = batch.len();
                apply_push_batch(&mut conn, &mut batch);
                ops_since_checkpoint += n as u64;
            }
        } else {
            handle_op(&mut conn, first, &retry_max);
            ops_since_checkpoint += 1;
        }
        maybe_checkpoint(&conn, &mut ops_since_checkpoint, &mut last_checkpoint);
    }
    // Final checkpoint on shutdown so the WAL is fully merged.
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
}

fn apply_push_batch(
    conn: &mut Connection,
    batch: &mut Vec<(Job, i64, oneshot::Sender<Result<()>>)>,
) {
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("tx begin: {e}");
            for (_, _, reply) in batch.drain(..) {
                let _ = reply.send(Err(Error::Queue(msg.clone())));
            }
            return;
        }
    };
    let mut results: Vec<(oneshot::Sender<Result<()>>, Result<()>)> =
        Vec::with_capacity(batch.len());
    {
        let mut stmt = match tx.prepare_cached(
            "INSERT OR IGNORE INTO jobs
                (url, canonical_url, depth, priority, method, attempts, not_before)
             VALUES (?,?,?,?,?,?,?)",
        ) {
            Ok(s) => s,
            Err(e) => {
                let msg = format!("tx prep: {e}");
                for (_, _, reply) in batch.drain(..) {
                    let _ = reply.send(Err(Error::Queue(msg.clone())));
                }
                return;
            }
        };
        for (job, not_before, reply) in batch.drain(..) {
            let canonical = crate::url_util::canonicalize(&job.url);
            let r = stmt
                .execute(params![
                    job.url.to_string(),
                    canonical,
                    job.depth as i64,
                    job.priority as i64,
                    SqliteQueue::method_to_str(job.method),
                    job.attempts as i64,
                    not_before,
                ])
                .map(|_| ())
                .map_err(|e| Error::Queue(format!("insert: {e}")));
            results.push((reply, r));
        }
    }
    if let Err(e) = tx.commit() {
        let msg = format!("tx commit: {e}");
        for (reply, _) in results {
            let _ = reply.send(Err(Error::Queue(msg.clone())));
        }
        return;
    }
    for (reply, r) in results {
        let _ = reply.send(r);
    }
}

fn handle_op(conn: &mut Connection, op: Op, retry_max: &AtomicU32) {
    match op {
        Op::Push(job, not_before, reply) => {
            let canonical = crate::url_util::canonicalize(&job.url);
            let r = conn
                .execute(
                    "INSERT OR IGNORE INTO jobs
                        (url, canonical_url, depth, priority, method, attempts, not_before)
                     VALUES (?,?,?,?,?,?,?)",
                    params![
                        job.url.to_string(),
                        canonical,
                        job.depth as i64,
                        job.priority as i64,
                        SqliteQueue::method_to_str(job.method),
                        job.attempts as i64,
                        not_before,
                    ],
                )
                .map(|_| ())
                .map_err(|e| Error::Queue(format!("insert: {e}")));
            let _ = reply.send(r);
        }
        Op::Requeue(original_id, job, not_before, reply) => {
            let canonical = crate::url_util::canonicalize(&job.url);
            let r = conn
                .execute(
                    "UPDATE jobs
                     SET url=?2,
                         canonical_url=?3,
                         depth=?4,
                         priority=?5,
                         method=?6,
                         attempts=?7,
                         last_error=?8,
                         state='pending',
                         not_before=?9,
                         updated_at=strftime('%s','now')
                     WHERE id=?1",
                    params![
                        original_id as i64,
                        job.url.to_string(),
                        canonical,
                        job.depth as i64,
                        job.priority as i64,
                        SqliteQueue::method_to_str(job.method),
                        job.attempts as i64,
                        job.last_error,
                        not_before,
                    ],
                )
                .and_then(|n| {
                    if n == 0 {
                        Err(rusqlite::Error::QueryReturnedNoRows)
                    } else {
                        Ok(())
                    }
                })
                .map_err(|e| Error::Queue(format!("requeue: {e}")));
            let _ = reply.send(r);
        }
        Op::Pop(reply) => {
            let now: i64 = chrono_now();
            let r: Result<Option<Job>> = (|| {
                let row = match conn.query_row(
                    "UPDATE jobs
                     SET state='in_flight',
                         updated_at=strftime('%s','now')
                     WHERE id = (
                         SELECT id FROM jobs
                         WHERE state='pending' AND not_before <= ?1
                         ORDER BY priority DESC, id ASC
                         LIMIT 1
                     )
                     RETURNING id, url, depth, priority, method, attempts, last_error",
                    params![now],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, Option<String>>(6)?,
                        ))
                    },
                ) {
                    Ok(row) => Some(row),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(Error::Queue(format!("pop: {e}"))),
                };
                let Some((id, url, depth, priority, method, attempts, last_error)) = row else {
                    return Ok(None);
                };
                let url_p = url::Url::parse(&url).map_err(Error::UrlParse)?;
                Ok(Some(Job {
                    id: id as u64,
                    url: url_p,
                    depth: depth as u32,
                    priority: priority as i32,
                    method: SqliteQueue::str_to_method(&method),
                    attempts: attempts as u32,
                    last_error,
                }))
            })();
            let _ = reply.send(r);
        }
        Op::Complete(id, reply) => {
            let r = conn
                .execute(
                    "UPDATE jobs SET state='done', updated_at=strftime('%s','now') WHERE id=?1",
                    params![id as i64],
                )
                .map(|_| ())
                .map_err(|e| Error::Queue(format!("complete: {e}")));
            let _ = reply.send(r);
        }
        Op::Fail(id, err, retry_after_secs, reply) => {
            let not_before = chrono_now() + retry_after_secs as i64;
            let cap = retry_max.load(Ordering::Relaxed) as i64;
            let r = conn
                .execute(
                    "UPDATE jobs
                     SET state=CASE WHEN attempts+1 >= ?4 THEN 'failed' ELSE 'pending' END,
                         attempts=attempts+1,
                         last_error=?2,
                         not_before=?3,
                         updated_at=strftime('%s','now')
                     WHERE id=?1",
                    params![id as i64, err, not_before, cap],
                )
                .map(|_| ())
                .map_err(|e| Error::Queue(format!("fail: {e}")));
            let _ = reply.send(r);
        }
        Op::FailPermanently(id, err, reply) => {
            let r = conn
                .execute(
                    "UPDATE jobs
                     SET state='failed',
                         attempts=attempts+1,
                         last_error=?2,
                         not_before=strftime('%s','now'),
                         updated_at=strftime('%s','now')
                     WHERE id=?1",
                    params![id as i64, err],
                )
                .map(|_| ())
                .map_err(|e| Error::Queue(format!("fail_permanently: {e}")));
            let _ = reply.send(r);
        }
        Op::Len(reply) => {
            let r: Result<usize> = conn
                .query_row(
                    "SELECT COUNT(*) FROM jobs
                     WHERE state='pending' AND not_before <= strftime('%s','now')",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n as usize)
                .map_err(|e| Error::Queue(format!("len: {e}")));
            let _ = reply.send(r);
        }
        Op::PendingCount(reply) => {
            let r: Result<usize> = conn
                .query_row("SELECT COUNT(*) FROM jobs WHERE state='pending'", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map(|n| n as usize)
                .map_err(|e| Error::Queue(format!("pending_count: {e}")));
            let _ = reply.send(r);
        }
        Op::NextReadyDelay(reply) => {
            let now = chrono_now();
            let r: Result<Option<Duration>> = conn
                .query_row(
                    "SELECT MIN(not_before) FROM jobs WHERE state='pending'",
                    [],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .map(|ts| {
                    ts.map(|ts| {
                        if ts <= now {
                            Duration::ZERO
                        } else {
                            Duration::from_secs((ts - now) as u64)
                        }
                    })
                })
                .map_err(|e| Error::Queue(format!("next_ready_delay: {e}")));
            let _ = reply.send(r);
        }
        Op::HasPendingRender(reply) => {
            let r: Result<bool> = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM jobs WHERE state='pending' AND method='render' LIMIT 1)",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n != 0)
                .map_err(|e| Error::Queue(format!("has_pending_render_jobs: {e}")));
            let _ = reply.send(r);
        }
        Op::PeekPendingUrls(reply) => {
            let r: Result<Vec<url::Url>> = (|| {
                let mut stmt = conn
                    .prepare("SELECT url FROM jobs WHERE state='pending'")
                    .map_err(|e| Error::Queue(format!("peek prepare: {e}")))?;
                let urls: Vec<url::Url> = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(|e| Error::Queue(format!("peek query: {e}")))?
                    .filter_map(|r| r.ok())
                    .filter_map(|s| url::Url::parse(&s).ok())
                    .collect();
                Ok(urls)
            })();
            let _ = reply.send(r);
        }
    }
}

fn maybe_checkpoint(conn: &Connection, ops_since: &mut u64, last: &mut Instant) {
    if *ops_since >= CHECKPOINT_EVERY_N_OPS || last.elapsed() >= CHECKPOINT_EVERY {
        // TRUNCATE mode: after the checkpoint, reset the WAL file to
        // zero size. Prevents unbounded growth on long runs. Best-effort;
        // a failure (e.g. reader holding the DB) just means we try again
        // next tick.
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        *ops_since = 0;
        *last = Instant::now();
    }
}

#[async_trait::async_trait]
impl JobQueue for SqliteQueue {
    async fn push(&self, job: Job) -> Result<()> {
        self.push_after(job, Duration::ZERO).await
    }

    async fn push_after(&self, job: Job, delay: Duration) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let delay_secs = if delay.is_zero() {
            0
        } else {
            ((delay.as_millis().saturating_add(999)) / 1_000).min(i64::MAX as u128) as i64
        };
        let not_before = chrono_now().saturating_add(delay_secs);
        self.send(Op::Push(job, not_before, tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("push: writer dropped reply".into()))?
    }

    async fn requeue_after(&self, original_id: u64, job: Job, delay: Duration) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let delay_secs = if delay.is_zero() {
            0
        } else {
            ((delay.as_millis().saturating_add(999)) / 1_000).min(i64::MAX as u128) as i64
        };
        let not_before = chrono_now().saturating_add(delay_secs);
        self.send(Op::Requeue(original_id, job, not_before, tx))
            .await?;
        rx.await
            .map_err(|_| Error::Queue("requeue_after: writer dropped reply".into()))?
    }

    async fn pop(&self) -> Result<Option<Job>> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Pop(tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("pop: writer dropped reply".into()))?
    }

    async fn complete(&self, id: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Complete(id, tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("complete: writer dropped reply".into()))?
    }

    async fn fail(&self, id: u64, err: &str, retry_after_secs: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Fail(id, err.to_string(), retry_after_secs, tx))
            .await?;
        rx.await
            .map_err(|_| Error::Queue("fail: writer dropped reply".into()))?
    }

    async fn fail_permanently(&self, id: u64, err: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::FailPermanently(id, err.to_string(), tx))
            .await?;
        rx.await
            .map_err(|_| Error::Queue("fail_permanently: writer dropped reply".into()))?
    }

    async fn len(&self) -> Result<usize> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Len(tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("len: writer dropped reply".into()))?
    }

    async fn pending_count(&self) -> Result<usize> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::PendingCount(tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("pending_count: writer dropped reply".into()))?
    }

    async fn next_ready_delay(&self) -> Result<Option<Duration>> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::NextReadyDelay(tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("next_ready_delay: writer dropped reply".into()))?
    }

    async fn has_pending_render_jobs(&self) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::HasPendingRender(tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("has_pending_render_jobs: writer dropped reply".into()))?
    }

    async fn peek_pending_urls(&self) -> Result<Vec<url::Url>> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::PeekPendingUrls(tx)).await?;
        rx.await
            .map_err(|_| Error::Queue("peek_pending_urls: writer dropped reply".into()))?
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
