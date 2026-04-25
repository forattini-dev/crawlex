//! CHIPS (Cookies Having Independent Partitioned State) support.
//!
//! Wave 1 #23. Modern browsers (Chrome 114+, Safari, Firefox) store
//! cookies that carry the `Partitioned` attribute under a double-key
//! combining the top-level site and the cookie origin. Detectors that
//! compare a crawler's cookie-emission pattern against a real browser
//! look for three things:
//!
//! 1. The `Partitioned` attribute is honoured on ingest (unpartitioned
//!    storage leaks third-party cookies into the top-level context).
//! 2. `SameSite=None; Secure` is *required* for `Partitioned` to take
//!    effect — bare `Partitioned` is treated as if the attribute were
//!    absent (matches Chromium behaviour).
//! 3. A cookie set under top-level site `A` is not returned when the
//!    same origin is embedded under top-level site `B`.
//!
//! We model that as a `PartitionedCookieStore` keyed by
//! `(top_level_site, origin)`. Non-partitioned cookies fall back to the
//! existing unpartitioned jar owned by `impersonate::cookies` — this
//! module explicitly does NOT try to replace it.
//!
//! Refs:
//! - <https://developers.google.com/privacy-sandbox/3pcd/chips>
//! - <https://datatracker.ietf.org/doc/draft-cutler-httpbis-partitioned-cookies/>

use cookie::Cookie as RawCookie;
use dashmap::DashMap;
use http::HeaderMap;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

/// A single stored partitioned cookie. Mirrors the shape of
/// `impersonate::cookies::StoredCookie` but deliberately kept as a
/// distinct type: CHIPS cookies have a strict set of attribute
/// requirements the unpartitioned jar doesn't enforce.
#[derive(Clone, Debug)]
pub struct PartitionedCookie {
    pub name: String,
    pub value: String,
    /// Origin the cookie was set from (scheme + host + port).
    pub origin: String,
    pub path: String,
    pub expires_at: Option<u64>,
    /// Always `true` for CHIPS cookies (spec requirement).
    pub secure: bool,
    pub http_only: bool,
    /// `SameSite` attribute the server sent. CHIPS requires `None`.
    pub same_site: SameSite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
    Unspecified,
}

impl SameSite {
    fn from_raw(raw: Option<cookie::SameSite>) -> Self {
        match raw {
            Some(cookie::SameSite::Strict) => SameSite::Strict,
            Some(cookie::SameSite::Lax) => SameSite::Lax,
            Some(cookie::SameSite::None) => SameSite::None,
            None => SameSite::Unspecified,
        }
    }
}

impl PartitionedCookie {
    fn is_expired(&self, now: u64) -> bool {
        matches!(self.expires_at, Some(t) if t <= now)
    }
}

/// Partition key: `(top_level_site, origin)`. We store the site as the
/// registrable domain of the top-level URL — the CHIPS spec actually
/// mandates the "schemeful site", but for bot-detection purposes
/// registrable-domain isolation matches what Chromium exposes to
/// third-party frames.
type PartitionKey = (String, String);

#[derive(Default)]
pub struct PartitionedCookieStore {
    inner: Arc<DashMap<PartitionKey, Arc<Mutex<Vec<PartitionedCookie>>>>>,
    /// Count of Set-Cookie headers rejected because they carried
    /// `Partitioned` without the `SameSite=None; Secure` combo. Useful
    /// for operate-time observability — the fixture tests read this.
    invalid_partitioned: Arc<parking_lot::Mutex<usize>>,
}

