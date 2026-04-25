use std::time::Duration;

use crawlex::queue::{FetchMethod, InMemoryQueue, Job, JobQueue};
use url::Url;

fn job(id: u64, path: &str) -> Job {
    Job {
        id,
        url: Url::parse(&format!("https://example.com/{path}")).unwrap(),
        depth: 0,
        priority: 0,
        method: FetchMethod::HttpSpoof,
        attempts: 0,
        last_error: None,
    }
}

#[tokio::test]
async fn in_memory_push_after_waits_until_ready() {
    let q = InMemoryQueue::new();
    q.push_after(job(1, "delayed"), Duration::from_millis(80))
        .await
        .unwrap();

    assert!(q.pop().await.unwrap().is_none());
    assert_eq!(q.pending_count().await.unwrap(), 1);
    assert!(q.next_ready_delay().await.unwrap().is_some());

    tokio::time::sleep(Duration::from_millis(100)).await;
    let got = q.pop().await.unwrap().expect("job should be ready");
    assert_eq!(got.id, 1);
}

#[tokio::test]
async fn in_memory_fail_requeues_inflight_job() {
    let q = InMemoryQueue::new();
    q.push(job(7, "retry")).await.unwrap();
    let got = q.pop().await.unwrap().unwrap();
    q.fail(got.id, "boom", 0).await.unwrap();
    let retry = q.pop().await.unwrap().unwrap();
    assert_eq!(retry.id, 7);
    assert_eq!(retry.attempts, 1);
    assert_eq!(retry.last_error.as_deref(), Some("boom"));
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_push_after_reports_pending_delay() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("q.db");
    let q = crawlex::queue::sqlite::SqliteQueue::open(&path).unwrap();
    q.push_after(job(0, "sqlite-delayed"), Duration::from_secs(1))
        .await
        .unwrap();

    assert!(q.pop().await.unwrap().is_none());
    assert_eq!(q.pending_count().await.unwrap(), 1);
    assert!(q.next_ready_delay().await.unwrap().is_some());
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_requeue_after_preserves_same_canonical_delayed_job() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("q-requeue.db");
    let q = crawlex::queue::sqlite::SqliteQueue::open(&path).unwrap();
    q.push(job(11, "same-url")).await.unwrap();

    let got = q.pop().await.unwrap().expect("initial job");
    let mut replacement = got.clone();
    replacement.id = 99;
    replacement.method = FetchMethod::Render;
    replacement.priority = replacement.priority.saturating_add(10);
    q.requeue_after(got.id, replacement, Duration::from_secs(1))
        .await
        .unwrap();

    assert!(q.pop().await.unwrap().is_none());
    assert_eq!(q.pending_count().await.unwrap(), 1);
    assert!(q.next_ready_delay().await.unwrap().is_some());
}
