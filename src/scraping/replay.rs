//! Dev-replay cache for the spider runtime.
//!
//! Slice 19. Two pluggable backends sit behind a `Replay` trait:
//!
//! * [`DirReplay`] — writes a pair of files per unique request to a
//!   directory: `<hash>.json` (metadata: status, final_url, headers) and
//!   `<hash>.body` (raw response bytes). Keyed by the SHA-256 hash of
//!   `(method, url, body)`.
//! * [`ReddbReplay`] — single JSON file per spider, `<dir>/<spider_id>.replay.json`,
//!   storing every recorded response under its request hash. Lives in
//!   the same data directory as the adaptive fingerprint store from
//!   slice 13 but in its own namespace (file/extension), so the two
//!   never collide.
//!
//! A [`ReplayingFetcher`] wraps any inner [`Fetcher`] and short-circuits
//! the network on cache hits. Cache misses fall through, then the
//! response is recorded for next time. This is intentionally
//! transparent — the spider runtime keeps calling `Fetcher::fetch` and
//! is none the wiser.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::request::Request;
use super::spider::{FetchError, Fetcher, Response};

/// Persisted shape of one recorded response. `body` is held as raw bytes
/// in-memory and serialised as base64 inside JSON so the file stays
/// printable for diffing.
#[derive(Debug, Clone)]
pub struct RecordedResponse {
    pub status: u16,
    pub final_url: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// On-disk metadata (everything except the raw body bytes — those land
/// in a sibling `<hash>.body` file for `DirReplay`, or are base64'd
/// alongside for `ReddbReplay`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OnDiskMeta {
    method: String,
    url: String,
    status: u16,
    final_url: String,
    headers: HashMap<String, String>,
}

/// Two-key cache contract. Implementations decide whether `record`
/// flushes to disk synchronously (both built-in backends do).
pub trait Replay: Send + Sync {
    fn lookup(&self, req: &Request, body: &[u8]) -> io::Result<Option<RecordedResponse>>;
    fn record(&self, req: &Request, body: &[u8], resp: &Response) -> io::Result<()>;
}

/// Stable cache key. The body bytes are folded into the hash so that
/// POST bodies with different payloads do not collide. Method and url
/// are separated by a unit-separator byte to keep the digest unambiguous.
pub fn cache_key(method: &str, url: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(method.as_bytes());
    h.update([0x1f]);
    h.update(url.as_bytes());
    h.update([0x1f]);
    h.update(body);
    hex::encode(h.finalize())
}

// ── Directory backend ────────────────────────────────────────────────

/// On-disk replay cache: one pair of files per recorded request.
pub struct DirReplay {
    root: PathBuf,
}

impl DirReplay {
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.json"))
    }

    fn body_path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.body"))
    }
}

impl Replay for DirReplay {
    fn lookup(&self, req: &Request, body: &[u8]) -> io::Result<Option<RecordedResponse>> {
        let key = cache_key(&req.method, &req.url, body);
        let mp = self.meta_path(&key);
        if !mp.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&mp)?;
        let meta: OnDiskMeta = serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let body_bytes = fs::read(self.body_path(&key)).unwrap_or_default();
        Ok(Some(RecordedResponse {
            status: meta.status,
            final_url: meta.final_url,
            headers: meta.headers,
            body: body_bytes,
        }))
    }

    fn record(&self, req: &Request, body: &[u8], resp: &Response) -> io::Result<()> {
        let key = cache_key(&req.method, &req.url, body);
        let meta = OnDiskMeta {
            method: req.method.clone(),
            url: req.url.clone(),
            status: resp.status,
            final_url: resp.final_url.clone(),
            headers: resp.headers.clone(),
        };
        let json = serde_json::to_vec_pretty(&meta)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(&self.meta_path(&key), &json)?;
        write_atomic(&self.body_path(&key), &resp.body)?;
        Ok(())
    }
}

// ── Reddb (per-spider JSON store) backend ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReddbEntry {
    method: String,
    url: String,
    status: u16,
    final_url: String,
    headers: HashMap<String, String>,
    /// base64 of the body bytes.
    body_b64: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ReddbOnDisk {
    entries: HashMap<String, ReddbEntry>,
}

/// Per-spider single-file replay cache. Lives next to (but separate
/// from) the adaptive fingerprint store — same directory, same spider
/// id, different file extension.
pub struct ReddbReplay {
    path: PathBuf,
    inner: RwLock<ReddbOnDisk>,
}

