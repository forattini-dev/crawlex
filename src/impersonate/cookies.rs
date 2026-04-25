//! Cookie jar — RFC 6265-light.
//!
//! We parse Set-Cookie using the `cookie` crate so we preserve the full
//! attribute set (Domain, Path, Expires, Max-Age, Secure, HttpOnly,
//! SameSite). Cookies are stored per **registrable** domain and filtered on
//! each outgoing request by:
//!
//! * Domain match (exact or suffix match; `Domain=.example.com` ⇒ subdomains).
//! * Path prefix.
//! * Expiry (Max-Age / Expires).
//! * Secure — only sent over https.
//!
//! SameSite is currently best-effort: the crawler is a single-origin
//! navigator (no cross-site iframes driving requests), so we always send.
//! Persistence to disk is not implemented here — that's a separate feature.
//!
//! We do not attempt to be a compliant reference implementation; we pick the
//! subset detectors actually care about and the subset real sites depend on
//! for session continuity.

use cookie::Cookie as RawCookie;
use dashmap::DashMap;
use http::HeaderMap;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

#[derive(Clone, Debug)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    /// Lowercased effective host. Leading dot stripped.
    pub domain: String,
    /// `true` when Set-Cookie had an explicit `Domain=` attribute (enables
    /// subdomain match); `false` → host-only (exact match required).
    pub domain_explicit: bool,
    pub path: String,
    /// Absolute expiry in seconds since UNIX_EPOCH; `None` = session.
    pub expires_at: Option<u64>,
    pub secure: bool,
    pub http_only: bool,
}

impl StoredCookie {
    fn is_expired(&self, now: u64) -> bool {
        matches!(self.expires_at, Some(t) if t <= now)
    }
}

#[derive(Clone, Default)]
pub struct CookieJar {
    /// Key: registrable domain → list of cookies (ordered by insertion).
    /// List (not map) because (name, domain, path) together form the key per
    /// RFC 6265; two cookies with the same name but different paths coexist.
    inner: Arc<DashMap<String, Arc<Mutex<Vec<StoredCookie>>>>>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    fn registrable(host: &str) -> String {
        crate::discovery::subdomains::registrable_domain(host).unwrap_or_else(|| host.to_string())
    }

    /// Parse all Set-Cookie values from a response and merge into the jar.
    pub fn ingest(&self, url: &Url, headers: &HeaderMap) {
        let Some(host) = url.host_str() else { return };
        let default_path = default_path(url.path());
        let key = Self::registrable(host);
        let slot = self
            .inner
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
            .clone();
        let mut list = slot.lock();
        for value in headers.get_all("set-cookie").iter() {
            let Ok(s) = value.to_str() else { continue };
            let Ok(parsed) = RawCookie::parse(s.to_string()) else {
                continue;
            };
            let name = parsed.name().to_string();
            if name.is_empty() {
                continue;
            }
            let val = parsed.value().to_string();

            // Domain + domain_explicit.
            let (domain, domain_explicit) = match parsed.domain() {
                Some(d) => {
                    let clean = d.trim_start_matches('.').to_ascii_lowercase();
                    if clean.is_empty() {
                        (host.to_ascii_lowercase(), false)
                    } else if !host_matches_domain(host, &clean) {
                        // Invalid Domain attribute — RFC says ignore.
                        continue;
                    } else {
                        (clean, true)
                    }
                }
                None => (host.to_ascii_lowercase(), false),
            };

            // Path: explicit or default (per RFC 6265 §5.1.4).
            let path = parsed
                .path()
                .map(|p| p.to_string())
                .unwrap_or_else(|| default_path.clone());

            // Expiry: Max-Age wins over Expires.
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

            let secure = parsed.secure().unwrap_or(false);
            let http_only = parsed.http_only().unwrap_or(false);

            // Deletion: empty value OR past expiry.
            if val.is_empty() || expires_at.is_some_and(|t| t <= now_secs()) {
                list.retain(|c| !(c.name == name && c.domain == domain && c.path == path));
                continue;
            }

            // Upsert: replace matching (name, domain, path).
            list.retain(|c| !(c.name == name && c.domain == domain && c.path == path));
            if Self::is_high_signal_name(&name) {
                // Emit a trace event so the "did we just refresh __cf_bm?"
                // question is answerable at operate-time without parsing
                // raw Set-Cookie lines. `crawlex::cookies::high_signal`
                // target is cheap to filter in the log pipeline.
                tracing::debug!(
                    target: "crawlex::cookies::high_signal",
                    cookie_name = %name,
                    domain = %domain,
                    expires_at = ?expires_at,
                    "refreshed high-signal antibot cookie"
                );
            }
            list.push(StoredCookie {
                name,
                value: val,
                domain,
                domain_explicit,
                path,
                expires_at,
                secure,
                http_only,
            });
        }
    }

