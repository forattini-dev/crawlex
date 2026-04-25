//! Progressive Web App discovery: manifest + service workers.
//!
//! Surfaces we chase:
//! * `<link rel="manifest" href="...">` in HTML (handled by link extractor).
//! * Common manifest filenames at site root.
//! * Service worker scripts — usually registered via
//!   `navigator.serviceWorker.register('/sw.js')` in JS.
//! * `browserconfig.xml` (legacy MS PWA).
//! * `manifest.json` `start_url`, `scope`, `icons`, `shortcuts`, `share_target`.

use url::Url;

pub const PWA_PROBE_PATHS: &[&str] = &[
    "/manifest.json",
    "/manifest.webmanifest",
    "/site.webmanifest",
    "/app.webmanifest",
    "/sw.js",
    "/service-worker.js",
    "/serviceworker.js",
    "/workbox-sw.js",
    "/firebase-messaging-sw.js",
    "/browserconfig.xml",
    "/pwa.js",
    "/manifest.appcache",
];

pub fn probe_urls(origin: &Url) -> Vec<Url> {
    PWA_PROBE_PATHS
        .iter()
        .filter_map(|p| origin.join(p).ok())
        .collect()
}

/// Extract URLs from a PWA manifest JSON body. We look at:
/// `start_url`, `scope`, `icons[].src`, `screenshots[].src`,
/// `shortcuts[].url`, `shortcuts[].icons[].src`, `related_applications[].url`,
/// `share_target.action`.
pub fn extract_urls_from_manifest(base: &Url, body: &str) -> Vec<Url> {
    let mut out = Vec::new();
    let Ok(json): std::result::Result<serde_json::Value, _> = serde_json::from_str(body) else {
        return out;
    };
    let push = |raw: &str, out: &mut Vec<Url>| {
        if raw.is_empty() {
            return;
        }
        if let Ok(u) = base.join(raw) {
            out.push(u);
        }
    };
    if let Some(s) = json.get("start_url").and_then(|v| v.as_str()) {
        push(s, &mut out);
    }
    if let Some(s) = json.get("scope").and_then(|v| v.as_str()) {
        push(s, &mut out);
    }
    for key in ["icons", "screenshots"] {
        if let Some(arr) = json.get(key).and_then(|v| v.as_array()) {
            for el in arr {
                if let Some(s) = el.get("src").and_then(|v| v.as_str()) {
                    push(s, &mut out);
                }
            }
        }
    }
    if let Some(arr) = json.get("shortcuts").and_then(|v| v.as_array()) {
        for sc in arr {
            if let Some(s) = sc.get("url").and_then(|v| v.as_str()) {
                push(s, &mut out);
            }
            if let Some(icons) = sc.get("icons").and_then(|v| v.as_array()) {
                for i in icons {
                    if let Some(s) = i.get("src").and_then(|v| v.as_str()) {
                        push(s, &mut out);
                    }
                }
            }
        }
    }
    if let Some(arr) = json.get("related_applications").and_then(|v| v.as_array()) {
        for ra in arr {
            if let Some(s) = ra.get("url").and_then(|v| v.as_str()) {
                push(s, &mut out);
            }
        }
    }
    if let Some(st) = json.get("share_target") {
        if let Some(s) = st.get("action").and_then(|v| v.as_str()) {
            push(s, &mut out);
        }
    }
    out
}

/// Scan JS source for `serviceWorker.register('...')` / `import(...)` /
/// `workbox.*` registrations. Returns extracted paths/URLs.
pub fn extract_service_workers_from_js(base: &Url, js: &str) -> Vec<Url> {
    let mut out = Vec::new();
    let patterns = [
        "serviceWorker.register(",
        "serviceWorker .register(",
        "navigator.serviceWorker.register(",
    ];
    for pat in patterns {
        let mut cursor = 0;
        while let Some(p) = js[cursor..].find(pat) {
            let abs = cursor + p + pat.len();
            if let Some(literal) = take_js_string_literal(&js[abs..]) {
                if let Ok(u) = base.join(&literal) {
                    out.push(u);
                }
            }
            cursor = abs;
        }
    }
    out
}

fn take_js_string_literal(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let q = bytes[i];
    if q != b'"' && q != b'\'' && q != b'`' {
        return None;
    }
    let start = i + 1;
    let mut end = start;
    while end < bytes.len() && bytes[end] != q {
        if bytes[end] == b'\\' {
            end += 2;
        } else {
            end += 1;
        }
    }
    if end >= bytes.len() {
        return None;
    }
    Some(s[start..end].to_string())
}
