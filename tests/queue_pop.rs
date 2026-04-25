//! Verifies `SqliteQueue::pop` is atomic under concurrent callers.
//!
//! Without atomic pop a row could be returned twice (double-dispatch
//! bug): two concurrent SELECTs both succeed before either UPDATE
//! flips the row to `in_flight`. We test by seeding N rows, popping
//! from M tasks concurrently and asserting every returned id is unique.

#![cfg(feature = "sqlite")]

use std::collections::HashSet;
use std::sync::Arc;

use crawlex::queue::sqlite::SqliteQueue;
use crawlex::queue::{FetchMethod, Job, JobQueue};
use url::Url;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_pop_returns_unique_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("q.db").to_string_lossy().to_string();
    let queue = Arc::new(SqliteQueue::open(&path).unwrap());

    const N: usize = 200;
    for i in 0..N {
        queue
            .push(Job {
                id: 0, // SQLite auto-id
                url: Url::parse(&format!("https://example.com/p/{i}")).unwrap(),
                depth: 0,
                priority: 0,
                method: FetchMethod::HttpSpoof,
                attempts: 0,
                last_error: None,
            })
            .await
            .unwrap();
    }

    // M poppers, each tries to drain the queue.
    const M: usize = 32;
    let mut tasks = Vec::with_capacity(M);
    for _ in 0..M {
        let q = queue.clone();
        tasks.push(tokio::spawn(async move {
            let mut got = Vec::new();
            while let Some(job) = q.pop().await.unwrap() {
                got.push(job.id);
            }
            got
        }));
    }

    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.unwrap());
    }

    // Every id we got back must be unique (no double-dispatch).
    let unique: HashSet<u64> = all.iter().copied().collect();
    assert_eq!(
        unique.len(),
        all.len(),
        "duplicate ids returned: total={} unique={}",
        all.len(),
        unique.len()
    );
    // And we should have drained all N rows.
    assert_eq!(unique.len(), N);
}
