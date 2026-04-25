use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

struct LinkSelector {
    selector: Selector,
    attr: &'static str,
    srcset: bool,
}

static LINK_SELECTORS: LazyLock<Vec<LinkSelector>> = LazyLock::new(|| {
    [
        ("a[href]", "href", false),
        ("link[href]", "href", false),
        ("script[src]", "src", false),
        ("img[src]", "src", false),
        ("img[srcset]", "srcset", true),
        ("source[src]", "src", false),
        ("source[srcset]", "srcset", true),
        ("iframe[src]", "src", false),
        ("embed[src]", "src", false),
        ("object[data]", "data", false),
        ("video[src]", "src", false),
        ("audio[src]", "src", false),
        ("track[src]", "src", false),
        ("form[action]", "action", false),
        ("meta[content]", "content", false), // og:url, etc. Harmless if not a URL.
    ]
    .into_iter()
    .map(|(css, attr, srcset)| LinkSelector {
        selector: Selector::parse(css).expect("static link selector must parse"),
        attr,
        srcset,
    })
    .collect()
});

pub fn extract_links(base: &Url, html: &str) -> Vec<Url> {
    let doc = Html::parse_document(html);
    extract_links_from_document(base, &doc)
}

pub fn extract_links_from_document(base: &Url, doc: &Html) -> Vec<Url> {
    let mut out = Vec::new();
    for spec in LINK_SELECTORS.iter() {
        for el in doc.select(&spec.selector) {
            if let Some(raw) = el.value().attr(spec.attr) {
                if spec.srcset {
                    push_srcset(base, raw, &mut out);
                } else if let Ok(u) = base.join(raw) {
                    out.push(u);
                }
            }
        }
    }
    out
}

fn push_srcset(base: &Url, raw: &str, out: &mut Vec<Url>) {
    for candidate in raw.split(',') {
        let Some(url_part) = candidate.split_whitespace().next() else {
            continue;
        };
        if let Ok(u) = base.join(url_part) {
            out.push(u);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::extract_links;
    use url::Url;

    #[test]
    fn extract_links_splits_srcset_candidates() {
        let base = Url::parse("https://example.com/dir/page").unwrap();
        let links = extract_links(
            &base,
            r#"<img srcset="small.jpg 1x, /big.jpg 2x"><source srcset="//cdn.example/a.webp 1x">"#,
        );
        let urls: Vec<_> = links.into_iter().map(|u| u.to_string()).collect();
        assert!(urls.contains(&"https://example.com/dir/small.jpg".to_string()));
        assert!(urls.contains(&"https://example.com/big.jpg".to_string()));
        assert!(urls.contains(&"https://cdn.example/a.webp".to_string()));
    }
}