impl ReddbReplay {
    pub fn open(dir: impl AsRef<Path>, spider_id: &str) -> io::Result<Self> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.replay.json", sanitise(spider_id)));
        let inner = if path.exists() {
            let bytes = fs::read(&path)?;
            if bytes.is_empty() {
                ReddbOnDisk::default()
            } else {
                serde_json::from_slice::<ReddbOnDisk>(&bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            }
        } else {
            ReddbOnDisk::default()
        };
        Ok(Self { path, inner: RwLock::new(inner) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Replay for ReddbReplay {
    fn lookup(&self, req: &Request, body: &[u8]) -> io::Result<Option<RecordedResponse>> {
        let key = cache_key(&req.method, &req.url, body);
        let g = self.inner.read();
        let Some(e) = g.entries.get(&key) else {
            return Ok(None);
        };
        let body = base64_decode(&e.body_b64)?;
        Ok(Some(RecordedResponse {
            status: e.status,
            final_url: e.final_url.clone(),
            headers: e.headers.clone(),
            body,
        }))
    }

    fn record(&self, req: &Request, body: &[u8], resp: &Response) -> io::Result<()> {
        let key = cache_key(&req.method, &req.url, body);
        let entry = ReddbEntry {
            method: req.method.clone(),
            url: req.url.clone(),
            status: resp.status,
            final_url: resp.final_url.clone(),
            headers: resp.headers.clone(),
            body_b64: base64_encode(&resp.body),
        };
        let mut g = self.inner.write();
        g.entries.insert(key, entry);
        let bytes = serde_json::to_vec(&*g)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(&self.path, &bytes)
    }
}

// ── Fetcher wrapper ──────────────────────────────────────────────────

/// Wraps an inner `Fetcher`, consulting `Replay` first. On miss it
/// delegates to the inner fetcher and records the response before
/// returning it. Slice 19 only supports requests with empty bodies —
/// the `Request` type will carry a body field in a later slice; until
/// then the cache key folds in an empty byte string.
pub struct ReplayingFetcher<F: Fetcher> {
    inner: F,
    replay: Arc<dyn Replay>,
}

impl<F: Fetcher> ReplayingFetcher<F> {
    pub fn new(inner: F, replay: Arc<dyn Replay>) -> Self {
        Self { inner, replay }
    }
}

impl<F: Fetcher> Fetcher for ReplayingFetcher<F> {
    fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
        // No body field on `Request` yet — slice 19 hashes the empty body.
        let body: &[u8] = b"";
        match self.replay.lookup(req, body) {
            Ok(Some(rec)) => Ok(Response {
                request: req.clone(),
                final_url: rec.final_url,
                status: rec.status,
                body: rec.body,
                headers: rec.headers,
            }),
            Ok(None) => {
                let resp = self.inner.fetch(req)?;
                let _ = self.replay.record(req, body, &resp);
                Ok(resp)
            }
            Err(_) => self.inner.fetch(req),
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn sanitise(spider_id: &str) -> String {
    spider_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension({
        let mut s = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        s.push_str(".tmp");
        s
    });
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> io::Result<Vec<u8>> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD
        .decode(s)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraping::request::Request;
    use crate::scraping::session::{BackendKind, SessionManager};
    use crate::scraping::spider::{
        ParseYield, Spider, SpiderConfig, SpiderRunner,
    };
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn resp(req: &Request, status: u16, body: &str) -> Response {
        let mut h = HashMap::new();
        h.insert("content-type".into(), "text/html".into());
        Response {
            request: req.clone(),
            final_url: req.url.clone(),
            status,
            body: body.as_bytes().to_vec(),
            headers: h,
        }
    }

    #[test]
    fn cache_key_is_deterministic_and_collision_resistant() {
        let k1 = cache_key("GET", "https://a.test/", b"");
        let k2 = cache_key("GET", "https://a.test/", b"");
        assert_eq!(k1, k2);
        assert_ne!(k1, cache_key("POST", "https://a.test/", b""));
        assert_ne!(k1, cache_key("GET", "https://a.test/x", b""));
        assert_ne!(k1, cache_key("GET", "https://a.test/", b"payload"));
    }

    #[test]
    fn dir_backend_round_trips() {
        let dir = tempdir().unwrap();
        let store = DirReplay::open(dir.path()).unwrap();
        let req = Request::new("https://x.test/");
        assert!(store.lookup(&req, b"").unwrap().is_none());
        let r = resp(&req, 200, "hello");
        store.record(&req, b"", &r).unwrap();
        let got = store.lookup(&req, b"").unwrap().unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.body, b"hello");
        assert_eq!(got.final_url, "https://x.test/");
        assert_eq!(got.headers.get("content-type").unwrap(), "text/html");
    }

    #[test]
    fn dir_backend_writes_json_plus_body_files() {
        let dir = tempdir().unwrap();
        let store = DirReplay::open(dir.path()).unwrap();
        let req = Request::new("https://x.test/");
        store.record(&req, b"", &resp(&req, 200, "hi")).unwrap();
        let key = cache_key("GET", "https://x.test/", b"");
        assert!(dir.path().join(format!("{key}.json")).exists());
        assert!(dir.path().join(format!("{key}.body")).exists());
    }

    #[test]
    fn reddb_backend_persists_across_reopen() {
        let dir = tempdir().unwrap();
        let req = Request::new("https://r.test/p");
        {
            let s = ReddbReplay::open(dir.path(), "spider-a").unwrap();
            s.record(&req, b"", &resp(&req, 201, "body")).unwrap();
        }
        let s2 = ReddbReplay::open(dir.path(), "spider-a").unwrap();
        let got = s2.lookup(&req, b"").unwrap().unwrap();
        assert_eq!(got.status, 201);
        assert_eq!(got.body, b"body");
    }

    #[test]
    fn reddb_backend_isolates_spiders() {
        let dir = tempdir().unwrap();
        let a = ReddbReplay::open(dir.path(), "alpha").unwrap();
        let b = ReddbReplay::open(dir.path(), "beta").unwrap();
        let req = Request::new("https://shared.test/");
        a.record(&req, b"", &resp(&req, 200, "from-a")).unwrap();
        assert!(b.lookup(&req, b"").unwrap().is_none());
        assert_ne!(a.path(), b.path());
    }

    /// Mock fetcher that counts how many times it was called. Stands in
    /// for "the fixture server" — what we care about is whether replay
    /// short-circuited before reaching this layer.
    struct CountingFetcher {
        responses: HashMap<String, (u16, Vec<u8>)>,
        hits: Mutex<usize>,
    }
    impl CountingFetcher {
        fn new() -> Self {
            Self { responses: HashMap::new(), hits: Mutex::new(0) }
        }
        fn with(mut self, url: &str, status: u16, body: &str) -> Self {
            self.responses
                .insert(url.into(), (status, body.as_bytes().to_vec()));
            self
        }
        fn hits(&self) -> usize {
            *self.hits.lock().unwrap()
        }
    }
    impl Fetcher for CountingFetcher {
        fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
            *self.hits.lock().unwrap() += 1;
            let (status, body) = self
                .responses
                .get(&req.url)
                .cloned()
                .unwrap_or((404, b"".to_vec()));
            Ok(Response {
                request: req.clone(),
                final_url: req.url.clone(),
                status,
                body,
                headers: HashMap::new(),
            })
        }
    }

    struct SimpleSpider;
    impl Spider for SimpleSpider {
        fn start_urls(&self) -> Vec<String> {
            vec!["https://fixture.test/a".into(), "https://fixture.test/b".into()]
        }
        fn parse(&self, resp: &Response) -> Vec<ParseYield> {
            vec![ParseYield::item(serde_json::json!({
                "url": resp.final_url,
                "text": resp.text(),
            }))]
        }
    }

    fn mgr() -> Arc<SessionManager> {
        Arc::new(SessionManager::new(BackendKind::Http))
    }

    #[test]
    fn replaying_fetcher_records_then_replays_without_hitting_inner() {
        let dir = tempdir().unwrap();
        let store: Arc<dyn Replay> = Arc::new(DirReplay::open(dir.path()).unwrap());

        // First run: every fetch goes through the inner fetcher and is
        // recorded.
        let inner1 = CountingFetcher::new()
            .with("https://fixture.test/a", 200, "page-a")
            .with("https://fixture.test/b", 200, "page-b");
        let wrapped1 = ReplayingFetcher::new(inner1, Arc::clone(&store));
        let mut r1 = SpiderRunner::new(SpiderConfig::default(), mgr());
        let spider = SimpleSpider;
        r1.seed(&spider, None);
        let out1 = r1.run(&spider, &wrapped1);
        assert_eq!(out1.items.len(), 2);
        assert_eq!(wrapped1.inner.hits(), 2, "first run hits inner twice");

        // Second run: same store, fresh inner fetcher that would FAIL
        // if anyone actually called it. Replay must short-circuit.
        struct ExplodingFetcher;
        impl Fetcher for ExplodingFetcher {
            fn fetch(&self, _req: &Request) -> Result<Response, FetchError> {
                Err(FetchError::Network("inner must not be called".into()))
            }
        }
        let wrapped2 = ReplayingFetcher::new(ExplodingFetcher, Arc::clone(&store));
        let mut r2 = SpiderRunner::new(SpiderConfig::default(), mgr());
        r2.seed(&spider, None);
        let out2 = r2.run(&spider, &wrapped2);
        assert_eq!(out2.items.len(), 2, "second run still emits both items");
        assert_eq!(out2.items[0]["text"], "page-a");
        assert_eq!(out2.items[1]["text"], "page-b");
    }

    #[test]
    fn reddb_replay_integration_records_then_replays() {
        let dir = tempdir().unwrap();
        let store: Arc<dyn Replay> =
            Arc::new(ReddbReplay::open(dir.path(), "spider-x").unwrap());

        let inner = CountingFetcher::new().with("https://fixture.test/a", 200, "A");
        let wrapped = ReplayingFetcher::new(inner, Arc::clone(&store));
        let req = Request::new("https://fixture.test/a");
        let r1 = wrapped.fetch(&req).unwrap();
        assert_eq!(r1.body, b"A");
        assert_eq!(wrapped.inner.hits(), 1);

        let r2 = wrapped.fetch(&req).unwrap();
        assert_eq!(r2.body, b"A");
        assert_eq!(wrapped.inner.hits(), 1, "second call served from cache");
    }
}
