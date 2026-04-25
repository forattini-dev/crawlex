use url::Url;

pub fn canonicalize(url: &Url) -> String {
    if matches!(url.scheme(), "http" | "https") {
        return canonicalize_http_url(url);
    }

    let mut u = url.clone();
    u.set_fragment(None);
    if let Some(host) = u.host_str() {
        let lower = host.to_ascii_lowercase();
        let _ = u.set_host(Some(&lower));
    }
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !is_tracking_query_key(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let mut sorted = pairs;
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    {
        let mut qs = u.query_pairs_mut();
        qs.clear();
        for (k, v) in &sorted {
            qs.append_pair(k, v);
        }
    }
    if u.query() == Some("") {
        u.set_query(None);
    }
    u.to_string()
}

fn canonicalize_http_url(url: &Url) -> String {
    let host = url
        .host_str()
        .map(|h| h.to_ascii_lowercase())
        .unwrap_or_default();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let mut out = format!("web://{host}");
    if let Some(port) = url.port() {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(&canonical_path(url.path()));
    if let Some(query) = canonical_query(url) {
        out.push('?');
        out.push_str(&query);
    }
    out
}

fn canonical_path(path: &str) -> String {
    let path = if path.is_empty() { "/" } else { path };
    let without_index = path
        .strip_suffix("/index.html")
        .or_else(|| path.strip_suffix("/index.htm"))
        .or_else(|| path.strip_suffix("/index.php"))
        .unwrap_or(path);
    let trimmed = without_index.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn canonical_query(url: &Url) -> Option<String> {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_query_key(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.is_empty() {
        return None;
    }
    pairs.sort();
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        out.append_pair(&k, &v);
    }
    Some(out.finish())
}

fn is_tracking_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_") || matches!(key.as_str(), "fbclid" | "gclid" | "mc_cid" | "mc_eid")
}
