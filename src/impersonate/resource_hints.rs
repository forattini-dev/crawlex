//! Honor `<link rel="dns-prefetch|preconnect|preload|modulepreload">`
//! resource hints the way real Chrome does.
//!
//! Chrome's loader inspects the parsed HTML head, and for each hint
//! emits exactly one of:
//! * `dns-prefetch` → a DNS A/AAAA lookup that **warms the resolver
//!   cache** but does NOT open a socket. Cheap and safe.
//! * `preconnect` → DNS + TCP + (if https) TLS handshake, leaving an
//!   idle connection in the pool. The next actual request reuses it.
//! * `preload` / `modulepreload` → a real subresource fetch scheduled
//!   ahead of the parser.
//!
//! A crawler that ignores these hints emits a request sequence that is
//! out-of-order vs. a real Chrome trace for the same page: the detector
//! sees us fetch `/main.js` after `/index.html` but never saw the DNS
//! warm-up or the preconnect handshake that Chrome would emit ahead of
//! the parser. That divergence is a fingerprint.
//!
//! Scope of this module: **parsing only**. We return a structured
//! `ResourceHints` payload; the crawler layer decides whether to schedule
//! lookups/fetches. Keeping the side-effect-free parser separate lets
//! us unit-test the hint extraction without touching the connection
//! pool, the DNS cache, or the frontier.

use scraper::{Html, Selector};
use url::Url;

/// One `<link rel=…>` hint extracted from the HTML head, resolved against
/// the page's base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceHint {
    pub kind: HintKind,
    /// Absolute URL. For `dns-prefetch` / `preconnect` the path is
    /// typically just `/`; what matters is the host+port for DNS /
    /// connection warming.
    pub url: Url,
    /// `as` attribute when present (preload only). Informational —
    /// tells the scheduler whether the hint should turn into a Script,
    /// Style, Font, Image, etc. fetch.
    pub as_: Option<String>,
    /// `crossorigin` attribute literal (`anonymous`, `use-credentials`,
    /// or empty string for bare `crossorigin`). Preserved because it
    /// affects the connection-coalescing bucket Chrome puts the hint
    /// in — a CORS preconnect warms a different connection than a
    /// same-origin one.
    pub crossorigin: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintKind {
    /// DNS lookup only, no socket.
    DnsPrefetch,
    /// DNS + TCP + (https: TLS), idle connection parked in pool.
    Preconnect,
    /// Full GET scheduled ahead of the parser.
    Preload,
    /// Like preload but for ES modules (emits `sec-fetch-dest: script`).
    ModulePreload,
}

impl HintKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HintKind::DnsPrefetch => "dns-prefetch",
            HintKind::Preconnect => "preconnect",
            HintKind::Preload => "preload",
            HintKind::ModulePreload => "modulepreload",
        }
    }
}

/// All resource hints found in a parsed document, grouped by kind so a
/// caller can batch the DNS prefetches separately from preconnects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceHints {
    pub all: Vec<ResourceHint>,
}

impl ResourceHints {
    /// Iterator over hints of a given kind. Avoids a double-walk when
    /// the caller only cares about DNS prefetches (or only preconnects).
    pub fn of(&self, kind: HintKind) -> impl Iterator<Item = &ResourceHint> {
        self.all.iter().filter(move |h| h.kind == kind)
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }
}

