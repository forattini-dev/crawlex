//! Unified artifacts storage — round-trip and metadata tests for the
//! SQLite + filesystem backends. Kept non-ignored so they run in the
//! default `cargo test` suite; no network, no Chromium spawn.

use std::sync::Arc;

use bytes::Bytes;
use crawlex::config::ContentStoreConfig;
use crawlex::discovery::assets::AssetKind;
use crawlex::storage::PageMetadata;
use crawlex::storage::{ArtifactKind, ArtifactMeta, ArtifactStorage};
use http::HeaderMap;
use url::Url;

fn sample_meta<'a>(
    url: &'a Url,
    session_id: &'a str,
    kind: ArtifactKind,
    step_id: Option<&'a str>,
    selector: Option<&'a str>,
    name: Option<&'a str>,
) -> ArtifactMeta<'a> {
    ArtifactMeta {
        url,
        final_url: None,
        session_id,
        kind,
        name,
        step_id,
        step_kind: step_id.map(|_| "screenshot"),
        selector,
        mime: None,
    }
}

#[test]
fn artifact_kind_wire_str_round_trip() {
    let variants = [
        ArtifactKind::ScreenshotViewport,
        ArtifactKind::ScreenshotFullPage,
        ArtifactKind::ScreenshotElement,
        ArtifactKind::SnapshotHtml,
        ArtifactKind::SnapshotDom,
        ArtifactKind::SnapshotPostJsHtml,
        ArtifactKind::SnapshotResponseBody,
        ArtifactKind::SnapshotState,
        ArtifactKind::SnapshotAxTree,
    ];
    for k in variants {
        let w = k.wire_str();
        assert_eq!(ArtifactKind::from_wire(w), Some(k), "round-trip {w}");
        // Kind implies a MIME and extension — keep them non-empty so
        // consumers don't have to special-case.
        assert!(!k.mime().is_empty());
        assert!(!k.extension().is_empty());
    }
}

#[tokio::test]
async fn sqlite_save_artifact_round_trips_and_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let sq: Arc<dyn ArtifactStorage> =
        Arc::new(crawlex::storage::sqlite::SqliteStorage::open(tmp.path().join("t.db")).unwrap());

    let u = Url::parse("https://example.test/p").unwrap();
    let sess = "sess_test_A";
    let png_bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0, 1, 2, 3, 4, 5];
    let html_bytes: Vec<u8> = b"<html>hi</html>".to_vec();

    sq.save_artifact(
        &sample_meta(
            &u,
            sess,
            ArtifactKind::ScreenshotFullPage,
            Some("s000"),
            None,
            Some("full_shot"),
        ),
        &png_bytes,
    )
    .await
    .unwrap();
    sq.save_artifact(
        &sample_meta(
            &u,
            sess,
            ArtifactKind::ScreenshotElement,
            Some("s001"),
            Some("#root"),
            Some("root_elem"),
        ),
        &png_bytes,
    )
    .await
    .unwrap();
    sq.save_artifact(
        &sample_meta(
            &u,
            sess,
            ArtifactKind::SnapshotHtml,
            Some("s002"),
            None,
            None,
        ),
        &html_bytes,
    )
    .await
    .unwrap();
    // Different session — should not surface under `sess` filter.
    sq.save_artifact(
        &sample_meta(
            &u,
            "sess_test_B",
            ArtifactKind::ScreenshotFullPage,
            None,
            None,
            None,
        ),
        &png_bytes,
    )
    .await
    .unwrap();

    // Give the writer thread a beat to flush the batch.
    for _ in 0..20 {
        let rows = sq.list_artifacts(Some(sess), None).await.unwrap();
        if rows.len() >= 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let rows = sq.list_artifacts(Some(sess), None).await.unwrap();
    assert_eq!(rows.len(), 3, "expected 3 rows for session A, got {rows:?}");
    let elem = rows
        .iter()
        .find(|r| matches!(r.kind, ArtifactKind::ScreenshotElement))
        .expect("element artifact");
    assert_eq!(elem.selector.as_deref(), Some("#root"));
    assert_eq!(elem.step_id.as_deref(), Some("s001"));
    assert_eq!(elem.name.as_deref(), Some("root_elem"));
    assert_eq!(elem.size as usize, png_bytes.len());
    assert_eq!(elem.sha256.len(), 64);

    let only_shots = sq
        .list_artifacts(Some(sess), Some(ArtifactKind::ScreenshotFullPage))
        .await
        .unwrap();
    assert_eq!(only_shots.len(), 1);
    assert!(matches!(
        only_shots[0].kind,
        ArtifactKind::ScreenshotFullPage
    ));

    let all_b = sq.list_artifacts(Some("sess_test_B"), None).await.unwrap();
    assert_eq!(all_b.len(), 1);
}

