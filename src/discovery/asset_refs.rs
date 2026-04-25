//! Classified extraction of asset references from a rendered page.
//!
//! Every `<script src>`, `<link rel="stylesheet">`, `<img>`, `<iframe>`,
//! `<video>`, `<audio>`, `<a href>` produces an `AssetRef` tagged with
//! kind, target URL, target registrable domain, and whether it lives
//! inside the current target scope.
//!
//! This is the data that populates the `asset_refs` and
//! `external_domains` tables introduced by Fase A — it answers the
//! operator's question "what *external* infra is this page pulling
//! from?" without us having to crawl those targets.

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use url::Url;

use crate::discovery::subdomains::registrable_domain;

static SCRIPT_SRC_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("script[src]").expect("static selector"));
static LINK_HREF_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("link[href]").expect("static selector"));
static IMG_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("img").expect("static selector"));
static SOURCE_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("source").expect("static selector"));
static IFRAME_SRC_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("iframe[src]").expect("static selector"));
static DATA_SITEKEY_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("[data-sitekey]").expect("static selector"));
static VIDEO_SRC_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("video[src]").expect("static selector"));
static AUDIO_SRC_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("audio[src]").expect("static selector"));
static A_HREF_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").expect("static selector"));

/// Coarse vendor category for an external host. A single domain can match
/// more than one bucket (`googletagmanager.com` is both a tag manager and
/// an analytics vendor) — `categorise()` returns every match.
///
/// Kept deliberately small: this is operator-facing rollup, not a full
/// vendor taxonomy. Matches the strings persisted in
/// `external_domains.categories_json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Analytics,
    Cdn,
    Ads,
    Social,
    FontService,
    TagManager,
    CloudStorage,
    VideoHost,
    Payments,
    Auth,
    Support,
    Maps,
    Captcha,
    Other,
}

impl Category {
    /// Stable string slug used in the JSON column. Matches the
    /// `#[serde(rename_all = "kebab-case")]` mapping above so manual
    /// callers stay in sync with serde-driven serialisation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Analytics => "analytics",
            Self::Cdn => "cdn",
            Self::Ads => "ads",
            Self::Social => "social",
            Self::FontService => "font-service",
            Self::TagManager => "tag-manager",
            Self::CloudStorage => "cloud-storage",
            Self::VideoHost => "video-host",
            Self::Payments => "payments",
            Self::Auth => "auth",
            Self::Support => "support",
            Self::Maps => "maps",
            Self::Captcha => "captcha",
            Self::Other => "other",
        }
    }
}

/// Heuristic vendor categorisation for an external host.
///
/// Pure function, deterministic, no I/O. Caller usually passes the
/// registrable domain (`to_domain` on `AssetRef`) but we also try the
/// raw host for the "exact host" entries in the table — that lets
/// `fonts.googleapis.com` register as both `FontService` (host match)
/// and not pollute every `googleapis.com` lookup with a font tag.
///
/// Returns categories sorted + deduplicated so JSON output is stable
/// across runs (operators diff these between crawls).
pub fn categorise(host: &str) -> Vec<Category> {
    use std::collections::BTreeSet;
    let host_lc = host.trim().to_ascii_lowercase();
    if host_lc.is_empty() {
        return Vec::new();
    }
    // We match against both the raw input (in case caller passed a full
    // host like `fonts.googleapis.com`) and its registrable domain.
    let registrable = registrable_domain(&host_lc).unwrap_or_else(|| host_lc.clone());

    let mut found: BTreeSet<Category> = BTreeSet::new();
    for (pattern, cats) in CATEGORY_TABLE {
        if matches_host(pattern, &host_lc, &registrable) {
            for c in *cats {
                found.insert(*c);
            }
        }
    }
    found.into_iter().collect()
}

