use url::Url;

pub fn canonicalize(url: &Url) -> String {
    crate::frontier::identity::canonicalize(url)
}
