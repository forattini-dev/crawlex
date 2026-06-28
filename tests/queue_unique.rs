use crawlex::frontier::UrlIdentity;
use crawlex::queue::{FetchMethod, InMemoryQueue, Job, JobQueue, QueueInsert};
use url::Url;

fn job(id: u64, raw_url: &str) -> Job {
    Job {
        id,
        crawl_id: 0,
        url: Url::parse(raw_url).unwrap(),
        depth: 0,
        priority: 0,
        method: FetchMethod::HttpSpoof,
        attempts: 0,
        last_error: None,
    }
}

fn key(raw_url: &str) -> String {
    UrlIdentity::from_url(&Url::parse(raw_url).unwrap()).canonical_key
}

#[tokio::test]
async fn in_memory_push_unique_dedupes_canonical_url_aliases() {
    let q = InMemoryQueue::new();

    let inserted = q
        .push_unique(
            job(
                1,
                "https://www.example.com/store/index.html?utm_source=x&a=1",
            ),
            key("https://www.example.com/store/index.html?utm_source=x&a=1"),
        )
        .await
        .unwrap();
    let duplicate = q
        .push_unique(
            job(2, "http://example.com/store/?a=1&utm_medium=y"),
            key("http://example.com/store/?a=1&utm_medium=y"),
        )
        .await
        .unwrap();

    assert_eq!(inserted, QueueInsert::Inserted);
    assert_eq!(duplicate, QueueInsert::Duplicate);
    assert_eq!(q.pending_count().await.unwrap(), 1);
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_push_unique_reports_duplicate_for_canonical_url_aliases() {
    let tmp = tempfile::tempdir().unwrap();
    let q = crawlex::queue::sqlite::SqliteQueue::open(tmp.path().join("q.db")).unwrap();

    let inserted = q
        .push_unique(
            job(
                0,
                "https://www.example.com/store/index.html?utm_source=x&a=1",
            ),
            key("https://www.example.com/store/index.html?utm_source=x&a=1"),
        )
        .await
        .unwrap();
    let duplicate = q
        .push_unique(
            job(0, "http://example.com/store/?a=1&utm_medium=y"),
            key("http://example.com/store/?a=1&utm_medium=y"),
        )
        .await
        .unwrap();

    assert_eq!(inserted, QueueInsert::Inserted);
    assert_eq!(duplicate, QueueInsert::Duplicate);
    assert_eq!(q.pending_count().await.unwrap(), 1);
}
