pub mod inmemory;
#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use inmemory::InMemoryQueue;

use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FetchMethod {
    Auto,
    HttpSpoof,
    Render,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub url: Url,
    pub depth: u32,
    pub priority: i32,
    pub method: FetchMethod,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[async_trait::async_trait]
pub trait JobQueue: Send + Sync {
    async fn push(&self, job: Job) -> Result<()>;
    async fn push_after(&self, job: Job, delay: Duration) -> Result<()> {
        if delay.is_zero() {
            self.push(job).await
        } else {
            let _ = job;
            Err(Error::Queue(
                "push_after not implemented by this queue backend".into(),
            ))
        }
    }
    async fn requeue_after(&self, original_id: u64, job: Job, delay: Duration) -> Result<()> {
        self.push_after(job, delay).await?;
        self.complete(original_id).await
    }
    async fn pop(&self) -> Result<Option<Job>>;
    async fn complete(&self, id: u64) -> Result<()>;
    async fn fail(&self, id: u64, err: &str, retry_after_secs: u64) -> Result<()>;
    async fn fail_permanently(&self, id: u64, err: &str) -> Result<()> {
        self.fail(id, err, 0).await
    }
    async fn len(&self) -> Result<usize>;

    /// Count all pending jobs, including jobs whose delay has not elapsed.
    async fn pending_count(&self) -> Result<usize> {
        self.len().await
    }

    /// Delay until the next pending job becomes ready. `None` means there
    /// are no pending delayed jobs.
    async fn next_ready_delay(&self) -> Result<Option<Duration>> {
        Ok(None)
    }

    /// Read URLs of pending jobs without mutating the queue. Used by
    /// startup-time enrichment (crt.sh seeding, DNS probes) that needs to
    /// know what we're about to crawl. Default: empty list (not supported).
    async fn peek_pending_urls(&self) -> Result<Vec<Url>> {
        Ok(Vec::new())
    }

    /// Checks if there are pending render jobs in the queue.
    async fn has_pending_render_jobs(&self) -> Result<bool> {
        Ok(false)
    }
}
