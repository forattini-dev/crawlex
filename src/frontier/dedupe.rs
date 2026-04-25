//! Frontier dedup — bloom filter + bounded exact recent set.
//!
//! The exact recent set is a cheap hit-or-miss cache; the bloom is the
//! canonical answer. `exact_recent` gets wiped when it grows past
//! `exact_cap`; the bloom stays and catches re-inserts.
//!
//! `insert_url_set` is the intended public entry point for URL dedup: it
//! normalizes a URL and inserts **every canonical permutation**
//! (`{http,https}×{www.,bare}×{/,/index.html,/index.htm,/index.php,∅}`) so we never
//! re-crawl the same page under a different URL spelling. Ported
//! conceptually from Firecrawl's `generateURLPermutations`
//! (`apps/api/src/lib/crawl-redis.ts`).

use growable_bloom_filter::GrowableBloom;
use parking_lot::Mutex;
use std::collections::HashSet;
use url::Url;

pub struct Dedupe {
    bloom: Mutex<GrowableBloom>,
    exact_recent: Mutex<HashSet<String>>,
    exact_cap: usize,
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
    /// is a URL — that one normalises permutations.
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

    /// Insert a URL along with its canonical permutations so any future
    /// attempt to enqueue the same page under `http` vs `https`, `www.` vs
    /// bare, or `/`/`/index.html`/`/index.php` is caught as duplicate.
    ///
    /// Returns `true` when the URL was **newly seen** by every permutation
    /// (i.e. none of the canonical spellings was previously inserted),
    /// `false` when at least one permutation had already been seen.
    pub fn insert_url_set(&self, url: &Url) -> bool {
        let perms = generate_url_permutations(url);
        // `is_new` semantics: a URL is new iff **none** of its permutations
        // was previously seen. We insert every permutation so future lookups
        // under any spelling short-circuit. Check then insert — not the
        // other way — so a single call reports consistent "newness".
        let any_seen = {
            let recent = self.exact_recent.lock();
            perms.iter().any(|p| recent.contains(p))
        };
        if !any_seen {
            // Double-check the bloom (may have evicted-from-exact hits).
            let b = self.bloom.lock();
            let bloom_seen = perms.iter().any(|p| b.contains(p));
            if bloom_seen {
                // Fall through; still insert for future dedup.
            } else {
                drop(b);
                for p in &perms {
                    self.insert_if_new(p);
                }
                return true;
            }
        }
        // Seen — still insert any missing permutations so future lookups
        // under any spelling also match.
        for p in &perms {
            // insert_if_new handles exact-recent + bloom internally.
            self.insert_if_new(p);
        }
        false
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