/// Match logic used by `categorise()`. A pattern matches when:
///   * it equals the host exactly (`fonts.googleapis.com`), OR
///   * the host ends with `.{pattern}` (suffix match for subdomains), OR
///   * the registrable domain equals the pattern (covers
///     `www.facebook.com` → `facebook.com`).
///
/// Patterns that contain `/` (e.g. `google.com/recaptcha`) are matched
/// against the host's registrable domain only — we don't have a path
/// here, so they degrade to the `google.com` parent. That over-matches
/// captcha for any google.com URL, which is fine for a coarse rollup.
fn matches_host(pattern: &str, host: &str, registrable: &str) -> bool {
    // For path-style patterns, only the domain part matters at this layer.
    let domain_part = pattern.split('/').next().unwrap_or(pattern);
    if domain_part.is_empty() {
        return false;
    }
    if host == domain_part {
        return true;
    }
    if registrable == domain_part {
        return true;
    }
    // Suffix match for subdomains, e.g. `cdn.jsdelivr.net` matches `jsdelivr.net`.
    if host.len() > domain_part.len() + 1 && host.ends_with(domain_part) {
        let cut = host.len() - domain_part.len() - 1;
        if host.as_bytes()[cut] == b'.' {
            return true;
        }
    }
    false
}

/// Hardcoded vendor → category table. Order does not matter; duplicates
/// across rows are folded by the `BTreeSet` in `categorise()`. Add new
/// vendors here only — the rest of the pipeline picks them up
/// automatically.
const CATEGORY_TABLE: &[(&str, &[Category])] = &[
    // ----- Analytics -----
    ("google-analytics.com", &[Category::Analytics]),
    (
        "googletagmanager.com",
        &[Category::Analytics, Category::TagManager],
    ),
    ("amplitude.com", &[Category::Analytics]),
    ("mixpanel.com", &[Category::Analytics]),
    ("segment.com", &[Category::Analytics, Category::TagManager]),
    ("segment.io", &[Category::Analytics, Category::TagManager]),
    ("hotjar.com", &[Category::Analytics]),
    ("clarity.ms", &[Category::Analytics]),
    ("fullstory.com", &[Category::Analytics]),
    ("heap.io", &[Category::Analytics]),
    ("plausible.io", &[Category::Analytics]),
    ("matomo.org", &[Category::Analytics]),
    ("clicky.com", &[Category::Analytics]),
    ("tealium.com", &[Category::Analytics, Category::TagManager]),
    // ----- CDN -----
    ("cloudflare.com", &[Category::Cdn]),
    ("cloudfront.net", &[Category::Cdn]),
    ("fastly.net", &[Category::Cdn]),
    ("fastlylb.net", &[Category::Cdn]),
    ("akamai.net", &[Category::Cdn]),
    ("akamaihd.net", &[Category::Cdn]),
    ("akamaized.net", &[Category::Cdn]),
    ("jsdelivr.net", &[Category::Cdn]),
    ("unpkg.com", &[Category::Cdn]),
    ("bunnycdn.com", &[Category::Cdn]),
    ("keycdn.com", &[Category::Cdn]),
    ("cdn.jsdelivr.net", &[Category::Cdn]),
    ("cdnjs.cloudflare.com", &[Category::Cdn]),
    // ----- Ads -----
    ("doubleclick.net", &[Category::Ads]),
    ("googlesyndication.com", &[Category::Ads]),
    ("googleadservices.com", &[Category::Ads]),
    ("google.com/ads", &[Category::Ads]),
    ("adnxs.com", &[Category::Ads]),
    ("rubiconproject.com", &[Category::Ads]),
    ("criteo.com", &[Category::Ads]),
    ("taboola.com", &[Category::Ads]),
    ("outbrain.com", &[Category::Ads]),
    // ----- Social -----
    ("facebook.com", &[Category::Social]),
    ("fbcdn.net", &[Category::Social]),
    ("twitter.com", &[Category::Social]),
    ("x.com", &[Category::Social]),
    ("t.co", &[Category::Social]),
    ("linkedin.com", &[Category::Social]),
    ("instagram.com", &[Category::Social]),
    ("youtube.com", &[Category::Social, Category::VideoHost]),
    ("tiktok.com", &[Category::Social]),
    ("pinterest.com", &[Category::Social]),
    ("reddit.com", &[Category::Social]),
    ("discord.gg", &[Category::Social]),
    ("discord.com", &[Category::Social]),
    // ----- Font services -----
    ("fonts.googleapis.com", &[Category::FontService]),
    ("fonts.gstatic.com", &[Category::FontService]),
    ("use.typekit.net", &[Category::FontService]),
    ("kit.fontawesome.com", &[Category::FontService]),
    ("cloud.typenetwork.com", &[Category::FontService]),
    // ----- Tag managers (others alongside the analytics combos above) -----
    ("launchdarkly.com", &[Category::TagManager]),
    // ----- Cloud storage -----
    ("amazonaws.com", &[Category::CloudStorage]),
    ("s3.amazonaws.com", &[Category::CloudStorage]),
    ("storage.googleapis.com", &[Category::CloudStorage]),
    ("blob.core.windows.net", &[Category::CloudStorage]),
    // ----- Video hosts (additional, alongside youtube above) -----
    ("youtu.be", &[Category::VideoHost]),
    ("vimeo.com", &[Category::VideoHost]),
    ("wistia.com", &[Category::VideoHost]),
    ("jwplatform.com", &[Category::VideoHost]),
    // ----- Payments -----
    ("stripe.com", &[Category::Payments]),
    ("paypal.com", &[Category::Payments]),
    ("pagar.me", &[Category::Payments]),
    ("braintreegateway.com", &[Category::Payments]),
    ("adyen.com", &[Category::Payments]),
    ("checkout.com", &[Category::Payments]),
    // ----- Auth -----
    ("auth0.com", &[Category::Auth]),
    ("okta.com", &[Category::Auth]),
    ("onelogin.com", &[Category::Auth]),
    // ----- Support / chat -----
    ("intercom.io", &[Category::Support]),
    ("zendesk.com", &[Category::Support]),
    ("freshdesk.com", &[Category::Support]),
    ("drift.com", &[Category::Support]),
    ("tidio.com", &[Category::Support]),
    // ----- Maps -----
    ("google.com/maps", &[Category::Maps]),
    ("maps.googleapis.com", &[Category::Maps]),
    ("mapbox.com", &[Category::Maps]),
    // ----- Captcha -----
    ("google.com/recaptcha", &[Category::Captcha]),
    ("hcaptcha.com", &[Category::Captcha]),
    ("arkoselabs.com", &[Category::Captcha]),
    ("challenges.cloudflare.com", &[Category::Captcha]),
];