#[tokio::test]
async fn sqlite_content_store_writes_blobs_without_legacy_inline_columns_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("s.db");
    let sq = crawlex::storage::sqlite::SqliteStorage::open(&db_path).unwrap();

    let raw_url = Url::parse("https://sqlite.example/raw").unwrap();
    let rendered_url = Url::parse("https://sqlite.example/rendered").unwrap();
    let final_url = Url::parse("https://sqlite.example/rendered#done").unwrap();
    let headers = HeaderMap::new();
    let body = Bytes::from_static(b"raw response body");
    let html = "<html><body>rendered body</body></html>";
    let meta = PageMetadata {
        final_url,
        status: 200,
        bytes: html.len() as u64,
        rendered: true,
        kind: AssetKind::Page,
    };

    sq.save_raw(&raw_url, &headers, &body).await.unwrap();
    sq.save_rendered(&rendered_url, html, &meta).await.unwrap();

    let mut rows = Vec::new();
    for _ in 0..20 {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT url, body IS NULL, html IS NULL, body_sha256, html_sha256,
                        body_blob_path, html_blob_path FROM pages ORDER BY url",
            )
            .unwrap();
        rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        if rows.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(rows.len(), 2, "expected raw + rendered page rows");
    let blob_root = tmp.path().join("s.db.blobs");
    for (url, body_is_null, html_is_null, body_sha, html_sha, body_blob, html_blob) in rows {
        if url.ends_with("/raw") {
            assert_eq!(body_is_null, 1);
            assert!(body_sha.as_deref().is_some_and(|s| s.len() == 64));
            let rel = body_blob.expect("raw blob path");
            assert!(blob_root.join(rel).exists());
        } else {
            assert_eq!(html_is_null, 1);
            assert!(html_sha.as_deref().is_some_and(|s| s.len() == 64));
            let rel = html_blob.expect("html blob path");
            assert!(blob_root.join(rel).exists());
        }
    }
}

#[tokio::test]
async fn sqlite_content_store_can_keep_legacy_inline_columns() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("legacy.db");
    let cfg = ContentStoreConfig {
        inline_legacy_columns: true,
        ..Default::default()
    };
    let sq =
        crawlex::storage::sqlite::SqliteStorage::open_with_content_store(&db_path, &cfg).unwrap();

    let url = Url::parse("https://sqlite.example/legacy").unwrap();
    let headers = HeaderMap::new();
    let body = Bytes::from_static(b"legacy body");
    sq.save_raw(&url, &headers, &body).await.unwrap();

    for _ in 0..20 {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let body_len: Option<i64> = conn
            .query_row(
                "SELECT length(body) FROM pages WHERE url=?1",
                [&url.as_str()],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        if body_len == Some(body.len() as i64) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("legacy inline body was not persisted");
}

#[tokio::test]
async fn filesystem_save_artifact_writes_bytes_and_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let fs: Arc<dyn ArtifactStorage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());

    let u = Url::parse("https://fs.example/p").unwrap();
    let sess = "sess_fs_X";
    let payload = b"AXTREE DUMP\n".to_vec();

    fs.save_artifact(
        &sample_meta(
            &u,
            sess,
            ArtifactKind::SnapshotAxTree,
            Some("s003"),
            None,
            Some("ax_after_login"),
        ),
        &payload,
    )
    .await
    .unwrap();

    // Look into the artifacts tree: exactly one bytes file + one sidecar.
    let dir = tmp.path().join("artifacts").join(sess);
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert_eq!(
        entries.len(),
        2,
        "expected 2 files (bytes + sidecar), got {entries:?}"
    );
    let mut had_bytes = false;
    let mut had_meta = false;
    for e in &entries {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".txt") {
            let bytes = std::fs::read(e.path()).unwrap();
            assert_eq!(bytes, payload);
            had_bytes = true;
        } else if name.ends_with(".meta.json") {
            let raw = std::fs::read_to_string(e.path()).unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(v["kind"], "snapshot.ax_tree");
            assert_eq!(v["session_id"], sess);
            assert_eq!(v["step_id"], "s003");
            assert_eq!(v["name"], "ax_after_login");
            assert_eq!(v["mime"], "text/plain");
            assert_eq!(v["size"], payload.len());
            assert_eq!(v["sha256"].as_str().unwrap().len(), 64);
            had_meta = true;
        }
    }
    assert!(had_bytes && had_meta);

    // list_artifacts scans sidecars — should find the one we wrote and
    // should respect the kind filter.
    let rows = fs.list_artifacts(Some(sess), None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].kind, ArtifactKind::SnapshotAxTree));
    assert_eq!(rows[0].name.as_deref(), Some("ax_after_login"));

    let no_match = fs
        .list_artifacts(Some(sess), Some(ArtifactKind::ScreenshotFullPage))
        .await
        .unwrap();
    assert!(no_match.is_empty());
}

