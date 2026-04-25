//! Tests for the Firecrawl-ported HTML cleaner.

use crawlex::extract::html_clean::{clean_html, remove_skip_to_content_links, CleanOptions};

#[test]
fn strips_script_style_meta_head_noscript() {
    let html = r#"<!doctype html><html>
<head><title>hi</title><meta name="x"></head>
<body><script>alert(1)</script><style>.x{}</style><noscript>no</noscript><p>keep</p></body></html>"#;
    let out = clean_html(
        html,
        &CleanOptions {
            url: "https://example.com/",
            exclude_tags: &[],
            only_main_content: false,
        },
    )
    .unwrap();
    assert!(out.contains("<p>keep</p>"));
    assert!(!out.contains("<script"));
    assert!(!out.contains("<style"));
    assert!(!out.contains("<meta"));
    assert!(!out.contains("<head"));
    assert!(!out.contains("<noscript"));
}

#[test]
fn only_main_content_strips_header_nav_footer() {
    let html = r#"<html><body>
<header>hdr</header>
<nav>nv</nav>
<main><p>article</p></main>
<footer>ftr</footer>
</body></html>"#;
    let out = clean_html(
        html,
        &CleanOptions {
            url: "https://example.com/",
            exclude_tags: &[],
            only_main_content: true,
        },
    )
    .unwrap();
    assert!(out.contains("<main><p>article</p></main>"));
    assert!(!out.contains("hdr"));
    assert!(!out.contains("ftr"));
    assert!(!out.contains("nv"));
}

#[test]
fn force_include_main_keeps_wrapping_header() {
    // #main inside a <header> forces keeping the header.
    let html = r#"<html><body>
<header><div id="main"><p>real</p></div></header>
<nav>junk</nav>
</body></html>"#;
    let out = clean_html(
        html,
        &CleanOptions {
            url: "https://example.com/",
            exclude_tags: &[],
            only_main_content: true,
        },
    )
    .unwrap();
    assert!(out.contains(r#"id="main""#));
    assert!(out.contains("real"));
    assert!(!out.contains("junk"));
}

#[test]
fn exclude_tags_custom_removes() {
    let html = r#"<html><body><div class="x">x</div><p>y</p></body></html>"#;
    let out = clean_html(
        html,
        &CleanOptions {
            url: "https://example.com/",
            exclude_tags: &[".x"],
            only_main_content: false,
        },
    )
    .unwrap();
    assert!(!out.contains("<div"));
    assert!(out.contains("<p>y</p>"));
}

#[test]
fn relative_href_resolved_against_base() {
    let html = r#"<html><body><a href="/page">L</a><a href="sub/x">M</a></body></html>"#;
    let out = clean_html(
        html,
        &CleanOptions {
            url: "https://example.com/docs/index.html",
            exclude_tags: &[],
            only_main_content: false,
        },
    )
    .unwrap();
    assert!(
        out.contains(r#"href="https://example.com/page""#),
        "out: {}",
        out
    );
    assert!(
        out.contains(r#"href="https://example.com/docs/sub/x""#),
        "out: {}",
        out
    );
}

#[test]
fn relative_img_src_resolved_against_base() {
    let html = r#"<html><body><img src="logo.png"></body></html>"#;
    let out = clean_html(
        html,
        &CleanOptions {
            url: "https://example.com/blog/",
            exclude_tags: &[],
            only_main_content: false,
        },
    )
    .unwrap();
    assert!(out.contains(r#"src="https://example.com/blog/logo.png""#));
}

#[test]
fn srcset_picks_biggest_candidate() {
    let html = r#"<html><body>
<img src="small.png" srcset="small.png 1x, big.png 2x, huge.png 3x">
</body></html>"#;
    let out = clean_html(
        html,
        &CleanOptions {
            url: "https://example.com/",
            exclude_tags: &[],
            only_main_content: false,
        },
    )
    .unwrap();
    // Biggest is huge.png at 3x → src should be rewritten to it.
    assert!(out.contains("huge.png"), "out: {out}");
}

#[test]
fn skip_to_content_link_stripped() {
    let input = "# Page\n[Skip to Content](#main)\n\nReal content here.";
    let out = remove_skip_to_content_links(input);
    assert!(!out.contains("Skip to Content"));
    assert!(out.contains("Real content here."));
}

#[test]
fn skip_to_content_label_is_case_insensitive() {
    let input = "[SKIP TO CONTENT](#x)[lower](#y)[Skip to content](#z)end";
    let out = remove_skip_to_content_links(input);
    // Only "Skip to Content" matches (case-insensitive), others remain.
    assert!(!out.contains("SKIP TO CONTENT"));
    assert!(!out.contains("Skip to content"));
    assert!(out.contains("[lower](#y)"));
    assert!(out.ends_with("end"));
}

#[test]
fn skip_to_content_only_hash_anchors_stripped() {
    // Link that looks similar but points to a real URL stays.
    let input = "[Skip to Content](/real)";
    let out = remove_skip_to_content_links(input);
    assert_eq!(out, input);
}
