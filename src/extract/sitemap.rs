//! XML sitemap + sitemapindex parser.
//!
//! Ported from Firecrawl `apps/api/native/src/crawler.rs` (MIT). Emits a
//! structured `SitemapProcessingResult` so callers can decide between
//! "recurse into another .xml" and "process these URLs as regular jobs".
//!
//! Handles:
//!   * `<urlset>` with zero or more `<url><loc>`.
//!   * `<sitemapindex>` with zero or more `<sitemap><loc>` — each entry
//!     points to another XML to fetch.
//!   * Nested `.xml` / `.xml.gz` entries inside `<urlset>`: surfaced as
//!     `"recurse"` instructions instead of `"process"`.

use roxmltree::Document;
use url::Url;

use crate::extract::link_filter::is_file;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitemapAction {
    /// URL is another sitemap XML — follow it recursively.
    Recurse,
    /// URL is a terminal page — push it to the crawl queue.
    Process,
}

#[derive(Debug, Clone)]
pub struct SitemapInstruction {
    pub action: SitemapAction,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SitemapProcessingResult {
    pub instructions: Vec<SitemapInstruction>,
    pub total_count: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SitemapError {
    #[error("XML parse error: {0}")]
    Parse(String),
    #[error("invalid sitemap: root must be 'urlset' or 'sitemapindex', got '{0}'")]
    InvalidRoot(String),
}

/// Parse a sitemap XML and emit crawl instructions. See the docs on
/// [`SitemapAction`] for the two output kinds.
pub fn process_sitemap(xml: &str) -> Result<SitemapProcessingResult, SitemapError> {
    let parse_opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = Document::parse_with_options(xml, parse_opts)
        .map_err(|e| SitemapError::Parse(e.to_string()))?;
    let root = doc.root_element();

    let mut instructions = Vec::new();
    let mut total_count: u32 = 0;

    match root.tag_name().name() {
        "sitemapindex" => {
            let sitemap_urls: Vec<String> = root
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "sitemap")
                .filter_map(|n| loc_of(&n))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !sitemap_urls.is_empty() {
                total_count += sitemap_urls.len() as u32;
                instructions.push(SitemapInstruction {
                    action: SitemapAction::Recurse,
                    urls: sitemap_urls,
                });
            }
        }
        "urlset" => {
            let mut xml_sitemaps: Vec<String> = Vec::new();
            let mut valid_urls: Vec<String> = Vec::new();
            for n in root
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "url")
            {
                let Some(loc) = loc_of(&n) else { continue };
                let loc = loc.trim();
                if loc.is_empty() {
                    continue;
                }
                let lower = loc.to_ascii_lowercase();
                if lower.ends_with(".xml") || lower.ends_with(".xml.gz") {
                    xml_sitemaps.push(loc.to_string());
                } else if let Ok(parsed) = Url::parse(loc) {
                    let path_lower = parsed.path().to_ascii_lowercase();
                    if !is_file(&path_lower) {
                        valid_urls.push(loc.to_string());
                    }
                }
            }
            if !xml_sitemaps.is_empty() {
                total_count += xml_sitemaps.len() as u32;
                instructions.push(SitemapInstruction {
                    action: SitemapAction::Recurse,
                    urls: xml_sitemaps,
                });
            }
            if !valid_urls.is_empty() {
                total_count += valid_urls.len() as u32;
                instructions.push(SitemapInstruction {
                    action: SitemapAction::Process,
                    urls: valid_urls,
                });
            }
        }
        other => return Err(SitemapError::InvalidRoot(other.to_string())),
    }

    Ok(SitemapProcessingResult {
        instructions,
        total_count,
    })
}

fn loc_of<'a, 'input>(node: &roxmltree::Node<'a, 'input>) -> Option<&'a str> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == "loc")
        .and_then(|n| n.text())
}
