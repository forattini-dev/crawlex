//! Slice 7 — Job TTL: terminal-reason write + reaper sweep.
//!
//! Storage-layer tracer for the run-lifecycle bounds. The crawler-side
//! watchdog/dispatcher wiring lands in a follow-up; this slice asserts:
//!
//! - `record_job_terminal_blocking` writes `terminal_reason` and (when
//!   retention is configured) stamps `result_expires_at` on both the
//!   `crawl_stats` row and the matching `pages` row.
//! - `reap_expired_blocking` deletes only rows past the deadline,
//!   leaving NULL-TTL legacy rows and fresh rows alone.
//! - Terminal-reason enum wire strings match the canonical taxonomy.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crawlex::discovery::assets::AssetKind;
use crawlex::storage::sqlite::{
    reap_expired_blocking, record_job_terminal_blocking, ReapStats, SqliteStorage,
};
use crawlex::storage::{ArtifactStorage, PageMetadata, TelemetryStorage};
use crawlex::TerminalReason;
use rusqlite::Connection;
use url::Url;

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn seed_page(store: &SqliteStorage, url: &Url) {
    let meta = PageMetadata {
        final_url: url.clone(),
        status: 200,
        bytes: 4,
        rendered: false,
        kind: AssetKind::Page,
    };
    store.save_rendered(url, "<html/>", &meta).await.unwrap();
}

async fn seed_crawl_stats(store: &SqliteStorage, crawl_id: u64, url: &Url) {
    let mut stats = crawlex::crawl_stats::CrawlStats::new(crawl_id, url.clone());
    stats.success = true;
    store.record_crawl_stats(&stats).await.unwrap();
}