/// Coarse classification of an outbound reference. Mirrors the `kind`
/// column in the `asset_refs` table exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRefKind {
    Script,
    Style,
    Font,
    Image,
    Video,
    Audio,
    Iframe,
    Link, // `<a href>`, non-asset navigational link
    Turnstile,
    Other,
}

impl AssetRefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Style => "style",
            Self::Font => "font",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Iframe => "iframe",
            Self::Link => "link",
            Self::Turnstile => "turnstile",
            Self::Other => "other",
        }
    }
}

/// One outbound reference extracted from a page.
#[derive(Debug, Clone)]
pub struct AssetRef {
    pub from_page_url: String,
    pub to_url: String,
    /// Registrable domain of `to_url`. Empty when the URL has no host
    /// (data:, javascript:, mailto:, etc) — those rows are skipped by
    /// the persister so callers don't need to filter them explicitly.
    pub to_domain: String,
    pub kind: AssetRefKind,
    /// True when the `to_url` is within the caller's target scope
    /// (same registrable domain as `target_root`). Callers pass the
    /// target explicitly so this module never guesses.
    pub is_internal: bool,
}

/// Extract every classified asset reference from `html` at `base`,
/// tagging each as internal-to-`target_root` or external.
///
/// Fallbacks for the edge cases that come up in real HTML:
///   * `<img srcset>` splits into multiple URLs separated by `,` — we
///     emit one `AssetRef` per URL.
///   * `<link rel="…">` uses the rel attr to discriminate:
///     `rel=stylesheet` ⇒ `Style`, `rel=preload as=font` / anything
///     ending `.woff|woff2|ttf|otf` ⇒ `Font`, everything else ⇒
///     `Other` so an `<link rel="canonical">` still gets recorded.
pub fn extract_asset_refs(base: &Url, html: &str, target_root: &str) -> Vec<AssetRef> {
    let doc = Html::parse_document(html);
    extract_asset_refs_from_document(base, &doc, target_root)
}