    /// Recognise cookies that anti-bot vendors use to smooth out session
    /// score between requests. Cloudflare's docs explicitly say `__cf_bm`
    /// "consolidates" the bot score per session; losing it across a
    /// proxy/identity rotation forces a fresh re-scoring that often lands
    /// us on a harder challenge. The same idea applies to the other
    /// vendors below.
    ///
    /// Keep the list conservative: false positives here lead to leaking
    /// session state across identity boundaries (bad), so we only match
    /// names/patterns documented by each vendor.
    ///
    /// Sources:
    /// - Cloudflare: `__cf_bm`, `cf_clearance`, `__cfuvid` (see
    ///   developers.cloudflare.com/bots/concepts/bot-detection-engines/)
    /// - Akamai Bot Manager: `_abck`, `bm_sz`, `ak_bmsc`, `bm_sv`, `bm_mi`
    /// - DataDome: `datadome`
    /// - PerimeterX/HUMAN: `_px*` family
    /// - Imperva (Incapsula): `incap_ses_*`, `visid_incap_*`, `reese84`
    pub fn is_high_signal_name(name: &str) -> bool {
        if matches!(
            name,
            "__cf_bm"
                | "cf_clearance"
                | "__cfuvid"
                | "_cfuvid"
                | "__cflb"
                | "_abck"
                | "bm_sz"
                | "ak_bmsc"
                | "bm_sv"
                | "bm_mi"
                | "datadome"
                | "reese84"
        ) {
            return true;
        }
        // PerimeterX names follow `_px<digit>` / `_pxvid` / `_pxde` /
        // `_pxhd`. Tighten the prefix match so unrelated third-party
        // names like `_px_preferences` don't leak across identity
        // rotations (review nit #6).
        if let Some(suffix) = name.strip_prefix("_px") {
            let first = suffix.as_bytes().first().copied();
            return matches!(
                (first, suffix),
                (Some(b'0'..=b'9'), _) | (_, "vid" | "de" | "hd" | "VJ34v3")
            );
        }
        // Imperva (Incapsula) session cookies are always suffixed with a
        // numeric ID; the raw prefix alone never appears on a legitimate
        // name.
        name.starts_with("incap_ses_") || name.starts_with("visid_incap_")
    }

    /// Take a snapshot of every high-signal cookie currently stored for
    /// `host`'s registrable domain. Intended for use at identity-rotation
    /// time: the caller snapshots before rotating, discards the rest of
    /// the jar, then injects the snapshot back via [`Self::inject`]. The
    /// antibot vendor sees a familiar session cookie even though IP, UA
    /// and TLS fingerprint have changed — exactly the state Cloudflare's
    /// docs say `__cf_bm` is designed to absorb.
    pub fn extract_high_signal(&self, host: &str) -> Vec<StoredCookie> {
        let key = Self::registrable(host);
        let Some(slot) = self.inner.get(&key) else {
            return Vec::new();
        };
        let list = slot.lock();
        let now = now_secs();
        list.iter()
            .filter(|c| !c.is_expired(now) && Self::is_high_signal_name(&c.name))
            .cloned()
            .collect()
    }

    /// Enumerate every registrable domain currently tracked by the jar.
    /// Used by identity-rotation code paths that need to snapshot every
    /// host the session touched before tearing a jar down (cannot leak
    /// values — only the host keys are exposed).
    ///
    /// Intentionally does NOT prune expired entries: a host whose only
    /// cookies are expired session tokens is still a host the session
    /// visited; callers that care about expiry inspect the values
    /// returned by [`Self::extract_high_signal`].
    pub fn hosts(&self) -> Vec<String> {
        self.inner.iter().map(|e| e.key().clone()).collect()
    }

    /// Re-insert cookies into the jar for the given host. Uses the same
    /// (name, domain, path) upsert rule as `ingest` so repeated calls are
    /// idempotent. Intended pair with [`Self::extract_high_signal`].
    pub fn inject(&self, host: &str, cookies: Vec<StoredCookie>) {
        if cookies.is_empty() {
            return;
        }
        let key = Self::registrable(host);
        let slot = self
            .inner
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
            .clone();
        let mut list = slot.lock();
        for c in cookies {
            list.retain(|x| !(x.name == c.name && x.domain == c.domain && x.path == c.path));
            list.push(c);
        }
    }

