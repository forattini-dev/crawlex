//! HTML cleaner for "main content only" extraction.
//!
//! Ported from Firecrawl `apps/api/native/src/html.rs::_transform_html_inner`
//! (MIT). We keep the cleaner but drop Firecrawl's OMCE cross-page signature
//! machinery and metadata/image extraction — those belong to the Phase 5
//! ExtractSpec subsystem and would inflate this module past its one job.
//!
//! What the cleaner does, in order:
//!   1. Parse with kuchikiki.
//!   2. Strip `<head>`, `<meta>`, `<noscript>`, `<style>`, `<script>`.
//!   3. Strip caller-provided `exclude_tags`.
//!   4. If `only_main_content`, strip every element matching
//!      `EXCLUDE_NON_MAIN_TAGS` unless it contains a descendant matching
//!      `FORCE_INCLUDE_MAIN_TAGS` (so we don't nuke a wrapped main).
//!   5. For `<img srcset>`, pick the biggest candidate and put it in `src`.
//!   6. Resolve relative `href` and `src` against the document URL.
//!   7. Serialize back to string.
//!
//! Useful afterwards:
//!   * [`remove_skip_to_content_links`] — strip the very common `[Skip to
//!     Content](#...)` markdown link that RAG ingest sees as noise.

use kuchikiki::{parse_html, traits::TendrilSink};
use url::Url;

/// 42 CSS selectors Firecrawl curated to trim page chrome (header, footer,
/// nav, sidebars, ads, cookie bars, share widgets, breadcrumbs). Kept
/// verbatim from the upstream list so behaviour stays bug-compatible.
pub const EXCLUDE_NON_MAIN_TAGS: &[&str] = &[
    "header",
    "footer",
    "nav",
    "aside",
    ".header",
    ".top",
    ".navbar",
    "#header",
    ".footer",
    ".bottom",
    "#footer",
    ".sidebar",
    ".side",
    ".aside",
    "#sidebar",
    ".modal",
    ".popup",
    "#modal",
    ".overlay",
    ".ad",
    ".ads",
    ".advert",
    "#ad",
    ".lang-selector",
    ".language",
    "#language-selector",
    ".social",
    ".social-media",
    ".social-links",
    "#social",
    ".menu",
    ".navigation",
    "#nav",
    ".breadcrumbs",
    "#breadcrumbs",
    ".share",
    "#share",
    ".widget",
    "#widget",
    ".cookie",
    "#cookie",
    ".fc-decoration",
];

/// 13 selectors that, when present as descendants of an
/// `EXCLUDE_NON_MAIN_TAGS` match, preserve the ancestor. Prevents stripping
/// a `#main` that happens to be wrapped in a `header` (yes, sites do this).
pub const FORCE_INCLUDE_MAIN_TAGS: &[&str] = &[
    "#main",
    ".swoogo-cols",
    ".swoogo-text",
    ".swoogo-table-div",
    ".swoogo-space",
    ".swoogo-alert",
    ".swoogo-sponsors",
    ".swoogo-title",
    ".swoogo-tabs",
    ".swoogo-logo",
    ".swoogo-image",
    ".swoogo-button",
    ".swoogo-agenda",
];

