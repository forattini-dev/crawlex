//! Tests for the Firecrawl-ported link filter.
//!
//! Covers the denial matrix (file-ext, depth, protocol, social, robots,
//! external scope, subdomain allow, regex include/exclude).

use crawlex::extract::link_filter::{
    filter_links, get_url_depth, is_file, is_internal_link, is_non_web_protocol,
    is_social_media_or_email, is_subdomain, no_sections, FilterLinksInput,
};
use regex::Regex;
use url::Url;

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn is_file_matches_known_extensions() {
    assert!(is_file("/a/b/bundle.js"));
    assert!(is_file("/logo.PNG")); // case-insensitive
    assert!(is_file("/x.webp"));
    assert!(is_file("/pack.zip"));
    assert!(!is_file("/article"));
    assert!(!is_file("/"));
}

#[test]
fn depth_ignores_index_files() {
    assert_eq!(get_url_depth("/"), 0);
    assert_eq!(get_url_depth("/a"), 1);
    assert_eq!(get_url_depth("/a/b"), 2);
    assert_eq!(get_url_depth("/a/b/index.html"), 2);
    assert_eq!(get_url_depth("/a/b/index.php"), 2);
}

#[test]
fn internal_link_ignores_www_prefix() {
    let base = u("https://example.com/");
    let link = u("https://www.example.com/foo");
    assert!(is_internal_link(&link, &base));

    let off = u("https://other.org/foo");
    assert!(!is_internal_link(&off, &base));
}

#[test]
fn sections_distinguish_hash_router_from_anchor() {
    // Plain anchor — skip.
    assert!(!no_sections("/p#section"));
    // Hash-router — keep.
    assert!(no_sections("/p#/dashboard/page"));
    // No hash — keep.
    assert!(no_sections("/p"));
}

#[test]
fn non_web_protocols_blocked() {
    assert!(is_non_web_protocol("mailto:x@y.com"));
    assert!(is_non_web_protocol("tel:+551199"));
    assert!(is_non_web_protocol("ftp://files"));
    assert!(!is_non_web_protocol("https://site"));
}

#[test]
fn social_media_detected_by_host_substring() {
    assert!(is_social_media_or_email("https://twitter.com/share"));
    assert!(is_social_media_or_email("https://github.com/repo"));
    assert!(!is_social_media_or_email("https://example.com/"));
}

#[test]
fn is_subdomain_uses_psl() {
    let a = u("https://docs.example.com/x");
    let b = u("https://api.example.com/y");
    assert!(is_subdomain(&a, &b));
    let c = u("https://example.co.uk/");
    let d = u("https://other.co.uk/");
    assert!(!is_subdomain(&c, &d));
}

#[test]
fn filter_denies_files_and_external_by_default() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    let result = filter_links(FilterLinksInput {
        links: vec![
            "/page".to_string(),
            "/logo.png".to_string(),
            "https://other.com/x".to_string(),
            "mailto:team@example.com".to_string(),
            "/deep/path/to/article".to_string(),
        ],
        limit: None,
        max_depth: 10,
        base_url: &base,
        initial_url: &initial,
        regex_on_full_url: false,
        excludes: &[],
        includes: &[],
        allow_backward_crawling: true,
        ignore_robots_txt: true,
        robots_txt: "",
        robots_user_agent: None,
        allow_external_content_links: false,
        allow_subdomains: false,
    });
    assert_eq!(result.links, vec!["/page", "/deep/path/to/article"]);
    assert_eq!(result.denials.len(), 3);
}

#[test]
fn filter_depth_limit_rejects() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    let result = filter_links(FilterLinksInput {
        links: vec!["/a/b/c/d/e".to_string(), "/a/b".to_string()],
        limit: None,
        max_depth: 3,
        base_url: &base,
        initial_url: &initial,
        regex_on_full_url: false,
        excludes: &[],
        includes: &[],
        allow_backward_crawling: true,
        ignore_robots_txt: true,
        robots_txt: "",
        robots_user_agent: None,
        allow_external_content_links: false,
        allow_subdomains: false,
    });
    assert_eq!(result.links, vec!["/a/b"]);
    assert_eq!(result.denials.len(), 1);
}

#[test]
fn filter_robots_txt_blocks() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    let robots = "User-agent: *\nDisallow: /private/\n";
    let result = filter_links(FilterLinksInput {
        links: vec!["/public/p1".to_string(), "/private/p2".to_string()],
        limit: None,
        max_depth: 10,
        base_url: &base,
        initial_url: &initial,
        regex_on_full_url: false,
        excludes: &[],
        includes: &[],
        allow_backward_crawling: true,
        ignore_robots_txt: false,
        robots_txt: robots,
        robots_user_agent: Some("crawlex"),
        allow_external_content_links: false,
        allow_subdomains: false,
    });
    assert_eq!(result.links, vec!["/public/p1"]);
    assert_eq!(result.denials.len(), 1);
    assert_eq!(result.denials[0].0, "/private/p2");
}

#[test]
fn filter_include_exclude_patterns() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    let excl = [Regex::new(r"/admin").unwrap()];
    let incl = [Regex::new(r"/blog").unwrap()];
    let result = filter_links(FilterLinksInput {
        links: vec![
            "/blog/post".to_string(),
            "/admin/panel".to_string(),
            "/about".to_string(),
        ],
        limit: None,
        max_depth: 10,
        base_url: &base,
        initial_url: &initial,
        regex_on_full_url: false,
        excludes: &excl,
        includes: &incl,
        allow_backward_crawling: true,
        ignore_robots_txt: true,
        robots_txt: "",
        robots_user_agent: None,
        allow_external_content_links: false,
        allow_subdomains: false,
    });
    assert_eq!(result.links, vec!["/blog/post"]);
    // /admin → exclude pattern; /about → include pattern.
    assert_eq!(result.denials.len(), 2);
}

#[test]
fn filter_limit_respected() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    let result = filter_links(FilterLinksInput {
        links: (0..100).map(|i| format!("/page/{i}")).collect(),
        limit: Some(5),
        max_depth: 10,
        base_url: &base,
        initial_url: &initial,
        regex_on_full_url: false,
        excludes: &[],
        includes: &[],
        allow_backward_crawling: true,
        ignore_robots_txt: true,
        robots_txt: "",
        robots_user_agent: None,
        allow_external_content_links: false,
        allow_subdomains: false,
    });
    assert_eq!(result.links.len(), 5);
}

#[test]
fn filter_subdomain_policy() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    let result = filter_links(FilterLinksInput {
        links: vec![
            "https://docs.example.com/api".to_string(),
            "https://other.org/page".to_string(),
        ],
        limit: None,
        max_depth: 10,
        base_url: &base,
        initial_url: &initial,
        regex_on_full_url: false,
        excludes: &[],
        includes: &[],
        allow_backward_crawling: true,
        ignore_robots_txt: true,
        robots_txt: "",
        robots_user_agent: None,
        allow_external_content_links: false,
        allow_subdomains: true,
    });
    assert_eq!(result.links, vec!["https://docs.example.com/api"]);
    assert_eq!(result.denials.len(), 1);
}
