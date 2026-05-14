//! Slice 1 — canonical status taxonomy round-trip.
//!
//! Seeds one `pages` row per `Status` variant via the writer thread,
//! waits for the batch to flush, then exercises the read path used by
//! the SDK results endpoint (`crawlex pages list`) and asserts the
//! status-filter projection.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crawlex::discovery::assets::AssetKind;
use crawlex::storage::sqlite::{list_pages_with_status_blocking, SqliteStorage};
use crawlex::storage::ArtifactStorage;
use crawlex::storage::PageMetadata;
use crawlex::Status;
use url::Url;

#[test]
fn status_enum_roundtrip_via_wire() {
    for s in Status::all() {
        let wire = s.as_str();
        assert_eq!(Status::from_str(wire).unwrap(), *s, "wire `{wire}`");
    }
}

#[test]
fn status_enum_serde_snake_case() {
    let line = serde_json::to_string(&Status::Cancelled).unwrap();
    assert_eq!(line, "\"cancelled\"");
}

#[tokio::test]
async fn pages_list_filters_by_canonical_status() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("status.db");
    let store = Arc::new(SqliteStorage::open(&db_path).unwrap());

    // Seed: one row per status variant.
    for (i, status) in Status::all().iter().enumerate() {
        let url = Url::parse(&format!("https://example.test/p/{i}")).unwrap();
        let meta = PageMetadata {
            final_url: url.clone(),
            status: 200,
            bytes: 12,
            rendered: false,
            kind: AssetKind::Page,
        };
        store.save_rendered(&url, "<html/>", &meta).await.unwrap();
        store
            .set_page_crawl_status(url.to_string(), *status)
            .await
            .unwrap();
    }

    // Wait for the writer-thread batch to flush.
    let expected = Status::all().len();
    let mut all = Vec::new();
    for _ in 0..40 {
        let path = db_path.clone();
        all = tokio::task::spawn_blocking(move || {
            list_pages_with_status_blocking(&path, None, 0).unwrap()
        })
        .await
        .unwrap();
        let labeled = all.iter().filter(|r| r.crawl_status.is_some()).count();
        if labeled >= expected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        all.len() >= expected,
        "expected at least {expected} rows, got {} ({all:?})",
        all.len()
    );

    // Per-status filter should narrow to exactly one row, with the
    // wire value preserved.
    for s in Status::all() {
        let path = db_path.clone();
        let filter = *s;
        let rows = tokio::task::spawn_blocking(move || {
            list_pages_with_status_blocking(&path, Some(filter), 0).unwrap()
        })
        .await
        .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "filter {} matched {} rows",
            s.as_str(),
            rows.len()
        );
        assert_eq!(rows[0].crawl_status.as_deref(), Some(s.as_str()));
    }
}
