//! Extractor seam (slice 2 of the JobRunner extraction, GH #18).
//!
//! Today this is a thin wrapper around `discovery::links` — its job is to
//! pin the seam, not to add behavior. Asset classification flows
//! through here in a later slice once it stops being entangled with the
//! tech-fingerprint pipeline in `crawler.rs`.
//!
//! Rationale: concrete struct (no trait). One implementation, pure
//! logic — slice 1 of PRD #15 explicitly rejects a hypothetical seam
//! for the extractor.

use scraper::Html;
use url::Url;

/// Per-Job extraction surface. Stateless; cheap to construct.
#[derive(Debug, Default, Clone, Copy)]
pub struct Extractor;

impl Extractor {
    pub fn new() -> Self {
        Self
    }

    /// Parse `html` and extract outgoing links relative to `base`. Heavy
    /// HTML parse is the caller's job to run on a blocking thread when
    /// it's hot — same convention `crawler.rs` already follows.
    pub fn extract_links(&self, base: &Url, html: &str) -> Vec<Url> {
        crate::discovery::links::extract_links(base, html)
    }

    /// Same as `extract_links` but reuses a pre-parsed `Html` document
    /// — used by paths that already parsed to run multiple analyses
    /// (tech fingerprint, asset refs) over the same DOM.
    pub fn extract_links_from_document(&self, base: &Url, doc: &Html) -> Vec<Url> {
        crate::discovery::links::extract_links_from_document(base, doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/posts/42").unwrap()
    }

    #[test]
    fn resolves_relative_anchor() {
        let html = r#"<html><body><a href="/about">about</a></body></html>"#;
        let links = Extractor::new().extract_links(&base(), html);
        assert!(links.iter().any(|u| u.as_str() == "https://example.com/about"));
    }

    #[test]
    fn empty_document_yields_no_links() {
        let html = "<html><body></body></html>";
        let links = Extractor::new().extract_links(&base(), html);
        assert!(links.is_empty(), "no anchors → no links, got {links:?}");
    }

    #[test]
    fn parity_with_underlying_discovery_links() {
        // Wrapper must delegate 1:1 to `discovery::links::extract_links`.
        // If this drifts, the Extractor stopped being a pure pass-through
        // — that's a behavior change and needs an ADR, not a silent diff.
        let html = r#"<html><body>
            <a href="/a">a</a>
            <a href="https://other.example/b">b</a>
            <a href="?q=c">c</a>
        </body></html>"#;
        let via_extractor = Extractor::new().extract_links(&base(), html);
        let via_discovery = crate::discovery::links::extract_links(&base(), html);
        assert_eq!(via_extractor, via_discovery);
    }

    #[test]
    fn document_path_matches_html_path() {
        let html = r#"<html><body><a href="/p/1">a</a><a href="/p/2">b</a></body></html>"#;
        let parsed = Html::parse_document(html);
        let from_html = Extractor::new().extract_links(&base(), html);
        let from_doc = Extractor::new().extract_links_from_document(&base(), &parsed);
        assert_eq!(from_html, from_doc);
    }

    #[test]
    fn extractor_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Extractor>();
    }
}
