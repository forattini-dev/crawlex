//! Pre-network freshness skip — seeds a `pages` row through the real
//! SQLite storage write path, then asserts `evaluate_freshness` returns
//! the exact `Fresh` outcome the crawler uses to short-circuit the fetch.

use bytes::Bytes;
use crawlex::cache_validator::{evaluate_freshness, CacheValidationStatus};
use crawlex::storage::ArtifactStorage;
use http::HeaderMap;
use url::Url;

async fn seed(url: &str, last_modified: Option<&str>) -> (tempfile::TempDir, crawlex::storage::PageCacheMetadata) {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("freshness.db");
    let sq = crawlex::storage::sqlite::SqliteStorage::open(&db_path).unwrap();
    let parsed = Url::parse(url).unwrap();
    let mut headers = HeaderMap::new();
    if let Some(lm) = last_modified {
        headers.insert("last-modified", lm.parse().unwrap());
    }
    sq.save_raw_response(&parsed, &parsed, 200, &headers, &Bytes::from_static(b"<html></html>"), false)
        .await
        .unwrap();
    let mut meta = None;
    for _ in 0..50 {
        if let Some(m) = sq.page_cache_metadata(&parsed).await.unwrap() {
            meta = Some(m);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    (tmp, meta.expect("seeded row should be visible"))
}

#[tokio::test]
async fn cache_max_age_skips_fresh_row() {
    let (_tmp, meta) = seed("https://example.test/recent", None).await;
    let outcome = evaluate_freshness(&meta, Some(3600), None);
    assert_eq!(outcome.status, CacheValidationStatus::Fresh);
    assert_eq!(outcome.reason, "fresh-by-max-age");
}

#[tokio::test]
async fn modified_since_skips_old_last_modified() {
    let (_tmp, meta) = seed(
        "https://example.test/stale-lm",
        Some("Sun, 06 Nov 1994 08:49:37 GMT"),
    )
    .await;
    let outcome = evaluate_freshness(&meta, None, Some(1_700_000_000));
    assert_eq!(outcome.status, CacheValidationStatus::Fresh);
    assert_eq!(outcome.reason, "unmodified-since");
}

#[tokio::test]
async fn defaults_do_not_short_circuit() {
    let (_tmp, meta) = seed("https://example.test/no-knobs", None).await;
    let outcome = evaluate_freshness(&meta, None, None);
    assert_eq!(outcome.status, CacheValidationStatus::Unknown);
}
