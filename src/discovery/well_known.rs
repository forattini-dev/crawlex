//! Probe common `/.well-known/*` endpoints and extract URLs from their bodies.
//!
//! RFC 8615 registers well-known URIs; many leak additional surface:
//! - security.txt: contact, policy URLs
//! - openid-configuration: issuer, auth/token endpoints, jwks_uri
//! - apple-app-site-association / assetlinks.json: mobile deep links
//! - host-meta / host-meta.json: XRD links
//! - change-password (RFC 8615): password-change page
//! - oauth-authorization-server, webfinger: more endpoints

use url::Url;

/// Canonical probe list. Ordered by likelihood of yielding URLs.
pub const WELL_KNOWN_PATHS: &[&str] = &[
    "/.well-known/security.txt",
    "/.well-known/openid-configuration",
    "/.well-known/oauth-authorization-server",
    "/.well-known/apple-app-site-association",
    "/.well-known/assetlinks.json",
    "/.well-known/host-meta",
    "/.well-known/host-meta.json",
    "/.well-known/change-password",
    "/.well-known/webfinger",
    "/.well-known/nodeinfo",
    "/.well-known/dnt-policy.txt",
    "/.well-known/brand-indicators-for-message-identification",
];

pub fn probe_urls(origin: &Url) -> Vec<Url> {
    WELL_KNOWN_PATHS
        .iter()
        .filter_map(|p| origin.join(p).ok())
        .collect()
}

/// Extract absolute http(s) URLs from any of the common well-known body
/// formats. Since openid-configuration is JSON, host-meta is XML, and
/// security.txt is plain text, we take the pragmatic path: regex scan for
/// URL-like substrings. This is conservative but catches every URL these
/// formats care about.
pub fn extract_urls_from_body(body: &str) -> Vec<Url> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'h'
            && bytes[i..].starts_with(b"http")
            && (bytes[i..].starts_with(b"http://") || bytes[i..].starts_with(b"https://"))
        {
            let start = i;
            let mut end = i;
            while end < bytes.len() {
                let c = bytes[end];
                if c == b' '
                    || c == b'\n'
                    || c == b'\r'
                    || c == b'\t'
                    || c == b'"'
                    || c == b'\''
                    || c == b'<'
                    || c == b'>'
                    || c == b'`'
                    || c == b','
                    || c == b')'
                    || c == b']'
                    || c == b'}'
                {
                    break;
                }
                end += 1;
            }
            if end > start + 8 {
                let raw = &body[start..end];
                let trimmed = raw.trim_end_matches(|c: char| ".,;:!?".contains(c));
                if let Ok(u) = Url::parse(trimmed) {
                    out.push(u);
                }
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}