async fn wait_for_pages_row(path: &std::path::Path, url: &str) {
    for _ in 0..40 {
        let conn = Connection::open(path).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pages WHERE url = ?1",
                rusqlite::params![url],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if n > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("pages row never flushed for {url}");
}

async fn wait_for_crawl_stats_row(path: &std::path::Path, crawl_id: i64) {
    for _ in 0..40 {
        let conn = Connection::open(path).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM crawl_stats WHERE crawl_id = ?1",
                rusqlite::params![crawl_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if n > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("crawl_stats row never flushed for crawl_id={crawl_id}");
}

#[test]
fn terminal_reason_wire_strings_match_taxonomy() {
    // Per the slice 1 taxonomy, the wire strings for the three lifecycle
    // bounds this slice writes must be stable — log consumers grep on them.
    assert_eq!(
        TerminalReason::CancelledDueToTimeout.as_str(),
        "cancelled_due_to_timeout"
    );
    assert_eq!(
        TerminalReason::CancelledDueToLimits.as_str(),
        "cancelled_due_to_limits"
    );
    assert_eq!(
        TerminalReason::CancelledByUser.as_str(),
        "cancelled_by_user"
    );
    // Round-trip every variant so a typo in `as_str` / `from_str` breaks
    // the build before it breaks an operator's tail-grep.
    for r in TerminalReason::all() {
        assert_eq!(TerminalReason::from_str(r.as_str()).unwrap(), *r);
    }
}

#[tokio::test]
async fn record_job_terminal_writes_reason_and_expiry() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("ttl.db");
    let store = Arc::new(SqliteStorage::open(&db).unwrap());

    let url = Url::parse("https://example.test/p1").unwrap();
    seed_page(&store, &url).await;
    seed_crawl_stats(&store, 42, &url).await;

    wait_for_pages_row(&db, url.as_str()).await;
    wait_for_crawl_stats_row(&db, 42).await;

    let now = now_secs();
    let path = db.clone();
    tokio::task::spawn_blocking(move || {
        record_job_terminal_blocking(
            &path,
            42,
            TerminalReason::CancelledDueToTimeout,
            Some(60),
            now,
        )
        .unwrap();
    })
    .await
    .unwrap();

    let conn = Connection::open(&db).unwrap();
    let (reason, stats_exp): (String, i64) = conn
        .query_row(
            "SELECT terminal_reason, result_expires_at FROM crawl_stats WHERE crawl_id = 42",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(reason, "cancelled_due_to_timeout");
    assert_eq!(stats_exp, now + 60);

    let page_exp: i64 = conn
        .query_row(
            "SELECT result_expires_at FROM pages WHERE url = ?1",
            rusqlite::params![url.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(page_exp, now + 60);
}

#[tokio::test]
async fn record_job_terminal_without_retention_leaves_expiry_null() {
    // `result_retention_secs = None` means "don't reap" — the terminal
    // reason still needs to land, but TTL columns must stay NULL so the
    // reaper leaves the rows alone forever (legacy behaviour).
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("ttl_noret.db");
    let store = Arc::new(SqliteStorage::open(&db).unwrap());

    let url = Url::parse("https://example.test/p2").unwrap();
    seed_page(&store, &url).await;
    seed_crawl_stats(&store, 7, &url).await;
    wait_for_crawl_stats_row(&db, 7).await;
    wait_for_pages_row(&db, url.as_str()).await;

    let path = db.clone();
    tokio::task::spawn_blocking(move || {
        record_job_terminal_blocking(
            &path,
            7,
            TerminalReason::CancelledByUser,
            None,
            now_secs(),
        )
        .unwrap();
    })
    .await
    .unwrap();

    let conn = Connection::open(&db).unwrap();
    let (reason, stats_exp): (String, Option<i64>) = conn
        .query_row(
            "SELECT terminal_reason, result_expires_at FROM crawl_stats WHERE crawl_id = 7",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(reason, "cancelled_by_user");
    assert!(stats_exp.is_none(), "expected NULL expires_at, got {stats_exp:?}");
    let page_exp: Option<i64> = conn
        .query_row(
            "SELECT result_expires_at FROM pages WHERE url = ?1",
            rusqlite::params![url.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(page_exp.is_none());
}

#[tokio::test]
async fn reaper_deletes_only_expired_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("reap.db");
    let store = Arc::new(SqliteStorage::open(&db).unwrap());

    let expired = Url::parse("https://example.test/expired").unwrap();
    let fresh = Url::parse("https://example.test/fresh").unwrap();
    let legacy = Url::parse("https://example.test/legacy").unwrap();

    seed_page(&store, &expired).await;
    seed_page(&store, &fresh).await;
    seed_page(&store, &legacy).await;
    seed_crawl_stats(&store, 1, &expired).await;
    seed_crawl_stats(&store, 2, &fresh).await;
    seed_crawl_stats(&store, 3, &legacy).await;
    for u in [&expired, &fresh, &legacy] {
        wait_for_pages_row(&db, u.as_str()).await;
    }
    for cid in [1, 2, 3] {
        wait_for_crawl_stats_row(&db, cid).await;
    }

    let now = now_secs();
    // Hand-stamp TTLs: 1 is in the past, 2 in the future, 3 stays NULL.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE pages SET result_expires_at = ?1 WHERE url = ?2",
            rusqlite::params![now - 10, expired.as_str()],
        )
        .unwrap();
        conn.execute(
            "UPDATE pages SET result_expires_at = ?1 WHERE url = ?2",
            rusqlite::params![now + 3600, fresh.as_str()],
        )
        .unwrap();
        conn.execute(
            "UPDATE crawl_stats SET result_expires_at = ?1 WHERE crawl_id = 1",
            rusqlite::params![now - 10],
        )
        .unwrap();
        conn.execute(
            "UPDATE crawl_stats SET result_expires_at = ?1 WHERE crawl_id = 2",
            rusqlite::params![now + 3600],
        )
        .unwrap();
    }

    let db_clone = db.clone();
    let stats: ReapStats =
        tokio::task::spawn_blocking(move || reap_expired_blocking(&db_clone, now).unwrap())
            .await
            .unwrap();
    assert_eq!(stats.pages_deleted, 1);
    assert_eq!(stats.crawl_stats_deleted, 1);

    let conn = Connection::open(&db).unwrap();
    // Expired row gone.
    let n_expired: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pages WHERE url = ?1",
            rusqlite::params![expired.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_expired, 0);
    // Fresh + legacy rows survive.
    for u in [&fresh, &legacy] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pages WHERE url = ?1",
                rusqlite::params![u.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "expected {u} to survive reaper");
    }
    let n_stats: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM crawl_stats WHERE crawl_id IN (2, 3)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_stats, 2);
}

#[tokio::test]
async fn reaper_on_fresh_db_is_a_noop() {
    // Defaults preserve today's behaviour: no TTL columns populated, so
    // the reaper sweeps zero rows.
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("noop.db");
    let _store = SqliteStorage::open(&db).unwrap();
    let db_clone = db.clone();
    let stats: ReapStats =
        tokio::task::spawn_blocking(move || reap_expired_blocking(&db_clone, now_secs()).unwrap())
            .await
            .unwrap();
    assert_eq!(stats, ReapStats::default());
}
