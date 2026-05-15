//! Challenge detection seam (slice 3 of the JobRunner extraction, GH #19).
//!
//! Inspects an HTTP response and emits a `ChallengeSignal` when an
//! antibot interstitial is recognised. Absorbed the body of the legacy
//! `escalation.rs` — deleted in slice #21.
//!
//! Concrete struct, no trait. Pure logic.

use http::HeaderMap;

use crate::error::AntibotVendor;

/// What the detector emits when it recognises a challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChallengeSignal {
    pub vendor: AntibotVendor,
}

/// Pure detector. Holds no state; cheap to construct.
///
/// Superseded by [`crate::fingerprint::target::sources::AntibotMarkerSource`]
/// in slice B5 (PRD forattini-dev/crawlex#25). The new source ports
/// all logic verbatim (same 6 vendor sigs, same 403/503 gate, same
/// 16 KiB cap, same generic JS-stub / noscript fallback) into the
/// unified `Fingerprinter` engine. `ChallengeDetector` is kept for
/// one release while in-tree callers (`policy::engine`) migrate to
/// `Fingerprinter::analyze_hot` + `report.antibot` — that swap lands
/// in B14. Removed in B15 alongside ADR-0003.
#[deprecated(
    since = "1.0.5",
    note = "use crate::fingerprint::target::sources::AntibotMarkerSource via Fingerprinter::analyze_hot; ChallengeDetector is removed in B15"
)]
#[derive(Debug, Default, Clone, Copy)]
pub struct ChallengeDetector;

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

impl ChallengeDetector {
    pub fn new() -> Self {
        Self
    }

    /// Inspect an HTTP response and return a signal if the response
    /// matches a known antibot vendor signature.
    pub fn detect(&self, status: u16, headers: &HeaderMap, body: &[u8]) -> Option<ChallengeSignal> {
        let is_html = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_ascii_lowercase().contains("text/html"))
            .unwrap_or(false);

        if matches!(status, 403 | 503) {
            let body_lower = body_as_string(body);
            for (sig, vendor) in VENDOR_SIGNATURES {
                if body_lower.contains(sig) {
                    return Some(ChallengeSignal { vendor: *vendor });
                }
            }
        }

        if is_html && body.len() < 2048 {
            let body_str = body_as_string(body);
            if body_str.contains("<script") || body_str.contains("window.location") {
                return Some(ChallengeSignal {
                    vendor: AntibotVendor::Other,
                });
            }
            if body_str.contains("<noscript")
                && (body_str.contains("enable JavaScript")
                    || body_str.contains("Please enable JavaScript"))
            {
                return Some(ChallengeSignal {
                    vendor: AntibotVendor::Other,
                });
            }
        }
        None
    }
}

fn body_as_string(body: &[u8]) -> String {
    // Vendor signatures are ASCII; cap at 16 KiB so we don't pay to
    // scan huge bodies. Matches the legacy `escalation::body_as_string`.
    let slice = &body[..body.len().min(16 * 1024)];
    String::from_utf8_lossy(slice).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("content-type", "text/html; charset=utf-8".parse().unwrap());
        h
    }

    fn empty_headers() -> HeaderMap {
        HeaderMap::new()
    }

    #[test]
    fn detects_cloudflare_via_chl_bypass() {
        let sig = ChallengeDetector::new()
            .detect(403, &empty_headers(), b"... cf-chl-bypass ...")
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::Cloudflare);
    }

    #[test]
    fn detects_cloudflare_via_just_a_moment() {
        let sig = ChallengeDetector::new()
            .detect(503, &empty_headers(), b"<html>Just a moment...</html>")
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::Cloudflare);
    }

    #[test]
    fn detects_datadome() {
        let sig = ChallengeDetector::new()
            .detect(403, &empty_headers(), b"DataDome block")
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::DataDome);
    }

    #[test]
    fn detects_perimeterx() {
        let sig = ChallengeDetector::new()
            .detect(403, &empty_headers(), b"PerimeterX challenge")
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::PerimeterX);
    }

    #[test]
    fn detects_imperva_incapsula() {
        let sig = ChallengeDetector::new()
            .detect(403, &empty_headers(), b"_Incapsula_ ...")
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::Imperva);
    }

    #[test]
    fn detects_distil() {
        let sig = ChallengeDetector::new()
            .detect(403, &empty_headers(), b"distilnetworks tag")
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::DistilNetworks);
    }

    #[test]
    fn detects_generic_js_stub() {
        let body = b"<html><script>window.location='x'</script></html>";
        let sig = ChallengeDetector::new()
            .detect(200, &html_headers(), body)
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::Other);
    }

    #[test]
    fn detects_generic_noscript_challenge() {
        let body = b"<html><noscript>Please enable JavaScript</noscript></html>";
        let sig = ChallengeDetector::new()
            .detect(200, &html_headers(), body)
            .unwrap();
        assert_eq!(sig.vendor, AntibotVendor::Other);
    }

    #[test]
    fn healthy_200_returns_none() {
        let body = b"<html><body><h1>real content here</h1><p>lots of text</p></body></html>";
        assert!(ChallengeDetector::new()
            .detect(200, &html_headers(), body)
            .is_none());
    }

    #[test]
    fn vendor_signatures_only_trigger_on_403_or_503() {
        // 200 with cf-chl-bypass in body should not trigger — the legacy
        // detector only checks vendor sigs on 403/503.
        let body = b"cf-chl-bypass";
        assert!(ChallengeDetector::new()
            .detect(200, &empty_headers(), body)
            .is_none());
    }

    #[test]
    fn detector_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ChallengeDetector>();
    }
}
