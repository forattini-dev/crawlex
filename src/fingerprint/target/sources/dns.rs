//! Dns source — Cold tier (B9).
//!
//! Classifies DNS records (A/AAAA/CNAME/TXT/NS) for CDN inference.
//! CNAMEs pointing at `*.cloudfront.net` / `*.fastly.net` /
//! `*.akamaiedge.net` / `*.cloudflare.com` produce high-confidence
//! CDN evidence. Plumbed via `crate::discovery::dns` — this source
//! consumes the existing DNS client's output rather than reimplementing.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct DnsSource;

impl DnsSource {
    pub fn new() -> Self {
        Self
    }

    /// Classify a list of resolved CNAME / target hostnames.
    /// Substring match (case-insensitive) against the known-CDN table.
    pub fn classify_cnames(cnames: &[String]) -> Vec<Detection> {
        let table: &[(&str, Vendor)] = &[
            ("cloudfront.net", Vendor::CloudFront),
            ("amazonaws.com", Vendor::Aws),
            ("fastly.net", Vendor::Fastly),
            ("akamaiedge.net", Vendor::Akamai),
            ("akamaized.net", Vendor::Akamai),
            ("akamaihd.net", Vendor::Akamai),
            ("cloudflare.com", Vendor::Cloudflare),
            ("cdn.cloudflare.net", Vendor::Cloudflare),
            ("cdn.bunnycdn.com", Vendor::Bunny),
            ("vercel-dns.com", Vendor::Vercel),
            ("netlify.com", Vendor::Netlify),
        ];
        let mut out: Vec<Detection> = Vec::new();
        for cname in cnames {
            let lower = cname.to_ascii_lowercase();
            for (needle, vendor) in table {
                if lower.contains(needle) {
                    out.push(Detection::from_single(
                        Category::Cdn,
                        *vendor,
                        Evidence::new(
                            EvidenceSource::Dns,
                            format!("CNAME chain '{cname}' contains '{needle}'"),
                            10,
                        ),
                    ));
                    break;
                }
            }
        }
        out
    }
}

impl Source for DnsSource {
    fn name(&self) -> &'static str {
        "dns"
    }

    fn tier(&self) -> Tier {
        Tier::Cold
    }

    fn analyze(&self, _ctx: &TargetContext<'_>) -> Vec<Detection> {
        // DNS records live outside the TargetContext today; the
        // engine's Cold-tier dispatch passes them in via `classify_cnames`.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cloudfront() {
        let dets = DnsSource::classify_cnames(&["d1234.cloudfront.net.".into()]);
        assert!(dets.iter().any(|d| d.vendor == Vendor::CloudFront));
    }

    #[test]
    fn detects_akamai_via_akamaiedge() {
        let dets = DnsSource::classify_cnames(&["e1234.x.akamaiedge.net.".into()]);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Akamai));
    }

    #[test]
    fn detects_fastly() {
        let dets = DnsSource::classify_cnames(&["d.global.ssl.fastly.net.".into()]);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Fastly));
    }

    #[test]
    fn no_detection_on_origin_cname() {
        let dets = DnsSource::classify_cnames(&["origin.example.com.".into()]);
        assert!(dets.is_empty());
    }
}