/// Parse `<link rel="...">` hints from `html`, resolving relative
/// `href`s against `base`. Unknown or unsupported `rel` tokens are
/// ignored silently — the extractor is best-effort and HTML in the
/// wild carries a lot of noise (`rel="shortcut icon"` etc.).
///
/// One `<link>` element can carry space-separated `rel` tokens; we
/// emit one hint per recognized token so `<link rel="preload
/// modulepreload">` produces two entries. That matches Chrome's loader
/// which iterates rel tokens independently.
pub fn extract_resource_hints(base: &Url, html: &str) -> ResourceHints {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    let sel = match Selector::parse("link[rel][href]") {
        Ok(s) => s,
        Err(_) => return ResourceHints::default(),
    };
    for el in doc.select(&sel) {
        let rel = el.value().attr("rel").unwrap_or("").to_ascii_lowercase();
        let href = match el.value().attr("href") {
            Some(h) if !h.is_empty() => h,
            _ => continue,
        };
        let resolved = match base.join(href) {
            Ok(u) => u,
            Err(_) => continue,
        };
        // Only http/https hints — javascript:, data:, mailto: etc. are
        // not network-warming candidates.
        if !matches!(resolved.scheme(), "http" | "https") {
            continue;
        }
        let as_ = el
            .value()
            .attr("as")
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty());
        let crossorigin = el
            .value()
            .attr("crossorigin")
            .map(|s| s.trim().to_ascii_lowercase());

        for token in rel.split_ascii_whitespace() {
            let kind = match token {
                "dns-prefetch" => HintKind::DnsPrefetch,
                "preconnect" => HintKind::Preconnect,
                "preload" => HintKind::Preload,
                "modulepreload" => HintKind::ModulePreload,
                _ => continue,
            };
            out.push(ResourceHint {
                kind,
                url: resolved.clone(),
                as_: as_.clone(),
                crossorigin: crossorigin.clone(),
            });
        }
    }
    ResourceHints { all: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.test/page").unwrap()
    }

    #[test]
    fn extracts_dns_prefetch_and_preconnect() {
        let html = r#"
            <!doctype html><html><head>
              <link rel="dns-prefetch" href="//cdn.example.com">
              <link rel="preconnect" href="https://api.example.com" crossorigin>
            </head><body></body></html>
        "#;
        let hints = extract_resource_hints(&base(), html);
        assert_eq!(hints.len(), 2);
        let dp: Vec<_> = hints.of(HintKind::DnsPrefetch).collect();
        assert_eq!(dp.len(), 1);
        assert_eq!(dp[0].url.host_str(), Some("cdn.example.com"));
        let pc: Vec<_> = hints.of(HintKind::Preconnect).collect();
        assert_eq!(pc.len(), 1);
        assert_eq!(pc[0].url.host_str(), Some("api.example.com"));
        assert_eq!(pc[0].crossorigin.as_deref(), Some(""));
    }

    #[test]
    fn preload_with_as_attribute() {
        let html = r#"
            <html><head>
              <link rel="preload" href="/main.js" as="script">
              <link rel="modulepreload" href="/m.mjs">
            </head></html>
        "#;
        let hints = extract_resource_hints(&base(), html);
        assert_eq!(hints.len(), 2);
        let pl: Vec<_> = hints.of(HintKind::Preload).collect();
        assert_eq!(pl.len(), 1);
        assert_eq!(pl[0].as_.as_deref(), Some("script"));
        assert_eq!(pl[0].url.as_str(), "https://example.test/main.js");
        let mp: Vec<_> = hints.of(HintKind::ModulePreload).collect();
        assert_eq!(mp.len(), 1);
        assert_eq!(mp[0].url.as_str(), "https://example.test/m.mjs");
    }

    #[test]
    fn space_separated_rel_tokens_emit_multiple_hints() {
        // A single <link> with two tokens becomes two hints — Chrome's
        // loader treats them independently.
        let html = r#"
            <html><head>
              <link rel="preload modulepreload" href="/dual.mjs" as="script">
            </head></html>
        "#;
        let hints = extract_resource_hints(&base(), html);
        assert_eq!(hints.len(), 2);
        let kinds: Vec<_> = hints.all.iter().map(|h| h.kind).collect();
        assert!(kinds.contains(&HintKind::Preload));
        assert!(kinds.contains(&HintKind::ModulePreload));
    }

    #[test]
    fn ignores_non_http_and_unknown_rel() {
        let html = r#"
            <html><head>
              <link rel="icon" href="/favicon.ico">
              <link rel="stylesheet" href="/a.css">
              <link rel="preload" href="javascript:alert(1)">
              <link rel="preload" href="data:text/plain,abc">
              <link rel="preconnect" href="">
            </head></html>
        "#;
        let hints = extract_resource_hints(&base(), html);
        // icon/stylesheet are not hints we care about here; javascript:/data:
        // schemes are filtered; empty href is filtered.
        assert!(hints.is_empty(), "unexpected hints: {:?}", hints.all);
    }

    #[test]
    fn relative_and_protocol_relative_href_resolve() {
        let html = r#"
            <html><head>
              <link rel="dns-prefetch" href="//cdn.a.test/">
              <link rel="preconnect" href="/api">
            </head></html>
        "#;
        let hints = extract_resource_hints(&base(), html);
        assert_eq!(hints.len(), 2);
        // `//host/` resolves using the base scheme (https).
        assert_eq!(hints.all[0].url.scheme(), "https");
        assert_eq!(hints.all[0].url.host_str(), Some("cdn.a.test"));
        // `/api` resolves against the base host.
        assert_eq!(hints.all[1].url.as_str(), "https://example.test/api");
    }
}
