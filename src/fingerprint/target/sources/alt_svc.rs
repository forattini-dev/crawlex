//! AltSvc source — Hot tier (B3).
//!
//! Reads the `alt-svc` response header. `h3=":443"` advertises HTTP/3
//! support — emits a `Generic` HttpFingerprint-class detection so the
//! report surfaces transport-tier facts in the `other` slot until
//! B8 adds the dedicated `HttpFingerprint` single slot.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct AltSvcSource;

impl AltSvcSource {
    pub fn new() -> Self {
        Self
    }
}

impl Source for AltSvcSource {
    fn name(&self) -> &'static str {
        "alt_svc"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let Some(value) = ctx
            .headers
            .get("alt-svc")
            .and_then(|v| v.to_str().ok())
        else {
            return Vec::new();
        };
        let lower = value.to_ascii_lowercase();
        let mut out: Vec<Detection> = Vec::new();
        if lower.contains("h3") {
            out.push(Detection::from_single(
                Category::Other,
                Vendor::Generic,
                Evidence::new(
                    EvidenceSource::AltSvc,
                    format!("alt-svc advertises HTTP/3: '{value}'"),
                    4,
                ),
            ));
        }
        if lower.contains("h2") && !lower.contains("h3") {
            // Rare — most h2-only hosts don't ship alt-svc.
            out.push(Detection::from_single(
                Category::Other,
                Vendor::Generic,
                Evidence::new(
                    EvidenceSource::AltSvc,
                    format!("alt-svc advertises HTTP/2 only: '{value}'"),
                    2,
                ),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use url::Url;

    fn build_ctx<'a>(headers: &'a HeaderMap, url: &'a Url) -> TargetContext<'a> {
        TargetContext::http_only(url, 200, headers, b"")
    }

    #[test]
    fn detects_h3_advertise() {
        let mut h = HeaderMap::new();
        h.insert("alt-svc", r#"h3=":443"; ma=93600"#.parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AltSvcSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets
            .iter()
            .any(|d| d.evidence[0].detail.contains("HTTP/3")));
    }

    #[test]
    fn no_detection_without_alt_svc() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = AltSvcSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.is_empty());
    }
}
