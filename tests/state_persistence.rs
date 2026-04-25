//! Phase 3 setup: state snapshot round-trip across both persistent
//! storage backends. The Fase 3 browser-session engine will write
//! `{cookies, localStorage, ...}` as opaque JSON; here we check the
//! storage layer round-trips correctly, reject path-traversal attempts,
//! and survive an "unknown session_id" lookup.

use crawlex::storage::{filesystem::FilesystemStorage, Storage};

#[cfg(feature = "sqlite")]
use crawlex::storage::sqlite::SqliteStorage;

const SAMPLE: &str =
    r#"{"cookies":[{"name":"sid","value":"abc","domain":"example.com"}],"localStorage":{"k":"v"}}"#;

#[tokio::test]
async fn filesystem_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let fs = FilesystemStorage::open(tmp.path()).unwrap();

    fs.save_state("sess_abc123", SAMPLE).await.unwrap();
    let got = fs.load_state("sess_abc123").await.unwrap();
    assert_eq!(got.as_deref(), Some(SAMPLE));

    // Overwrite: second save wins.
    let newer = r#"{"cookies":[],"localStorage":{}}"#;
    fs.save_state("sess_abc123", newer).await.unwrap();
    let got = fs.load_state("sess_abc123").await.unwrap();
    assert_eq!(got.as_deref(), Some(newer));

    // Unknown session → None, not error.
    let missing = fs.load_state("sess_unknown").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn filesystem_rejects_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let fs = FilesystemStorage::open(tmp.path()).unwrap();

    // Slashes, backslashes, `..` — any of these could escape the state
    // dir if the backend naively joined them.
    for bad in ["../escape", "a/b", r"a\b", ".."] {
        let e1 = fs.save_state(bad, SAMPLE).await;
        assert!(e1.is_err(), "save_state should reject {bad:?}");
        let e2 = fs.load_state(bad).await;
        assert!(e2.is_err(), "load_state should reject {bad:?}");
    }
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("s.db").to_string_lossy().to_string();
    let sq = SqliteStorage::open(&path).unwrap();

    sq.save_state("sess_a", SAMPLE).await.unwrap();
    // Writer-thread is async; give it a tick to commit before the
    // read-only reader tries to pick it up.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let got = sq.load_state("sess_a").await.unwrap();
    assert_eq!(got.as_deref(), Some(SAMPLE));

    // Overwrite.
    let newer = r#"{}"#;
    sq.save_state("sess_a", newer).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let got = sq.load_state("sess_a").await.unwrap();
    assert_eq!(got.as_deref(), Some(newer));

    // Unknown.
    let missing = sq.load_state("sess_nope").await.unwrap();
    assert!(missing.is_none());
}
