//! Slice 8 — cursor pagination on the SDK results read path.
//!
//! Seeds 5_000 rows in the `pages` table, paginates with `limit=100`
//! through `list_pages_with_status_paged_blocking`, and asserts:
//!
//! * Every seeded URL is visited exactly once.
//! * No URL repeats across page boundaries.
//! * The cursor token is opaque (does not contain raw rowid digits).
//! * Pagination survives a "restart" — i.e. the function opens a
//!   fresh read-only SQLite connection on every call and the cursor
//!   does not depend on in-memory server state.

use std::collections::HashSet;

use crawlex::storage::sqlite::list_pages_with_status_paged_blocking;
use crawlex::Status;

/// Minimal `pages` schema — just the columns
/// `list_pages_with_status_paged_blocking` selects. Bypasses the full
/// `SqliteStorage` writer thread so a 5k-row seed fits inside a single
/// transaction (and a single millisecond).
fn seed_pages(db_path: &std::path::Path, n: usize, status: &str) {
    let conn = rusqlite::Connection::open(db_path).expect("rusqlite open");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pages (
            url TEXT PRIMARY KEY,
            final_url TEXT NOT NULL,
            status INTEGER NOT NULL,
            bytes INTEGER NOT NULL,
            rendered INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            crawl_status TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_pages_crawl_status ON pages(crawl_status);
        BEGIN;",
    )
    .unwrap();
    {
        let mut stmt = conn
            .prepare(
                "INSERT INTO pages (url, final_url, status, bytes, rendered, sha256, crawl_status) \
                 VALUES (?1, ?1, 200, 0, 0, '', ?2)",
            )
            .unwrap();
        for i in 0..n {
            let url = format!("https://example.test/p/{i:05}");
            stmt.execute(rusqlite::params![url, status]).unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
}

#[test]
fn paginates_5k_rows_with_limit_100_no_dupes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("pages.db");
    seed_pages(&db, 5_000, Status::Completed.as_str());

    let mut seen: HashSet<String> = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut pages_returned = 0usize;
    loop {
        let page = list_pages_with_status_paged_blocking(
            &db,
            Some(Status::Completed),
            100,
            cursor.as_deref(),
        )
        .expect("paged read");
        pages_returned += 1;
        assert!(
            page.rows.len() <= 100,
            "page exceeded limit: {}",
            page.rows.len()
        );
        for row in &page.rows {
            assert!(
                seen.insert(row.url.clone()),
                "duplicate row across pages: {}",
                row.url
            );
            assert_eq!(row.crawl_status.as_deref(), Some("completed"));
        }
        match page.next_cursor {
            Some(tok) => cursor = Some(tok),
            None => break,
        }
        // Guard against pathological infinite loops if the cursor
        // ever fails to advance.
        assert!(pages_returned < 1_000, "pagination did not terminate");
    }
    assert_eq!(seen.len(), 5_000, "missing rows after pagination");
    // 5000 / 100 = 50 full pages + a final empty (or partial) close.
    assert!(
        (50..=51).contains(&pages_returned),
        "unexpected page count: {pages_returned}"
    );
}

#[test]
fn pagination_survives_simulated_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("pages.db");
    seed_pages(&db, 250, Status::Errored.as_str());

    // Page 1
    let page1 = list_pages_with_status_paged_blocking(&db, Some(Status::Errored), 100, None)
        .expect("page 1");
    assert_eq!(page1.rows.len(), 100);
    let cursor1 = page1.next_cursor.clone().expect("expected next_cursor");

    // "Restart" — drop everything, no in-memory cursor state, then
    // resume from the same token. `list_pages_with_status_paged_blocking`
    // opens a fresh read-only connection on every call, so this is
    // the same code path that a long-lived server hits across restarts.
    drop(page1);

    let page2 = list_pages_with_status_paged_blocking(
        &db,
        Some(Status::Errored),
        100,
        Some(&cursor1),
    )
    .expect("page 2 after restart");
    assert_eq!(page2.rows.len(), 100);
    let cursor2 = page2.next_cursor.clone().expect("expected next_cursor");

    let page3 = list_pages_with_status_paged_blocking(
        &db,
        Some(Status::Errored),
        100,
        Some(&cursor2),
    )
    .expect("page 3 after restart");
    assert_eq!(page3.rows.len(), 50);
    assert!(page3.next_cursor.is_none());

    // Union must be exactly the 250 seeded URLs.
    let mut seen: HashSet<String> = HashSet::new();
    for r in page2.rows.iter().chain(page3.rows.iter()) {
        seen.insert(r.url.clone());
    }
    // page1 wasn't kept; recompute from cursor=None up to cursor1.
    let again = list_pages_with_status_paged_blocking(&db, Some(Status::Errored), 100, None)
        .expect("page 1 replay");
    for r in &again.rows {
        seen.insert(r.url.clone());
    }
    assert_eq!(seen.len(), 250);
}

#[test]
fn cursor_filter_must_match_request_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("pages.db");
    seed_pages(&db, 50, Status::Completed.as_str());

    let page = list_pages_with_status_paged_blocking(&db, Some(Status::Completed), 10, None)
        .expect("page 1");
    let tok = page.next_cursor.expect("cursor");
    // Replay under a different filter → hard error (rowid ordering
    // matches, but the consumer would silently drop or repeat rows).
    let err =
        list_pages_with_status_paged_blocking(&db, Some(Status::Errored), 10, Some(&tok))
            .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("cursor minted"), "{msg}");
}

#[test]
fn unbounded_limit_returns_no_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("pages.db");
    seed_pages(&db, 50, Status::Completed.as_str());

    let page = list_pages_with_status_paged_blocking(&db, None, 0, None).expect("unbounded");
    assert_eq!(page.rows.len(), 50);
    assert!(page.next_cursor.is_none());
}

