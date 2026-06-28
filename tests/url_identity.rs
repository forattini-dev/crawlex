use crawlex::frontier::identity::UrlIdentity;
use url::Url;

fn key(raw: &str) -> String {
    UrlIdentity::from_url(&Url::parse(raw).unwrap()).canonical_key
}

#[test]
fn url_identity_collapses_common_page_aliases() {
    let first = key("https://www.example.com/store/index.html?utm_source=x&a=1#top");
    let second = key("http://example.com/store/?a=1&utm_medium=y#other");

    assert_eq!(first, second);
}
