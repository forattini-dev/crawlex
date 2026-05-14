// Per-spider persistent store for element fingerprints.
//
// Slice 13 of the v2 scraping framework. The PRD originally named
// `reddb-io/reddb` as the embedded backend; that crate name could not
// be verified without operator input, so this slice ships a small
// file-backed JSON store with the exact public API the PRD specifies:
//
//   * `AdaptiveStore::open(dir, spider_id)` opens (or creates) one file
//     per spider — `<dir>/<spider_id>.adaptive.json`.
//   * `save(domain, identifier, fp)` upserts a fingerprint keyed by
//     `(domain, identifier)`. Writes are atomic (tmp-then-rename) so a
//     crash mid-write leaves the previous version intact.
//   * `retrieve(domain, identifier)` returns `Option<Fingerprint>`.
//
// Spider isolation is enforced by the on-disk path — two spiders with
// overlapping domains write to distinct files and never collide.
// Concurrent reads are safe via `parking_lot::RwLock`; writes are
// serialised on the same lock.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::parser::Fingerprint;

#[derive(Debug, Default, Serialize, Deserialize)]
struct OnDisk {
    /// Map of `"<domain>\u{1f}<identifier>"` → fingerprint. Flattened to
    /// a string key because JSON object keys must be strings.
    entries: HashMap<String, Fingerprint>,
}

pub struct AdaptiveStore {
    path: PathBuf,
    inner: RwLock<OnDisk>,
}

impl AdaptiveStore {
    /// Open (or create) the store for `spider_id` under `dir`. The
    /// directory is created if missing. Returns an empty store on first
    /// use; reads back existing entries on subsequent opens.
    pub fn open(dir: &Path, spider_id: &str) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.adaptive.json", sanitise(spider_id)));
        let inner = if path.exists() {
            let bytes = fs::read(&path)?;
            if bytes.is_empty() {
                OnDisk::default()
            } else {
                serde_json::from_slice::<OnDisk>(&bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            }
        } else {
            OnDisk::default()
        };
        Ok(Self { path, inner: RwLock::new(inner) })
    }

    /// Upsert a fingerprint. Persists immediately so a crashed spider
    /// keeps its learning between runs.
    pub fn save(
        &self,
        domain: &str,
        identifier: &str,
        fingerprint: Fingerprint,
    ) -> io::Result<()> {
        let key = make_key(domain, identifier);
        let mut g = self.inner.write();
        g.entries.insert(key, fingerprint);
        write_atomic(&self.path, &*g)
    }

    /// Look up by `(domain, identifier)`. Returns `None` if absent.
    pub fn retrieve(&self, domain: &str, identifier: &str) -> Option<Fingerprint> {
        let key = make_key(domain, identifier);
        self.inner.read().entries.get(&key).cloned()
    }

    /// Path of the on-disk file. Exposed for tests and operator tooling.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn make_key(domain: &str, identifier: &str) -> String {
    // ASCII unit-separator — illegal in domains and rare in identifiers,
    // so it disambiguates `("a.b", "c")` from `("a", "b/c")`.
    format!("{}\u{1f}{}", domain, identifier)
}

fn sanitise(spider_id: &str) -> String {
    spider_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect()
}

fn write_atomic(path: &Path, data: &OnDisk) -> io::Result<()> {
    let tmp = path.with_extension("adaptive.json.tmp");
    let bytes = serde_json::to_vec(data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use std::thread;

    use tempfile::tempdir;

    use super::*;

    fn fp(tag: &str, id_val: &str, text: &str) -> Fingerprint {
        let mut classes = BTreeSet::new();
        classes.insert("a".to_string());
        classes.insert("b".to_string());
        Fingerprint {
            tag: tag.to_string(),
            id: Some(id_val.to_string()),
            classes,
            href: Some("/x".to_string()),
            other_attrs: BTreeMap::from([("data-x".to_string(), "1".to_string())]),
            text_hash: 0xdeadbeef,
            text_tokens: vec![text.to_string()],
            parent_chain: vec!["html".into(), "body".into()],
            sibling_index: 2,
        }
    }

    #[test]
    fn round_trip_identical_equality() {
        let dir = tempdir().unwrap();
        let s = AdaptiveStore::open(dir.path(), "spider-a").unwrap();
        let f = fp("a", "x", "hello");
        s.save("example.com", "btn.cta", f.clone()).unwrap();
        let got = s.retrieve("example.com", "btn.cta").unwrap();
        assert_eq!(got, f);
    }

    #[test]
    fn missing_key_returns_none() {
        let dir = tempdir().unwrap();
        let s = AdaptiveStore::open(dir.path(), "spider-a").unwrap();
        assert!(s.retrieve("example.com", "missing").is_none());
    }

    #[test]
    fn reopen_persists_entries() {
        let dir = tempdir().unwrap();
        let f = fp("p", "y", "world");
        {
            let s = AdaptiveStore::open(dir.path(), "s1").unwrap();
            s.save("example.com", "k", f.clone()).unwrap();
        }
        let s2 = AdaptiveStore::open(dir.path(), "s1").unwrap();
        assert_eq!(s2.retrieve("example.com", "k").unwrap(), f);
    }

    #[test]
    fn two_spiders_overlapping_domain_do_not_collide() {
        let dir = tempdir().unwrap();
        let a = AdaptiveStore::open(dir.path(), "spider-a").unwrap();
        let b = AdaptiveStore::open(dir.path(), "spider-b").unwrap();
        let fa = fp("a", "x", "alpha");
        let fb = fp("a", "x", "beta");
        a.save("shared.example", "btn", fa.clone()).unwrap();
        b.save("shared.example", "btn", fb.clone()).unwrap();
        assert_eq!(a.retrieve("shared.example", "btn").unwrap(), fa);
        assert_eq!(b.retrieve("shared.example", "btn").unwrap(), fb);
        assert_ne!(a.path(), b.path());
    }

    #[test]
    fn key_separator_disambiguates() {
        let dir = tempdir().unwrap();
        let s = AdaptiveStore::open(dir.path(), "s").unwrap();
        let f1 = fp("a", "1", "one");
        let f2 = fp("a", "2", "two");
        s.save("a.b", "c", f1.clone()).unwrap();
        s.save("a", "b/c", f2.clone()).unwrap();
        assert_eq!(s.retrieve("a.b", "c").unwrap(), f1);
        assert_eq!(s.retrieve("a", "b/c").unwrap(), f2);
    }

    #[test]
    fn concurrent_reads_safe_across_tasks() {
        let dir = tempdir().unwrap();
        let s = Arc::new(AdaptiveStore::open(dir.path(), "s").unwrap());
        let f = fp("a", "x", "hi");
        s.save("example.com", "k", f.clone()).unwrap();
        let mut handles = Vec::new();
        for _ in 0..16 {
            let s = Arc::clone(&s);
            let expected = f.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..50 {
                    let got = s.retrieve("example.com", "k").unwrap();
                    assert_eq!(got, expected);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn spider_id_with_path_separators_sanitised() {
        let dir = tempdir().unwrap();
        let s = AdaptiveStore::open(dir.path(), "spider/with..slashes").unwrap();
        // Path stays inside dir; no directory traversal.
        assert_eq!(s.path().parent().unwrap(), dir.path());
    }
}