    /// Build a `Cookie:` header value for the outgoing URL. Returns None if
    /// nothing matches.
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let key = Self::registrable(&host);
        let slot = self.inner.get(&key)?.clone();
        let mut list = slot.lock();
        let now = now_secs();
        // Prune expired on the read path so the jar self-heals.
        list.retain(|c| !c.is_expired(now));

        let is_https = url.scheme() == "https";
        let req_path = url.path();
        let mut pairs: Vec<(usize, String)> = Vec::new();
        for (idx, c) in list.iter().enumerate() {
            if c.secure && !is_https {
                continue;
            }
            if !domain_matches(&host, c) {
                continue;
            }
            if !path_matches(req_path, &c.path) {
                continue;
            }
            let _ = c.http_only; // not enforced client-side here
            pairs.push((idx, format!("{}={}", c.name, c.value)));
        }
        if pairs.is_empty() {
            return None;
        }
        // Spec says: longer-path cookies first, then older cookies first.
        // Our `idx` monotonically increases with insertion order, so sort by
        // (path length desc, idx asc).
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
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// RFC 6265 §5.1.4 default-path algorithm.
fn default_path(request_path: &str) -> String {
    if request_path.is_empty() || !request_path.starts_with('/') {
        return "/".into();
    }
    // Strip everything from the last '/' (but keep that '/') unless the path
    // contains only the leading '/'.
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

fn host_matches_domain(host: &str, domain: &str) -> bool {
    let h = host.trim_start_matches('.').to_ascii_lowercase();
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    h == d || h.ends_with(&format!(".{d}"))
}

fn domain_matches(request_host: &str, c: &StoredCookie) -> bool {
    let h = request_host.trim_start_matches('.');
    if c.domain_explicit {
        h == c.domain || h.ends_with(&format!(".{}", c.domain))
    } else {
        h == c.domain
    }
}

/// RFC 6265 §5.1.4 path-match algorithm.
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

// Keep old imports pruned to silence "unused" warnings when we removed
// IndexMap & Duration-heavy helpers but the compiler still sees them.
#[allow(dead_code)]
const _: Duration = Duration::from_secs(0);

#[cfg(test)]
mod high_signal_tests {
    use super::*;
    use http::HeaderMap;

    #[test]
    fn classifier_matches_documented_vendor_names() {
        // Cloudflare
        assert!(CookieJar::is_high_signal_name("__cf_bm"));
        assert!(CookieJar::is_high_signal_name("cf_clearance"));
        assert!(CookieJar::is_high_signal_name("__cfuvid"));
        // Akamai
        assert!(CookieJar::is_high_signal_name("_abck"));
        assert!(CookieJar::is_high_signal_name("bm_sz"));
        assert!(CookieJar::is_high_signal_name("ak_bmsc"));
        // DataDome, Imperva, PerimeterX
        assert!(CookieJar::is_high_signal_name("datadome"));
        assert!(CookieJar::is_high_signal_name("reese84"));
        assert!(CookieJar::is_high_signal_name("_pxde"));
        assert!(CookieJar::is_high_signal_name("_pxvid"));
        assert!(CookieJar::is_high_signal_name("incap_ses_1234_56789"));
        assert!(CookieJar::is_high_signal_name("visid_incap_56789"));
    }

    #[test]
    fn classifier_rejects_unrelated_names() {
        // Lax false-positive avoidance: don't absorb generic state.
        assert!(!CookieJar::is_high_signal_name("session"));
        assert!(!CookieJar::is_high_signal_name("PHPSESSID"));
        assert!(!CookieJar::is_high_signal_name("csrf"));
        assert!(!CookieJar::is_high_signal_name("user_id"));
        // `_px` prefix: accepted only when followed by documented suffixes.
        assert!(CookieJar::is_high_signal_name("_px3"));
        assert!(CookieJar::is_high_signal_name("_pxvid"));
        assert!(CookieJar::is_high_signal_name("_pxde"));
        assert!(CookieJar::is_high_signal_name("_pxhd"));
        // Unrelated names that previously matched the loose `_px` prefix:
        assert!(!CookieJar::is_high_signal_name("_px_preferences"));
        assert!(!CookieJar::is_high_signal_name("_pxtest"));
    }

    fn url(u: &str) -> Url {
        Url::parse(u).unwrap()
    }

    fn set_cookie_headers(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append("set-cookie", v.parse().unwrap());
        }
        h
    }

    #[test]
    fn extract_high_signal_returns_only_matching_cookies() {
        let jar = CookieJar::new();
        let u = url("https://example.com/");
        jar.ingest(
            &u,
            &set_cookie_headers(&[
                "__cf_bm=abc; Domain=.example.com; Path=/; Secure",
                "session_id=xyz; Domain=.example.com; Path=/",
                "_abck=zzz; Domain=.example.com; Path=/",
            ]),
        );
        let snapshot = jar.extract_high_signal("example.com");
        let names: Vec<_> = snapshot.iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"__cf_bm".to_string()));
        assert!(names.contains(&"_abck".to_string()));
        assert!(!names.contains(&"session_id".to_string()));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn inject_preserves_cookies_across_synthetic_rotation() {
        // Simulate the rotation workflow: extract, swap in a fresh jar,
        // inject back, verify Cookie: header still contains the high-
        // signal values.
        let old = CookieJar::new();
        let u = url("https://example.com/");
        old.ingest(
            &u,
            &set_cookie_headers(&[
                "__cf_bm=live; Domain=.example.com; Path=/; Secure",
                "non_critical=tmp; Domain=.example.com; Path=/",
            ]),
        );
        let preserved = old.extract_high_signal("example.com");
        assert_eq!(preserved.len(), 1);

        let fresh = CookieJar::new();
        // Fresh jar has nothing for example.com.
        assert!(fresh.cookie_header(&u).is_none());

        fresh.inject("example.com", preserved);
        let cookie_line = fresh.cookie_header(&u).expect("cookie header after inject");
        assert!(
            cookie_line.contains("__cf_bm=live"),
            "__cf_bm must survive rotation: {cookie_line}"
        );
        assert!(
            !cookie_line.contains("non_critical"),
            "non-critical cookies must NOT cross rotation: {cookie_line}"
        );
    }

