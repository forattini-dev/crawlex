//! Tests for URL permutations and the integrated Dedupe behaviour.
//!
//! Invariants from Firecrawl `generateURLPermutations`:
//!   1. Non-empty.
//!   2. Idempotent (apply twice → same set).
//!   3. No overlap between distinct-input URLs that are not aliases.
//!   4. Any alias of a seen URL is deduped on next insert.

use crawlex::frontier::dedupe::generate_url_permutations;
use crawlex::frontier::Dedupe;
use std::collections::HashSet;
use url::Url;

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn permutations_non_empty() {
    let p = generate_url_permutations(&u("https://example.com/"));
    assert!(!p.is_empty());
}

#[test]
fn permutations_include_http_and_https() {
    let p = generate_url_permutations(&u("https://example.com/page"));
    assert!(p.iter().any(|x| x.starts_with("https://")));
    assert!(p.iter().any(|x| x.starts_with("http://")));
}

#[test]
fn permutations_include_www_and_bare() {
    let p = generate_url_permutations(&u("https://example.com/"));
    assert!(p.iter().any(|x| x.contains("://example.com")));
    assert!(p.iter().any(|x| x.contains("://www.example.com")));
}

#[test]
fn permutations_include_index_variants() {
    let p = generate_url_permutations(&u("https://example.com/"));
    assert!(p.iter().any(|x| x.ends_with("/")));
    assert!(p.iter().any(|x| x.ends_with("/index.html")));
    assert!(p.iter().any(|x| x.ends_with("/index.htm")));
    assert!(p.iter().any(|x| x.ends_with("/index.php")));
}

#[test]
fn permutations_idempotent() {
    let a: HashSet<_> = generate_url_permutations(&u("https://example.com/a"))
        .into_iter()
        .collect();
    let b: HashSet<_> = generate_url_permutations(&u("https://example.com/a"))
        .into_iter()
        .collect();
    assert_eq!(a, b);
}

#[test]
fn distinct_urls_have_disjoint_permutation_sets() {
    let a: HashSet<_> = generate_url_permutations(&u("https://example.com/a"))
        .into_iter()
        .collect();
    let b: HashSet<_> = generate_url_permutations(&u("https://example.com/b"))
        .into_iter()
        .collect();
    assert!(a.is_disjoint(&b));
}

#[test]
fn scheme_switch_deduped_via_insert_url_set() {
    let d = Dedupe::new(1024, 0.01);
    assert!(d.insert_url_set(&u("https://example.com/p")));
    // Same URL but http scheme → must NOT count as new.
    assert!(!d.insert_url_set(&u("http://example.com/p")));
    // And www variant → also not new.
    assert!(!d.insert_url_set(&u("https://www.example.com/p")));
}

#[test]
fn index_html_vs_bare_slash_dedup() {
    let d = Dedupe::new(1024, 0.01);
    assert!(d.insert_url_set(&u("https://example.com/")));
    assert!(!d.insert_url_set(&u("https://example.com/index.html")));
    assert!(!d.insert_url_set(&u("https://example.com/index.htm")));
    assert!(!d.insert_url_set(&u("https://example.com/index.php")));
}

#[test]
fn non_http_scheme_not_expanded() {
    let p = generate_url_permutations(&u("data:text/plain;base64,SGVsbG8="));
    assert_eq!(p.len(), 1);
}

#[test]
fn query_is_canonicalized_and_fragment_is_dropped() {
    let p = generate_url_permutations(&u(
        "https://example.com/page?UTM_source=x&fbclid=y&b=2&a=1#top",
    ));
    assert!(p.iter().all(|x| x.contains("?a=1&b=2")));
    assert!(p
        .iter()
        .all(|x| !x.to_ascii_lowercase().contains("utm_source")));
    assert!(p.iter().all(|x| !x.contains("fbclid")));
    assert!(p.iter().all(|x| !x.contains("#top")));
}

#[test]
fn fragment_and_tracking_params_are_deduped() {
    let d = Dedupe::new(1024, 0.01);
    assert!(d.insert_url_set(&u("https://example.com/p?a=1&utm_source=x#one")));
    assert!(!d.insert_url_set(&u("http://www.example.com/p/?utm_medium=y&a=1#two")));
}
