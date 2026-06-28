//! Frontier dedup — bloom filter + bounded exact recent set.
//!
//! The exact recent set is the only in-memory duplicate answer that can skip
//! enqueue immediately. The bloom is a fast L1 hint: a hit returns
//! [`DedupeProbe::MaybeSeen`] and must be confirmed by an exact admission
//! layer before dropping a URL, because a bloom false positive would otherwise
//! lose coverage.
//!
//! `insert_url_set` is the legacy convenience entry point for callers that do
//! not have a queue admission layer. Frontier admission should use
//! [`Dedupe::probe_url`] and confirm non-recent hits against the exact queue
//! identity before calling [`Dedupe::mark_seen`].

use growable_bloom_filter::GrowableBloom;
use parking_lot::Mutex;
use std::collections::HashSet;
use url::Url;

use crate::frontier::identity::UrlIdentity;

pub struct Dedupe {
    bloom: Mutex<GrowableBloom>,
    exact_recent: Mutex<HashSet<String>>,
    exact_cap: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupeProbe {
    New(UrlIdentity),
    RecentDuplicate(UrlIdentity),
    MaybeSeen(UrlIdentity),
}

impl DedupeProbe {
    pub fn identity(&self) -> &UrlIdentity {
        match self {
            DedupeProbe::New(identity)
            | DedupeProbe::RecentDuplicate(identity)
            | DedupeProbe::MaybeSeen(identity) => identity,
        }
    }
}

impl Dedupe {
    pub fn new(expected: usize, fp_rate: f64) -> Self {
        Self {
            bloom: Mutex::new(GrowableBloom::new(fp_rate, expected)),
            exact_recent: Mutex::new(HashSet::new()),
            exact_cap: 100_000,
        }
    }

    /// Raw string-level dedup. Prefer [`Self::insert_url_set`] when the key
    /// is a URL because that uses the shared canonical URL identity.
    pub fn insert_if_new(&self, key: &str) -> bool {
        {
            let mut recent = self.exact_recent.lock();
            if recent.contains(key) {
                return false;
            }
            if recent.len() >= self.exact_cap {
                recent.clear();
            }
            recent.insert(key.to_string());
        }
        let mut b = self.bloom.lock();
        b.insert(key)
    }

    pub fn probe_url(&self, url: &Url) -> DedupeProbe {
        let identity = UrlIdentity::from_url(url);
        {
            let recent = self.exact_recent.lock();
            if recent.contains(&identity.canonical_key) {
                return DedupeProbe::RecentDuplicate(identity);
            }
        }
        let b = self.bloom.lock();
        if b.contains(&identity.canonical_key) {
            DedupeProbe::MaybeSeen(identity)
        } else {
            DedupeProbe::New(identity)
        }
    }

    pub fn mark_seen(&self, identity: &UrlIdentity) {
        {
            let mut recent = self.exact_recent.lock();
            if recent.len() >= self.exact_cap {
                recent.clear();
            }
            recent.insert(identity.canonical_key.clone());
        }
        let mut b = self.bloom.lock();
        b.insert(&identity.canonical_key);
    }

    /// Insert a URL using the shared canonical identity so future attempts to
    /// enqueue the same page under `http` vs `https`, `www.` vs bare, or
    /// `/`/`/index.html`/`/index.php` are caught as duplicates.
    ///
    /// Returns `true` when the URL was not present in the exact recent set and
    /// did not hit the bloom. A bloom hit returns `false`, so frontier
    /// admission should not use this method as its final answer.
    pub fn insert_url_set(&self, url: &Url) -> bool {
        let probe = self.probe_url(url);
        let is_new = matches!(probe, DedupeProbe::New(_));
        self.mark_seen(probe.identity());
        is_new
    }
}

/// Expand a URL into its canonical permutation set. Returns at most 20
/// entries (`2 schemes × 2 host variants × 5 path variants`).
///
/// Invariants (property-tested in `tests/url_permutations.rs`):
/// 1. Non-empty: always at least one entry (the input itself, normalized).
/// 2. Idempotent: applying twice returns the same set.
/// 3. No overlap: distinct input URLs that aren't aliases produce disjoint
///    permutation sets.
pub fn generate_url_permutations(url: &Url) -> Vec<String> {
    let scheme = url.scheme();
    // Only http/https get permuted — other schemes (data:, blob:) return
    // just the stringified input.
    if scheme != "http" && scheme != "https" {
        return vec![url.as_str().to_string()];
    }

    let host = match url.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return vec![url.as_str().to_string()],
    };
    let bare = host.trim_start_matches("www.").to_string();
    let hosts: Vec<String> = if bare == host {
        vec![host.clone(), format!("www.{bare}")]
    } else {
        vec![host.clone(), bare.clone()]
    };

    let port = url.port();
    let path = url.path();
    let path = if path.is_empty() { "/" } else { path };
    let query = canonical_query(url);

    // Path variants: strip trailing `/index.html|index.php` → base; add back
    // each variant plus bare "/". De-dup at the end.
    let path_base = path
        .strip_suffix("/index.html")
        .or_else(|| path.strip_suffix("/index.htm"))
        .or_else(|| path.strip_suffix("/index.php"))
        .unwrap_or(path)
        .trim_end_matches('/')
        .to_string();
    let path_base = if path_base.is_empty() {
        "".to_string()
    } else {
        path_base
    };
    let path_variants: Vec<String> = {
        let mut v = vec![
            format!("{path_base}/"),
            format!("{path_base}/index.html"),
            format!("{path_base}/index.htm"),
            format!("{path_base}/index.php"),
        ];
        if !path_base.is_empty() {
            // Allow the base with no trailing slash too.
            v.push(path_base.clone());
        }
        v
    };

    let schemes = ["http", "https"];
    let mut out: Vec<String> = Vec::with_capacity(16);
    for s in &schemes {
        for h in &hosts {
            for p in &path_variants {
                let mut u = format!("{s}://{h}");
                if let Some(pt) = port {
                    u.push_str(&format!(":{pt}"));
                }
                u.push_str(p);
                if let Some(q) = query.as_deref() {
                    u.push('?');
                    u.push_str(q);
                }
                out.push(u);
            }
        }
    }
    // De-dup preserving order.
    let mut seen = HashSet::new();
    out.retain(|u| seen.insert(u.clone()));
    out
}

fn canonical_query(url: &Url) -> Option<String> {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_query_key(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        return None;
    }
    pairs.sort();
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        out.append_pair(&k, &v);
    }
    Some(out.finish())
}

fn is_tracking_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_") || matches!(key.as_str(), "fbclid" | "gclid" | "mc_cid" | "mc_eid")
}
