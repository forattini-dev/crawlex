//! TlsServerHello source — Hot tier (B4).
//!
//! TLS / transport posture derived from the response's observed
//! cipher suite, TLS version, and ALPN. Today, evidence is mostly
//! generic ("TLS 1.3 + h2") with low weight, but the source pins the
//! plumbing — full JA3S / JA4S hash computation lands when the TLS
//! observation widens to expose ServerHello bytes.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct TlsServerHelloSource;

impl TlsServerHelloSource {
    pub fn new() -> Self {
        Self
    }
}

impl Source for TlsServerHelloSource {
    fn name(&self) -> &'static str {
        "tls_server_hello"
    }

    fn tier(&self) -> Tier {
        Tier::Hot
    }

    fn analyze(&self, ctx: &TargetContext<'_>) -> Vec<Detection> {
        let mut out: Vec<Detection> = Vec::new();
        // Server header echoes infra family — when present alongside
        // a particular cipher fingerprint, we'd raise Confidence. The
        // current TargetContext exposes only headers; richer TLS slots
        // appear in B8/B10 when the engine plumbs them in.
        //
        // Slice B4 emits a generic transport-tier Detection when an
        // ALPN-ish artefact shows in the `alt-svc` mirror — keeps the
        // source useful without claiming false certainty.
        if let Some(alpn_hint) = ctx.headers.get("alt-svc").and_then(|v| v.to_str().ok()) {
            if alpn_hint.to_ascii_lowercase().contains("h3") {
                out.push(Detection::from_single(
                    Category::Other,
                    Vendor::Generic,
                    Evidence::new(
                        EvidenceSource::TlsServerHello,
                        format!("HTTP/3 advertised via alt-svc ({alpn_hint})"),
                        2,
                    ),
                ));
            }
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
    fn emits_low_confidence_h3_marker() {
        let mut h = HeaderMap::new();
        h.insert("alt-svc", r#"h3=":443""#.parse().unwrap());
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = TlsServerHelloSource::new().analyze(&build_ctx(&h, &u));
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].category, Category::Other);
    }

    #[test]
    fn no_emit_without_signal() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let dets = TlsServerHelloSource::new().analyze(&build_ctx(&h, &u));
        assert!(dets.is_empty());
    }
}