pub fn extract_asset_refs_from_document(
    base: &Url,
    doc: &Html,
    target_root: &str,
) -> Vec<AssetRef> {
    let mut out: Vec<AssetRef> = Vec::new();
    let from_page_url = base.to_string();

    let push = |out: &mut Vec<AssetRef>, raw: &str, kind: AssetRefKind| {
        let Ok(u) = base.join(raw) else { return };
        // Skip non-http(s) schemes — data URIs, mailto, tel, javascript
        // are not outbound infra references and pollute the table.
        if !matches!(u.scheme(), "http" | "https") {
            return;
        }
        let host = match u.host_str() {
            Some(h) => h.to_ascii_lowercase(),
            None => return,
        };
        let to_domain = registrable_domain(&host).unwrap_or(host.clone());
        let is_internal = to_domain.eq_ignore_ascii_case(target_root);
        out.push(AssetRef {
            from_page_url: from_page_url.clone(),
            to_url: u.to_string(),
            to_domain,
            kind,
            is_internal,
        });
    };

    // Scripts.
    for el in doc.select(&SCRIPT_SRC_SELECTOR) {
        if let Some(src) = el.value().attr("src") {
            push(&mut out, src, AssetRefKind::Script);
        }
    }
    // Stylesheets + preload fonts + other <link rel=…> cases.
    for el in doc.select(&LINK_HREF_SELECTOR) {
        let Some(href) = el.value().attr("href") else {
            continue;
        };
        let rel = el.value().attr("rel").unwrap_or("").to_ascii_lowercase();
        let as_ = el.value().attr("as").unwrap_or("").to_ascii_lowercase();
        let is_stylesheet = rel.split_whitespace().any(|r| r == "stylesheet");
        let is_preload_font = rel.split_whitespace().any(|r| r == "preload") && as_ == "font";
        let looks_like_font = href_ends_with_font(href);
        let kind = if is_stylesheet {
            AssetRefKind::Style
        } else if is_preload_font || looks_like_font {
            AssetRefKind::Font
        } else {
            AssetRefKind::Other
        };
        push(&mut out, href, kind);
    }
    // Images (src + srcset).
    for el in doc.select(&IMG_SELECTOR) {
        if let Some(src) = el.value().attr("src") {
            push(&mut out, src, AssetRefKind::Image);
        }
        if let Some(set) = el.value().attr("srcset") {
            for candidate in split_srcset(set) {
                push(&mut out, candidate, AssetRefKind::Image);
            }
        }
    }
    // <source> inside picture/video/audio. Classified by parent type
    // via the type attribute when available; otherwise fall back to
    // Image (the common <picture> case).
    for el in doc.select(&SOURCE_SELECTOR) {
        let ty = el.value().attr("type").unwrap_or("").to_ascii_lowercase();
        let kind = if ty.starts_with("video/") {
            AssetRefKind::Video
        } else if ty.starts_with("audio/") {
            AssetRefKind::Audio
        } else {
            AssetRefKind::Image
        };
        if let Some(src) = el.value().attr("src") {
            push(&mut out, src, kind);
        }
        if let Some(set) = el.value().attr("srcset") {
            for candidate in split_srcset(set) {
                push(&mut out, candidate, kind);
            }
        }
    }
    // Iframes.
    for el in doc.select(&IFRAME_SRC_SELECTOR) {
        if let Some(src) = el.value().attr("src") {
            push(&mut out, src, AssetRefKind::Iframe);
        }
    }
    // Cloudflare Turnstile widget — typical embed is
    //   <div class="cf-turnstile" data-sitekey="0x4AAAA..."></div>
    // plus a loader script. We fingerprint the placeholder and carry
    // the sitekey in the URL fragment so it survives the (from,to,kind)
    // uniqueness index without extra columns.
    for el in doc.select(&DATA_SITEKEY_SELECTOR) {
        let class = el.value().attr("class").unwrap_or("");
        let is_turnstile = class
            .split_ascii_whitespace()
            .any(|c| c.eq_ignore_ascii_case("cf-turnstile"));
        if !is_turnstile {
            continue;
        }
        let Some(sitekey) = el.value().attr("data-sitekey").map(str::trim) else {
            continue;
        };
        if sitekey.is_empty() {
            continue;
        }
        let synthetic = format!(
            "https://challenges.cloudflare.com/turnstile/v0/api.js#sitekey={}",
            sitekey
        );
        push(&mut out, &synthetic, AssetRefKind::Turnstile);
    }
    // Video / audio elements' own src.
    for el in doc.select(&VIDEO_SRC_SELECTOR) {
        if let Some(src) = el.value().attr("src") {
            push(&mut out, src, AssetRefKind::Video);
        }
    }
    for el in doc.select(&AUDIO_SRC_SELECTOR) {
        if let Some(src) = el.value().attr("src") {
            push(&mut out, src, AssetRefKind::Audio);
        }
    }
    // Anchors (navigational links, not assets per se, but the operator
    // wants them so out-of-scope pages show up in external_domains).
    for el in doc.select(&A_HREF_SELECTOR) {
        if let Some(href) = el.value().attr("href") {
            push(&mut out, href, AssetRefKind::Link);
        }
    }

    // Dedupe on (to_url, kind) — the DB has a UNIQUE index on
    // (from_page_url, to_url, kind) but dropping the duplicates here
    // saves a write round-trip.
    out.sort_by(|a, b| a.to_url.cmp(&b.to_url).then(a.kind.cmp(&b.kind)));
    out.dedup_by(|a, b| a.to_url == b.to_url && a.kind == b.kind);
    out
}