#[tokio::test]
async fn filesystem_raw_and_rendered_writes_metadata_and_payloads() {
    let tmp = tempfile::tempdir().unwrap();
    let fs = Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());

    let raw_url = Url::parse("https://fs.example/raw").unwrap();
    let rendered_url = Url::parse("https://fs.example/rendered").unwrap();
    let final_url = Url::parse("https://fs.example/rendered#done").unwrap();
    let body = Bytes::from_static(b"encoded body");
    let html = "<html><body>rendered</body></html>";
    let headers = HeaderMap::new();
    let meta = PageMetadata {
        final_url,
        status: 200,
        bytes: html.len() as u64,
        rendered: true,
        kind: AssetKind::Page,
    };

    let (raw_res, rendered_res) = tokio::join!(
        fs.save_raw(&raw_url, &headers, &body),
        fs.save_rendered(&rendered_url, html, &meta)
    );
    raw_res.unwrap();
    rendered_res.unwrap();

    let metadata = std::fs::read_to_string(tmp.path().join("metadata.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = metadata
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2, "metadata rows: {metadata}");
    assert!(rows.iter().any(|v| v["kind"] == "raw"));
    assert!(rows.iter().any(|v| v["kind"] == "rendered"));
    for row in rows {
        let rel = row["rel_path"].as_str().unwrap();
        assert!(
            rel.starts_with("blobs/"),
            "expected blob rel_path, got {rel}"
        );
        assert!(tmp.path().join(rel).exists(), "missing payload {rel}");
        assert_eq!(row["sha256"].as_str().unwrap().len(), 64);
    }
}

#[tokio::test]
async fn filesystem_raw_response_metadata_records_final_status_and_truncation() {
    let tmp = tempfile::tempdir().unwrap();
    let fs = Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());

    let url = Url::parse("https://fs.example/raw").unwrap();
    let final_url = Url::parse("https://cdn.fs.example/raw").unwrap();
    let body = Bytes::from_static(b"partial body");
    let headers = HeaderMap::new();
    fs.save_raw_response(&url, &final_url, 206, &headers, &body, true)
        .await
        .unwrap();

    let metadata = std::fs::read_to_string(tmp.path().join("metadata.jsonl")).unwrap();
    let row: serde_json::Value = serde_json::from_str(metadata.trim()).unwrap();
    assert_eq!(row["url"], url.as_str());
    assert_eq!(row["final_url"], final_url.as_str());
    assert_eq!(row["status"], 206);
    assert_eq!(row["truncated"], true);
}

#[tokio::test]
async fn default_save_screenshot_lands_in_artifacts_table() {
    // Default trait impl of `save_screenshot` delegates to `save_artifact`
    // — verify a legacy caller to `save_screenshot` surfaces through
    // `list_artifacts`. Exercises the wrapper that keeps old callsites
    // working without churning every call site in the tree.
    let tmp = tempfile::tempdir().unwrap();
    let fs: Arc<dyn ArtifactStorage> =
        Arc::new(crawlex::storage::filesystem::FilesystemStorage::open(tmp.path()).unwrap());
    let u = Url::parse("https://legacy.example/x").unwrap();
    let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    fs.save_screenshot(&u, &png).await.unwrap();
    let rows = fs.list_artifacts(None, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].kind, ArtifactKind::ScreenshotFullPage));
    assert_eq!(rows[0].session_id, "legacy:legacy.example");
}