    #[test]
    fn hosts_enumerates_every_registrable_domain_touched() {
        let jar = CookieJar::new();
        jar.ingest(
            &url("https://a.example.com/"),
            &set_cookie_headers(&["__cf_bm=1; Domain=.example.com; Path=/"]),
        );
        jar.ingest(
            &url("https://foo.test/"),
            &set_cookie_headers(&["_abck=2; Domain=.foo.test; Path=/"]),
        );
        let mut hosts = jar.hosts();
        hosts.sort();
        assert_eq!(
            hosts,
            vec!["example.com".to_string(), "foo.test".to_string()]
        );
    }

    #[test]
    fn multi_host_extract_inject_round_trip_preserves_all_high_signal() {
        // Regression guard for the identity-rotation workflow: enumerate
        // every host the session touched via `hosts()`, snapshot each,
        // then re-inject into a fresh jar. All high-signal cookies must
        // survive; no non-critical state may cross the rotation boundary.
        let old = CookieJar::new();
        old.ingest(
            &url("https://one.example.com/"),
            &set_cookie_headers(&[
                "__cf_bm=alpha; Domain=.example.com; Path=/; Secure",
                "session=leaky; Domain=.example.com; Path=/",
            ]),
        );
        old.ingest(
            &url("https://api.foo.test/"),
            &set_cookie_headers(&[
                "datadome=beta; Domain=.foo.test; Path=/",
                "PHPSESSID=leaky2; Domain=.foo.test; Path=/",
            ]),
        );

        let snapshots: Vec<(String, Vec<StoredCookie>)> = old
            .hosts()
            .into_iter()
            .map(|h| {
                let snap = old.extract_high_signal(&h);
                (h, snap)
            })
            .collect();
        // Two hosts, each with exactly one high-signal cookie.
        let total: usize = snapshots.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(total, 2);

        let fresh = CookieJar::new();
        for (host, snap) in snapshots {
            fresh.inject(&host, snap);
        }

        let line_one = fresh
            .cookie_header(&url("https://one.example.com/"))
            .expect("example.com header after inject");
        assert!(line_one.contains("__cf_bm=alpha"));
        assert!(!line_one.contains("session=leaky"));

        let line_two = fresh
            .cookie_header(&url("https://api.foo.test/"))
            .expect("foo.test header after inject");
        assert!(line_two.contains("datadome=beta"));
        assert!(!line_two.contains("PHPSESSID"));
    }

    #[test]
    fn inject_upserts_on_name_domain_path_match() {
        let jar = CookieJar::new();
        let u = url("https://example.com/");
        jar.ingest(
            &u,
            &set_cookie_headers(&["__cf_bm=old; Domain=.example.com; Path=/"]),
        );
        // Snapshot one value, overwrite with a newer value via inject.
        let mut snapshot = jar.extract_high_signal("example.com");
        assert_eq!(snapshot.len(), 1);
        snapshot[0].value = "new".into();
        jar.inject("example.com", snapshot);
        let line = jar.cookie_header(&u).unwrap();
        assert!(line.contains("__cf_bm=new"));
        assert!(!line.contains("__cf_bm=old"));
    }
}
