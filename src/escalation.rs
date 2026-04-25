//! Heuristic to decide when a `FetchMethod::Auto` job should be re-queued as
//! `Render` because the HTTP spoof response doesn't have the real content.
//!
//! Signals (cheap, no DOM parsing needed):
//! * 403/503 with anti-bot vendor signatures in the body
//!   (`cf-chl-bypass`, `Just a moment`, `DataDome`, `PerimeterX`, `Imperva`).
//! * `Content-Type: text/html` but body < 2 KiB and contains a script that
//!   sets window.location or document.title — classic JS challenge stub.
//! * Explicit `<noscript>` "please enable JavaScript" with near-empty body.

use http::HeaderMap;

use crate::error::AntibotVendor;

/// (signature, vendor) pairs — checked in order; first match wins.
const VENDOR_SIGNATURES: &[(&str, AntibotVendor)] = &[
    ("cf-chl-bypass", AntibotVendor::Cloudflare),
    ("Just a moment", AntibotVendor::Cloudflare),
    ("/cdn-cgi/challenge-platform/", AntibotVendor::Cloudflare),
    ("DataDome", AntibotVendor::DataDome),
    ("PerimeterX", AntibotVendor::PerimeterX),
    ("_Incapsula_", AntibotVendor::Imperva),
    ("Imperva", AntibotVendor::Imperva),
    ("distilnetworks", AntibotVendor::DistilNetworks),
];

/// When the response *looks* like an antibot challenge, return the detected
/// vendor. Returns `None` for healthy responses; callers can turn that
/// into `Error::AntibotChallenge { vendor, ... }` directly.
pub fn detect_antibot_vendor(
    status: u16,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<AntibotVendor> {
    let is_html = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false);

    if matches!(status, 403 | 503) {
        let body_lower = body_as_string(body);
        for (sig, vendor) in VENDOR_SIGNATURES {
            if body_lower.contains(sig) {
                return Some(*vendor);
            }
        }
    }

    if is_html && body.len() < 2048 {
        let body_str = body_as_string(body);
        if body_str.contains("<script") || body_str.contains("window.location") {
            return Some(AntibotVendor::Other);
        }
        if body_str.contains("<noscript")
            && (body_str.contains("enable JavaScript")
                || body_str.contains("Please enable JavaScript"))
        {
            return Some(AntibotVendor::Other);
        }
    }
    None
}

pub fn should_escalate(status: u16, headers: &HeaderMap, body: &[u8]) -> bool {
    detect_antibot_vendor(status, headers, body).is_some()
}

fn body_as_string(body: &[u8]) -> String {
    // We don't need strict UTF-8 — vendor signatures are ASCII anyway. Cap
    // the slice to 16 KiB so we don't pay to scan huge bodies.
    let slice = &body[..body.len().min(16 * 1024)];
    String::from_utf8_lossy(slice).into_owned()
}