pub struct CleanOptions<'a> {
    /// Base URL of the document — used to resolve relative `src`/`href`.
    pub url: &'a str,
    /// Extra CSS selectors to strip beyond the defaults.
    pub exclude_tags: &'a [&'a str],
    /// If true, apply the curated `EXCLUDE_NON_MAIN_TAGS` strip pass.
    pub only_main_content: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CleanError {
    #[error("url parse: {0}")]
    Url(#[from] url::ParseError),
    #[error("selector: {0}")]
    Selector(String),
}

/// Transform HTML into a main-content-only document with resolved URLs.
/// Returns the serialized HTML.
pub fn clean_html(html: &str, opts: &CleanOptions<'_>) -> Result<String, CleanError> {
    let document = parse_html().one(html);
    let url = Url::parse(opts.url)?;

    // Strip well-known non-content tags.
    for tag in ["head", "meta", "noscript", "style", "script"] {
        while let Ok(hit) = document.select_first(tag) {
            hit.as_node().detach();
        }
    }

    // Caller-supplied extras.
    for sel in opts.exclude_tags {
        while let Ok(hit) = document.select_first(sel) {
            hit.as_node().detach();
        }
    }

    // Main-content strip: drop each match unless it contains a "force
    // include" descendant.
    if opts.only_main_content {
        for outer in EXCLUDE_NON_MAIN_TAGS.iter() {
            let matches: Vec<_> = document
                .select(outer)
                .map_err(|_| CleanError::Selector((*outer).to_string()))?
                .collect();
            for node in matches {
                let keep = FORCE_INCLUDE_MAIN_TAGS.iter().any(|inner| {
                    node.as_node()
                        .select(inner)
                        .is_ok_and(|mut it| it.next().is_some())
                });
                if !keep {
                    node.as_node().detach();
                }
            }
        }
    }

    // Resolve `<img srcset>` to the largest candidate.
    resolve_srcset(&document);

    // Resolve relative `src` and `href` against the document URL.
    resolve_attr(&document, "img[src]", "src", &url);
    resolve_attr(&document, "a[href]", "href", &url);

    Ok(document.to_string())
}

fn resolve_srcset(document: &kuchikiki::NodeRef) {
    let Ok(iter) = document.select("img[srcset]") else {
        return;
    };
    let imgs: Vec<_> = iter.collect();
    for img in imgs {
        let attrs = img.attributes.borrow();
        let Some(raw) = attrs.get("srcset") else {
            continue;
        };
        let mut sources: Vec<(String, f64, bool)> = raw
            .split(',')
            .filter_map(|x| {
                let tok: Vec<&str> = x.split_whitespace().collect();
                if tok.is_empty() {
                    return None;
                }
                let last = *tok.last()?;
                let (last, used) = if tok.len() > 1
                    && !last.is_empty()
                    && (last.ends_with('x') || last.ends_with('w'))
                {
                    (last, true)
                } else {
                    ("1x", false)
                };
                // Parse "2x" or "1200w" — drop the unit char.
                let unit_idx = last.char_indices().last()?.0;
                let size: f64 = last[..unit_idx].parse().ok()?;
                let url_part = if used {
                    tok[..tok.len() - 1].join(" ")
                } else {
                    tok.join(" ")
                };
                Some((url_part, size, last.ends_with('x')))
            })
            .collect();
        if sources.iter().all(|(_, _, is_x)| *is_x) {
            if let Some(src) = attrs.get("src") {
                sources.push((src.to_string(), 1.0, true));
            }
        }
        drop(attrs);
        sources.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(best) = sources.first() {
            img.attributes.borrow_mut().insert("src", best.0.clone());
        }
    }
}

fn resolve_attr(document: &kuchikiki::NodeRef, selector: &str, attr: &str, base: &Url) {
    let Ok(iter) = document.select(selector) else {
        return;
    };
    for node in iter {
        let old = {
            let a = node.attributes.borrow();
            match a.get(attr) {
                Some(s) => s.to_string(),
                None => continue,
            }
        };
        if let Ok(new) = base.join(&old) {
            node.attributes.borrow_mut().insert(attr, new.to_string());
        }
    }
}

/// Strip the common "[Skip to Content](#main)" markdown link that crawlers
/// leave behind after Turndown-style HTML→markdown conversion. Case-insensitive
/// on the label; targets only anchors whose href starts with `#`.
pub fn remove_skip_to_content_links(input: &str) -> String {
    const LABEL: &str = "Skip to Content";
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;

    'outer: while i < len {
        if bytes[i] == b'[' {
            let label_start = i + 1;
            let label_end = label_start + LABEL.len();
            if label_end <= len && bytes[label_start..label_end].iter().all(|b| b.is_ascii()) {
                let label_slice = &input[label_start..label_end];
                if label_slice.eq_ignore_ascii_case(LABEL)
                    && label_end + 3 <= len
                    && bytes[label_end] == b']'
                    && bytes[label_end + 1] == b'('
                    && bytes[label_end + 2] == b'#'
                {
                    let mut j = label_end + 3;
                    while j < len {
                        let ch = input[j..].chars().next().unwrap();
                        if ch == ')' {
                            i = j + ch.len_utf8();
                            continue 'outer;
                        }
                        j += ch.len_utf8();
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}
