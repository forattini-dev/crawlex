//! Integration tests for [`crawlex::extract::pattern::Pattern`] wired through
//! [`crawlex::extract::link_filter::filter_links`].
//!
//! Covers the glob grammar promised in `issues/4-slice.md` and the
//! exclude-over-include precedence rule.

use crawlex::extract::link_filter::{filter_links, FilterLinksInput};
use crawlex::extract::pattern::{Glob, Pattern};
use url::Url;

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn glob_grammar_table() {
    // (pattern, path, expected)
    let cases: &[(&str, &str, bool)] = &[
        // `*` does not cross `/`
        ("/blog/*", "/blog/post", true),
        ("/blog/*", "/blog/2025/post", false),
        // `**` crosses `/`
        ("/blog/**", "/blog/2025/post", true),
        ("/blog/**", "/blog/", true),
        // Exact match
        ("/about", "/about", true),
        ("/about", "/about/team", false),
        // Leading `**/`
        ("**/post.html", "/a/b/post.html", true),
        ("**/post.html", "/post.html", true),
        // Trailing `/**`
        ("/api/**", "/api/v1/users", true),
        ("/api/**", "/apix", false),
    ];

    for (pat, path, expected) in cases {
        let g = Glob::compile(pat).unwrap();
        assert_eq!(
            g.matches(path),
            *expected,
            "pattern={pat:?} path={path:?} expected={expected}"
        );
    }
}

#[test]
fn exclude_wins_over_include() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    let excl = [Pattern::glob("/blog/admin/**").unwrap()];
    let incl = [Pattern::glob("/blog/**").unwrap()];
    let result = filter_links(FilterLinksInput {
        links: vec![
            "/blog/post".to_string(),
            "/blog/admin/dashboard".to_string(),
            "/other".to_string(),
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
    // /blog/admin/dashboard → exclude wins even though include matches.
    // /other → include miss.
    assert_eq!(result.denials.len(), 2);
    let reasons: Vec<_> = result
        .denials
        .iter()
        .map(|(l, r)| (l.as_str(), r.as_str()))
        .collect();
    assert!(reasons.contains(&("/blog/admin/dashboard", "exclude-pattern")));
    assert!(reasons.contains(&("/other", "include-pattern")));
}

#[test]
fn auto_detect_mixed_glob_and_regex() {
    let base = u("https://example.com/");
    let initial = u("https://example.com/");
    // Mix: glob include + regex exclude via `re:` escape hatch.
    let excl = [Pattern::compile_auto(r"re:/admin/\d+").unwrap()];
    let incl = [Pattern::compile_auto("/**").unwrap()];
    let result = filter_links(FilterLinksInput {
        links: vec![
            "/admin/42".to_string(),
            "/admin/dashboard".to_string(),
            "/blog/post".to_string(),
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

    // `/admin/42` blocked by regex exclude; rest pass.
    assert_eq!(result.links, vec!["/admin/dashboard", "/blog/post"]);
    assert_eq!(result.denials.len(), 1);
    assert_eq!(result.denials[0].0, "/admin/42");
}