impl PartialOrd for AssetRefKind {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for AssetRefKind {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

fn split_srcset(v: &str) -> impl Iterator<Item = &str> {
    // `srcset` entries are comma-separated URL pairs "url 1x, url 2x";
    // take the URL chunk (everything up to the first whitespace) per
    // candidate.
    v.split(',')
        .filter_map(|part| part.split_whitespace().next())
        .filter(|s| !s.is_empty())
}

fn href_ends_with_font(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    // Strip query+fragment so `/fonts/foo.woff2?v=1` still matches.
    let head = lower
        .split('?')
        .next()
        .unwrap_or(&lower)
        .split('#')
        .next()
        .unwrap_or(&lower);
    head.ends_with(".woff")
        || head.ends_with(".woff2")
        || head.ends_with(".ttf")
        || head.ends_with(".otf")
        || head.ends_with(".eot")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/page").unwrap()
    }

    #[test]
    fn classifies_script_style_image() {
        let html = r#"
            <html><head>
              <script src="https://cdn.example.com/app.js"></script>
              <link rel="stylesheet" href="/assets/site.css">
              <link rel="preload" as="font" href="/fonts/Inter.woff2">
            </head><body>
              <img src="https://images.fastly.net/x.jpg">
            </body></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        let by_kind: std::collections::HashMap<AssetRefKind, Vec<String>> =
            refs.iter()
                .fold(std::collections::HashMap::new(), |mut m, r| {
                    m.entry(r.kind).or_default().push(r.to_url.clone());
                    m
                });
        assert!(by_kind.contains_key(&AssetRefKind::Script));
        assert!(by_kind.contains_key(&AssetRefKind::Style));
        assert!(by_kind.contains_key(&AssetRefKind::Font));
        assert!(by_kind.contains_key(&AssetRefKind::Image));
    }

    #[test]
    fn internal_vs_external_flag() {
        let html = r#"
            <html><body>
              <a href="/about">about</a>
              <a href="https://sub.example.com/x">sub</a>
              <a href="https://other.net/y">other</a>
            </body></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        let map: std::collections::HashMap<String, bool> = refs
            .iter()
            .map(|r| (r.to_domain.clone(), r.is_internal))
            .collect();
        assert_eq!(map.get("example.com"), Some(&true));
        // `sub.example.com` has registrable domain `example.com` → internal.
        assert_eq!(map.get("other.net"), Some(&false));
    }

