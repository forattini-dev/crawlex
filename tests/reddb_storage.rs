use bytes::Bytes;
use crawlex::discovery::tech_fingerprint::{
    TechEvidence, TechFingerprintReport, TechMatch, TechSource,
};
use crawlex::storage::{ArtifactStorage, IntelStorage, StateStorage};
use http::HeaderMap;
use reddb_client::Reddb;
use url::Url;

#[cfg(feature = "reddb-embedded")]
#[tokio::test]
async fn reddb_embedded_storage_persists_raw_and_session_state_across_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("crawlex.redb");
    let url: Url = "https://example.com/page".parse().unwrap();

    {
        let storage = crawlex::storage::reddb::ReddbStorage::open(&path)
            .await
            .unwrap();
        storage
            .save_raw(
                &url,
                &HeaderMap::new(),
                &Bytes::from_static(b"hello crawlex"),
            )
            .await
            .unwrap();
        storage
            .save_state("session-a", r#"{"cookies":[]}"#)
            .await
            .unwrap();
    }

    let storage = crawlex::storage::reddb::ReddbStorage::open(&path)
        .await
        .unwrap();
    assert_eq!(
        storage.load_state("session-a").await.unwrap().as_deref(),
        Some(r#"{"cookies":[]}"#)
    );
}

#[cfg(feature = "reddb-embedded")]
#[tokio::test]
async fn reddb_embedded_storage_persists_technology_fingerprints_and_host_rollups() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("crawlex.redb");
    let report = TechFingerprintReport {
        url: "https://example.com/login".to_string(),
        final_url: "https://example.com/login".to_string(),
        host: "example.com".to_string(),
        generated_at: 1_700_000_000,
        technologies: vec![TechMatch {
            slug: "cloudflare".to_string(),
            name: "Cloudflare".to_string(),
            category: "cdn".to_string(),
            confidence: 95,
            evidence: vec![TechEvidence {
                source: TechSource::Header,
                key: "server".to_string(),
                value: "cloudflare".to_string(),
                confidence_delta: 95,
            }],
        }],
    };

    {
        let storage = crawlex::storage::reddb::ReddbStorage::open(&path)
            .await
            .unwrap();
        storage.save_tech_fingerprint(&report).await.unwrap();
    }

    let db = Reddb::connect(&format!("file://{}", path.display()))
        .await
        .unwrap();
    let raw = db
        .query("SELECT host, url, report_json FROM crawlex_tech_fingerprints WHERE host = 'example.com'")
        .await
        .unwrap();
    assert_eq!(raw.rows.len(), 1);
    assert!(
        raw.rows[0]
            .iter()
            .any(|(column, value)| column == "report_json"
                && value.to_string().contains("cloudflare"))
    );

    let rollup = db
        .query("SELECT host, slug, name, category, seen_count, confidence_max FROM crawlex_host_tech WHERE host = 'example.com' AND slug = 'cloudflare'")
        .await
        .unwrap();
    assert_eq!(rollup.rows.len(), 1);
    assert!(rollup.rows[0]
        .iter()
        .any(|(column, value)| column == "seen_count" && value.to_string() == "1"));
    assert!(rollup.rows[0]
        .iter()
        .any(|(column, value)| column == "confidence_max" && value.to_string() == "95"));
}