impl PartitionedCookieStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest every Set-Cookie in `headers` for the response at `url`
    /// embedded under `top_level_site`. Cookies without `Partitioned`
    /// are ignored here (the unpartitioned jar handles them); cookies
    /// with `Partitioned` but missing the required attributes are
    /// dropped and counted in `invalid_partitioned_count()`.
    pub fn ingest(&self, top_level_site: &str, url: &Url, headers: &HeaderMap) {
        let Some(host) = url.host_str() else { return };
        let origin = url.origin().ascii_serialization();
        let default_path = default_path(url.path());
        let site = registrable(top_level_site);
        let key: PartitionKey = (site, origin.clone());

        let slot = self
            .inner
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
            .clone();
        let mut list = slot.lock();

        for value in headers.get_all("set-cookie").iter() {
            let Ok(s) = value.to_str() else { continue };
            // `Partitioned` isn't yet recognised by the `cookie` crate's
            // parser as a typed attribute, so we detect it by scanning
            // the raw attribute list. Case-insensitive per RFC 6265.
            let partitioned = has_partitioned_attr(s);
            if !partitioned {
                continue;
            }
            let Ok(parsed) = RawCookie::parse(s.to_string()) else {
                continue;
            };
            let name = parsed.name().to_string();
            if name.is_empty() {
                continue;
            }

            let secure = parsed.secure().unwrap_or(false);
            let same_site = SameSite::from_raw(parsed.same_site());

            // Chromium: `Partitioned` cookies MUST be `Secure` AND
            // `SameSite=None`; otherwise the attribute is treated as
            // if absent — i.e. the cookie is dropped from the
            // partitioned store entirely. We don't silently promote
            // to the unpartitioned jar; that's the caller's concern.
            if !secure || same_site != SameSite::None {
                *self.invalid_partitioned.lock() += 1;
                tracing::debug!(
                    target: "crawlex::chips::rejected",
                    cookie_name = %name,
                    host = %host,
                    secure, ?same_site,
                    "Partitioned cookie missing required SameSite=None; Secure"
                );
                continue;
            }

            let path = parsed
                .path()
                .map(|p| p.to_string())
                .unwrap_or_else(|| default_path.clone());
            let expires_at = parsed
                .max_age()
                .map(|d| now_secs() + d.whole_seconds().max(0) as u64)
                .or_else(|| {
                    parsed.expires().and_then(|e| {
                        e.datetime().map(|dt| {
                            let ts = dt.unix_timestamp();
                            if ts < 0 {
                                0
                            } else {
                                ts as u64
                            }
                        })
                    })
                });
            let http_only = parsed.http_only().unwrap_or(false);
            let val = parsed.value().to_string();

            // Deletion sentinels.
            if val.is_empty() || expires_at.is_some_and(|t| t <= now_secs()) {
                list.retain(|c| !(c.name == name && c.path == path));
                continue;
            }

            list.retain(|c| !(c.name == name && c.path == path));
            list.push(PartitionedCookie {
                name,
                value: val,
                origin: origin.clone(),
                path,
                expires_at,
                secure,
                http_only,
                same_site,
            });
        }
    }

    /// Build the `Cookie:` header for `url` when embedded under
    /// `top_level_site`. Returns `None` when the partitioned store has
    /// nothing to add — the caller is expected to merge this with the
    /// unpartitioned jar's output.
    pub fn cookie_header(&self, top_level_site: &str, url: &Url) -> Option<String> {
        let origin = url.origin().ascii_serialization();
        let key: PartitionKey = (registrable(top_level_site), origin);
        let slot = self.inner.get(&key)?.clone();
        let mut list = slot.lock();
        let now = now_secs();
        list.retain(|c| !c.is_expired(now));

        let is_https = url.scheme() == "https";
        let req_path = url.path();
        let mut pairs: Vec<(usize, String)> = Vec::new();
        for (idx, c) in list.iter().enumerate() {
            if c.secure && !is_https {
                continue;
            }
            if !path_matches(req_path, &c.path) {
                continue;
            }
            let _ = c.http_only;
            pairs.push((idx, format!("{}={}", c.name, c.value)));
        }
        if pairs.is_empty() {
            return None;
        }
        pairs.sort_by(|a, b| {
            let pa = list[a.0].path.len();
            let pb = list[b.0].path.len();
            pb.cmp(&pa).then(a.0.cmp(&b.0))
        });
        Some(
            pairs
                .into_iter()
                .map(|(_, s)| s)
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    pub fn invalid_partitioned_count(&self) -> usize {
        *self.invalid_partitioned.lock()
    }

    /// List every `(top_level_site, origin)` pair currently populated.
    /// Lets callers prove partition isolation — cookies in partition A
    /// must not appear under partition B.
    pub fn partitions(&self) -> Vec<PartitionKey> {
        self.inner.iter().map(|e| e.key().clone()).collect()
    }
}

fn has_partitioned_attr(raw: &str) -> bool {
    // The `cookie` crate parses unknown attributes into the "unparsed"
    // list that we can't reach without feature flags; a simple scan is
    // both cheaper and explicit about the casing rules we enforce.
    raw.split(';').any(|seg| {
        let t = seg.trim();
        t.eq_ignore_ascii_case("Partitioned")
    })
}

fn default_path(request_path: &str) -> String {
    if request_path.is_empty() || !request_path.starts_with('/') {
        return "/".into();
    }
    if let Some(idx) = request_path.rfind('/') {
        if idx == 0 {
            "/".into()
        } else {
            request_path[..idx].into()
        }
    } else {
        "/".into()
    }
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if request_path.starts_with(cookie_path) {
        let after = &request_path[cookie_path.len()..];
        if cookie_path.ends_with('/') || after.starts_with('/') {
            return true;
        }
    }
    false
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn registrable(host_or_url: &str) -> String {
    // Accept either a bare host or a full URL — convenient for
    // callers that pass the top-level navigation URL directly.
    if let Ok(u) = Url::parse(host_or_url) {
        if let Some(h) = u.host_str() {
            return crate::discovery::subdomains::registrable_domain(h)
                .unwrap_or_else(|| h.to_ascii_lowercase());
        }
    }
    crate::discovery::subdomains::registrable_domain(host_or_url)
        .unwrap_or_else(|| host_or_url.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(u: &str) -> Url {
        Url::parse(u).unwrap()
    }

    fn set_cookie(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append("set-cookie", v.parse().unwrap());
        }
        h
    }

    #[test]
    fn ingests_valid_partitioned_cookie() {
        let store = PartitionedCookieStore::new();
        store.ingest(
            "https://top.example/",
            &url("https://embed.cdn/"),
            &set_cookie(&["sid=abc; Path=/; Secure; SameSite=None; Partitioned"]),
        );
        let hdr = store
            .cookie_header("https://top.example/", &url("https://embed.cdn/"))
            .expect("cookie emitted");
        assert!(hdr.contains("sid=abc"));
    }

    #[test]
    fn rejects_partitioned_without_required_attrs() {
        let store = PartitionedCookieStore::new();
        // Missing Secure.
        store.ingest(
            "https://top.example/",
            &url("https://embed.cdn/"),
            &set_cookie(&["a=1; Path=/; SameSite=None; Partitioned"]),
        );
        // Missing SameSite=None (defaults to Lax treatment).
        store.ingest(
            "https://top.example/",
            &url("https://embed.cdn/"),
            &set_cookie(&["b=2; Path=/; Secure; Partitioned"]),
        );
        assert_eq!(store.invalid_partitioned_count(), 2);
        assert!(store
            .cookie_header("https://top.example/", &url("https://embed.cdn/"))
            .is_none());
    }

    #[test]
    fn ignores_non_partitioned_cookies() {
        let store = PartitionedCookieStore::new();
        store.ingest(
            "https://top.example/",
            &url("https://embed.cdn/"),
            &set_cookie(&["plain=yes; Path=/; Secure; SameSite=None"]),
        );
        assert!(store
            .cookie_header("https://top.example/", &url("https://embed.cdn/"))
            .is_none());
    }

    #[test]
    fn isolates_by_top_level_site() {
        let store = PartitionedCookieStore::new();
        store.ingest(
            "https://siteA.test/",
            &url("https://embed.cdn/"),
            &set_cookie(&["k=A; Path=/; Secure; SameSite=None; Partitioned"]),
        );
        store.ingest(
            "https://siteB.test/",
            &url("https://embed.cdn/"),
            &set_cookie(&["k=B; Path=/; Secure; SameSite=None; Partitioned"]),
        );
        let a = store
            .cookie_header("https://siteA.test/", &url("https://embed.cdn/"))
            .unwrap();
        let b = store
            .cookie_header("https://siteB.test/", &url("https://embed.cdn/"))
            .unwrap();
        assert!(a.contains("k=A") && !a.contains("k=B"));
        assert!(b.contains("k=B") && !b.contains("k=A"));
        assert_eq!(store.partitions().len(), 2);
    }
}