    #[test]
    fn skips_non_http_schemes() {
        let html = r#"
            <html><body>
              <a href="mailto:test@example.com">mail</a>
              <a href="javascript:void(0)">js</a>
              <img src="data:image/png;base64,iVBOR">
              <a href="https://real.test/">real</a>
            </body></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].to_domain, "real.test");
    }

    #[test]
    fn srcset_splits_candidates() {
        let html = r#"
            <html><body>
              <img srcset="https://a.test/one.jpg 1x, https://b.test/two.jpg 2x">
            </body></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        let domains: std::collections::HashSet<_> =
            refs.iter().map(|r| r.to_domain.clone()).collect();
        assert!(domains.contains("a.test"));
        assert!(domains.contains("b.test"));
    }

    #[test]
    fn dedupes_same_url_same_kind() {
        let html = r#"
            <html><body>
              <img src="https://a.test/x.jpg">
              <img src="https://a.test/x.jpg">
              <img src="https://a.test/x.jpg" alt="dup">
            </body></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn detects_turnstile_widget_with_sitekey() {
        let html = r#"
            <html><body>
              <div class="cf-turnstile" data-sitekey="0x4AAAAAAAdummySitekey"></div>
              <script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script>
            </body></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        let turnstile: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == AssetRefKind::Turnstile)
            .collect();
        assert_eq!(turnstile.len(), 1, "refs: {:?}", refs);
        assert!(
            turnstile[0]
                .to_url
                .contains("sitekey=0x4AAAAAAAdummySitekey"),
            "got: {}",
            turnstile[0].to_url
        );
        assert_eq!(turnstile[0].to_domain, "cloudflare.com");
    }

    #[test]
    fn turnstile_ignored_when_class_missing() {
        // data-sitekey on a non-Turnstile element (e.g. hCaptcha) must
        // not produce a Turnstile AssetRef.
        let html = r#"
            <html><body>
              <div class="h-captcha" data-sitekey="not-cf"></div>
            </body></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        assert!(refs.iter().all(|r| r.kind != AssetRefKind::Turnstile));
    }

    #[test]
    fn categorises_googletagmanager_as_analytics_and_tagmanager() {
        let cats = categorise("googletagmanager.com");
        assert!(cats.contains(&Category::Analytics), "got {:?}", cats);
        assert!(cats.contains(&Category::TagManager), "got {:?}", cats);
    }

    #[test]
    fn categorises_cloudfront_as_cdn() {
        // Both raw and a typical subdomain form should resolve to CDN.
        assert_eq!(categorise("cloudfront.net"), vec![Category::Cdn]);
        assert_eq!(categorise("d1234abcd.cloudfront.net"), vec![Category::Cdn]);
    }

    #[test]
    fn categorises_unknown_as_empty_vec() {
        assert!(categorise("some-random-vendor.example").is_empty());
        assert!(categorise("").is_empty());
    }

    #[test]
    fn exact_host_vs_suffix_match() {
        // Both should land on Social via the `facebook.com` entry, the
        // first as a direct registrable hit and the second via the
        // host-suffix path.
        assert_eq!(categorise("facebook.com"), vec![Category::Social]);
        assert_eq!(categorise("www.facebook.com"), vec![Category::Social]);
    }

    #[test]
    fn category_as_str_is_kebab_case() {
        // Guard the column-format contract: categories_json relies on
        // `as_str()` matching the serde rename rule.
        assert_eq!(Category::TagManager.as_str(), "tag-manager");
        assert_eq!(Category::FontService.as_str(), "font-service");
        assert_eq!(Category::Cdn.as_str(), "cdn");
    }

    #[test]
    fn font_detection_by_extension_and_rel() {
        let html = r#"
            <html><head>
              <link rel="preload" as="font" href="/a.woff2" crossorigin>
              <link rel="stylesheet" href="https://fonts.googleapis.com/css?family=Inter">
              <link rel="canonical" href="https://example.com/page">
              <link href="/bare.ttf">
            </head></html>
        "#;
        let refs = extract_asset_refs(&base(), html, "example.com");
        let by_kind: std::collections::HashMap<AssetRefKind, usize> =
            refs.iter()
                .fold(std::collections::HashMap::new(), |mut m, r| {
                    *m.entry(r.kind).or_insert(0) += 1;
                    m
                });
        // one Font for preload, one Font for bare .ttf
        assert_eq!(by_kind.get(&AssetRefKind::Font).copied().unwrap_or(0), 2);
        // one Style for googleapis css
        assert_eq!(by_kind.get(&AssetRefKind::Style).copied().unwrap_or(0), 1);
        // canonical is Other
        assert_eq!(by_kind.get(&AssetRefKind::Other).copied().unwrap_or(0), 1);
    }
}
