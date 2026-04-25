//! Tests for the sitemap XML processor ported from Firecrawl.

use crawlex::extract::sitemap::{process_sitemap, SitemapAction, SitemapError};

#[test]
fn urlset_terminal_urls_classified_as_process() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/a</loc></url>
  <url><loc>https://example.com/b</loc></url>
</urlset>"#;
    let r = process_sitemap(xml).unwrap();
    assert_eq!(r.total_count, 2);
    assert_eq!(r.instructions.len(), 1);
    assert_eq!(r.instructions[0].action, SitemapAction::Process);
    assert_eq!(r.instructions[0].urls.len(), 2);
}

#[test]
fn sitemapindex_child_urls_classified_as_recurse() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>https://example.com/sitemap-1.xml</loc></sitemap>
  <sitemap><loc>https://example.com/sitemap-2.xml</loc></sitemap>
</sitemapindex>"#;
    let r = process_sitemap(xml).unwrap();
    assert_eq!(r.total_count, 2);
    assert_eq!(r.instructions[0].action, SitemapAction::Recurse);
    assert_eq!(r.instructions[0].urls.len(), 2);
}

#[test]
fn urlset_with_nested_xml_entries_splits_recurse_and_process() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/a</loc></url>
  <url><loc>https://example.com/sub-sitemap.xml</loc></url>
  <url><loc>https://example.com/b</loc></url>
</urlset>"#;
    let r = process_sitemap(xml).unwrap();
    assert_eq!(r.total_count, 3);
    // Two instruction groups: recurse (.xml entries), process (terminal).
    assert_eq!(r.instructions.len(), 2);
    let recurse = r
        .instructions
        .iter()
        .find(|i| i.action == SitemapAction::Recurse)
        .unwrap();
    let process = r
        .instructions
        .iter()
        .find(|i| i.action == SitemapAction::Process)
        .unwrap();
    assert_eq!(recurse.urls.len(), 1);
    assert_eq!(process.urls.len(), 2);
}

#[test]
fn urlset_drops_file_ext_urls() {
    // A `.png` under <url><loc> should NOT be surfaced as a process target.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/article</loc></url>
  <url><loc>https://example.com/logo.png</loc></url>
</urlset>"#;
    let r = process_sitemap(xml).unwrap();
    assert_eq!(r.instructions.len(), 1);
    assert_eq!(r.instructions[0].action, SitemapAction::Process);
    assert_eq!(r.instructions[0].urls, vec!["https://example.com/article"]);
}

#[test]
fn invalid_root_element_errs() {
    let xml = r#"<?xml version="1.0"?><nope/>"#;
    match process_sitemap(xml) {
        Err(SitemapError::InvalidRoot(tag)) => assert_eq!(tag, "nope"),
        other => panic!("expected InvalidRoot, got {:?}", other),
    }
}

#[test]
fn malformed_xml_errs() {
    let xml = "not xml at all <>";
    assert!(matches!(process_sitemap(xml), Err(SitemapError::Parse(_))));
}

#[test]
fn empty_urlset_yields_zero() {
    let xml = r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"/>"#;
    let r = process_sitemap(xml).unwrap();
    assert_eq!(r.total_count, 0);
    assert_eq!(r.instructions.len(), 0);
}

#[test]
fn xml_gz_urls_also_classified_as_recurse() {
    let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/big.xml.gz</loc></url>
</urlset>"#;
    let r = process_sitemap(xml).unwrap();
    assert_eq!(r.instructions.len(), 1);
    assert_eq!(r.instructions[0].action, SitemapAction::Recurse);
}
