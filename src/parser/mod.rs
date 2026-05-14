// HTML parser foundation for v2 scraping framework.
//
// Two backends behind one surface:
//   * Tree mode (`parse_tree`)  → `scraper`/html5ever DOM for selectors,
//     adaptive matching, navigation.
//   * Streaming mode (`stream_rewrite`) → `lol_html` over bytes for
//     low-memory rewrite/extract paths.
//
// Charset detection priority: explicit `charset` arg → `<meta charset>` /
// `Content-Type` byte sniff → BOM → UTF-8 fallback with replacement chars.

use encoding_rs::{Encoding, UTF_8};

#[derive(Debug, thiserror::Error)]
pub enum ParserError {
    #[error("streaming rewrite error: {0}")]
    Rewriting(String),
    #[error("encoding error: {0}")]
    Encoding(String),
}

/// Owned tree handle. Holds the decoded source so callers can borrow
/// `ElementRef`s without lifetime juggling on the bytes side.
pub struct TreeHandle {
    html: scraper::Html,
    source: String,
    encoding: &'static Encoding,
}

impl TreeHandle {
    pub fn html(&self) -> &scraper::Html {
        &self.html
    }

    pub fn root_element(&self) -> scraper::ElementRef<'_> {
        self.html.root_element()
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn encoding(&self) -> &'static Encoding {
        self.encoding
    }
}

/// Parse bytes into a navigable tree. `charset` is an optional label
/// (e.g. `"utf-8"`, `"shift_jis"`, `"latin1"`). Unknown labels fall back
/// to UTF-8.
pub fn parse_tree(bytes: &[u8], charset: Option<&str>) -> TreeHandle {
    let (source, encoding) = decode(bytes, charset);
    let html = scraper::Html::parse_document(&source);
    TreeHandle { html, source, encoding }
}

/// Streaming rewrite over bytes. Wraps `lol_html::rewrite_str`-style
/// API but operates on raw bytes so callers can pipe arbitrary input.
pub fn stream_rewrite(
    bytes: &[u8],
    settings: lol_html::Settings<'_, '_>,
) -> Result<Vec<u8>, ParserError> {
    let mut output: Vec<u8> = Vec::with_capacity(bytes.len());
    {
        let mut rewriter = lol_html::HtmlRewriter::new(settings, |c: &[u8]| {
            output.extend_from_slice(c);
        });
        rewriter
            .write(bytes)
            .map_err(|e| ParserError::Rewriting(e.to_string()))?;
        rewriter
            .end()
            .map_err(|e| ParserError::Rewriting(e.to_string()))?;
    }
    Ok(output)
}

fn decode(bytes: &[u8], charset: Option<&str>) -> (String, &'static Encoding) {
    let explicit = charset.and_then(|c| Encoding::for_label(c.as_bytes()));
    let sniffed = explicit
        .or_else(|| sniff_meta_charset(bytes))
        .or_else(|| bom_encoding(bytes))
        .unwrap_or(UTF_8);
    let (cow, used, _had_malformed) = sniffed.decode(bytes);
    (cow.into_owned(), used)
}

fn bom_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some(encoding_rs::UTF_8)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some(encoding_rs::UTF_16BE)
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some(encoding_rs::UTF_16LE)
    } else {
        None
    }
}

// Cheap byte-level sniff for `<meta charset=...>` or
// `<meta http-equiv="Content-Type" content="...; charset=...">` within
// the first 1024 bytes. Conservative — anything weird returns None.
fn sniff_meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    let head = &bytes[..bytes.len().min(1024)];
    let lower: String = head.iter().map(|b| b.to_ascii_lowercase() as char).collect();
    let idx = lower.find("charset")?;
    let rest = &lower[idx + "charset".len()..];
    let rest = rest.trim_start_matches([' ', '=', '"', '\'']);
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c == ' ' || c == ';' || c == '>' || c == '/')
        .unwrap_or(rest.len());
    let label = &rest[..end];
    if label.is_empty() {
        None
    } else {
        Encoding::for_label(label.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lol_html::{element, Settings};
    use scraper::Selector;

    #[test]
    fn parse_tree_well_formed() {
        let html = b"<!doctype html><html><body><p class='x'>hi</p></body></html>";
        let h = parse_tree(html, None);
        let sel = Selector::parse("p.x").unwrap();
        let el = h.html().select(&sel).next().unwrap();
        assert_eq!(el.text().collect::<String>(), "hi");
        assert_eq!(h.encoding(), UTF_8);
    }

    #[test]
    fn parse_tree_malformed_no_panic() {
        // Unclosed tags, stray quotes, CDATA-looking junk.
        let html = b"<html><body><div><p>oops<span attr=\"x\
            <![CDATA[stuff]]></body";
        let h = parse_tree(html, None);
        // Tree exists; html5ever fixed it up.
        assert!(h.html().root_element().children().count() > 0);
    }

    #[test]
    fn parse_tree_latin1_via_explicit_label() {
        // 0xE9 == 'é' in latin-1.
        let bytes: &[u8] = b"<html><body><p>caf\xE9</p></body></html>";
        let h = parse_tree(bytes, Some("latin1"));
        let sel = Selector::parse("p").unwrap();
        let el = h.html().select(&sel).next().unwrap();
        assert_eq!(el.text().collect::<String>(), "café");
    }

    #[test]
    fn parse_tree_sjis_via_meta_sniff() {
        // <meta charset> tells the parser; bytes are shift_jis-encoded
        // "日本" (4 bytes: 93 fa 96 7b).
        let mut bytes: Vec<u8> = b"<html><head><meta charset=\"shift_jis\"></head><body><p>"
            .to_vec();
        bytes.extend_from_slice(&[0x93, 0xFA, 0x96, 0x7B]);
        bytes.extend_from_slice(b"</p></body></html>");
        let h = parse_tree(&bytes, None);
        let sel = Selector::parse("p").unwrap();
        let el = h.html().select(&sel).next().unwrap();
        assert_eq!(el.text().collect::<String>(), "日本");
    }

    #[test]
    fn parse_tree_utf8_bom() {
        let mut bytes: Vec<u8> = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"<p>x</p>");
        let h = parse_tree(&bytes, None);
        assert_eq!(h.encoding(), UTF_8);
    }

    #[test]
    fn stream_rewrite_basic() {
        let input = b"<a href=\"old\">link</a>";
        let out = stream_rewrite(
            input,
            Settings {
                element_content_handlers: vec![element!("a[href]", |el| {
                    el.set_attribute("href", "new").unwrap();
                    Ok(())
                })],
                ..Settings::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("href=\"new\""));
        assert!(!s.contains("old"));
    }

    #[test]
    fn stream_rewrite_handler_error_propagates() {
        let input = b"<x>";
        // Force handler to fail.
        let err = stream_rewrite(
            input,
            Settings {
                element_content_handlers: vec![element!("x", |_el| {
                    Err("boom".into())
                })],
                ..Settings::default()
            },
        )
        .unwrap_err();
        matches!(err, ParserError::Rewriting(_));
    }
}
