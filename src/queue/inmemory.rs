use parking_lot::Mutex;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::time::{Duration, Instant};

use crate::queue::{Job, JobQueue};
use crate::Result;

struct Prioritized {
    ready_at: Instant,
    job: Job,
}

impl Eq for Prioritized {}
impl PartialEq for Prioritized {
    fn eq(&self, other: &Self) -> bool {
        self.ready_at == other.ready_at
            && self.job.priority == other.job.priority
            && self.job.id == other.job.id
    }
}
impl Ord for Prioritized {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap pops the greatest item. Reverse ready_at so the
        // earliest ready job wins, then prefer higher priority.
        other
            .ready_at
            .cmp(&self.ready_at)
            .then_with(|| self.job.priority.cmp(&other.job.priority))
            .then_with(|| other.job.id.cmp(&self.job.id))
    }
}
impl PartialOrd for Prioritized {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Default)]
pub struct InMemoryQueue {
    heap: Mutex<BinaryHeap<Prioritized>>,
    in_flight: Mutex<HashMap<u64, Job>>,
}

impl InMemoryQueue {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl JobQueue for InMemoryQueue {
    async fn push(&self, job: Job) -> Result<()> {
        self.push_after(job, Duration::ZERO).await
    }

    async fn push_after(&self, job: Job, delay: Duration) -> Result<()> {
        self.heap.lock().push(Prioritized {
            ready_at: Instant::now() + delay,
            job,
        });
        Ok(())
    }

    async fn pop(&self) -> Result<Option<Job>> {
        let now = Instant::now();
        let mut heap = self.heap.lock();
        let Some(next) = heap.peek() else {
            return Ok(None);
        };
        if next.ready_at > now {
            return Ok(None);
        }
        let job = heap.pop().map(|p| p.job);
        if let Some(job) = job.as_ref() {
            self.in_flight.lock().insert(job.id, job.clone());
        }
        Ok(job)
    }

    async fn complete(&self, _id: u64) -> Result<()> {
        self.in_flight.lock().remove(&_id);
        Ok(())
    }

    async fn fail(&self, id: u64, err: &str, retry_after_secs: u64) -> Result<()> {
        let Some(mut job) = self.in_flight.lock().remove(&id) else {
            return Ok(());
        };
        job.attempts = job.attempts.saturating_add(1);
        job.last_error = Some(err.to_string());
        self.push_after(job, Duration::from_secs(retry_after_secs))
            .await?;
        Ok(())
    }

    async fn fail_permanently(&self, id: u64, _err: &str) -> Result<()> {
        self.in_flight.lock().remove(&id);
        Ok(())
    }

    async fn has_pending_render_jobs(&self) -> Result<bool> {
        Ok(self
            .heap
            .lock()
            .iter()
            .any(|p| matches!(p.job.method, crate::queue::FetchMethod::Render)))
    }

    async fn len(&self) -> Result<usize> {
        Ok(self.heap.lock().len())
    }

    async fn pending_count(&self) -> Result<usize> {
        self.len().await
    }

    async fn next_ready_delay(&self) -> Result<Option<Duration>> {
        let now = Instant::now();
        Ok(self.heap.lock().peek().map(|p| {
            p.ready_at
                .checked_duration_since(now)
                .unwrap_or(Duration::ZERO)
        }))
    }

    async fn peek_pending_urls(&self) -> Result<Vec<url::Url>> {
        Ok(self.heap.lock().iter().map(|p| p.job.url.clone()).collect())
    }
}
