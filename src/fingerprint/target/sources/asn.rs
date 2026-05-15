//! Asn source — Cold tier (B9).
//!
//! ASN lookup → hosting / CDN inference. Targets resolving to
//! Cloudflare's AS13335 are "behind Cloudflare" regardless of HTTP
//! headers. Plumbs via `crate::discovery::rdap` — consumes its output.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct AsnSource;

impl AsnSource {
    pub fn new() -> Self {
        Self
    }

    /// Map ASN + org-name to a hosting/CDN vendor. Substring match on
    /// org name lowercased; ASN-exact lookups can be added later.
    pub fn classify(asn: Option<u32>, org_name: Option<&str>) -> Vec<Detection> {
        let table: &[(u32, &str, Category, Vendor)] = &[
            (13335, "cloudflarenet", Category::Cdn, Vendor::Cloudflare),
            (20940, "akamai", Category::Cdn, Vendor::Akamai),
            (54113, "fastly", Category::Cdn, Vendor::Fastly),
            (16509, "amazon", Category::DnsHosting, Vendor::Aws),
            (15169, "google", Category::DnsHosting, Vendor::Gcp),
            (8075, "microsoft", Category::DnsHosting, Vendor::Azure),
            (14061, "digitalocean", Category::DnsHosting, Vendor::DigitalOcean),
            (24940, "hetzner", Category::DnsHosting, Vendor::Hetzner),
            (16276, "ovh", Category::DnsHosting, Vendor::Ovh),
        ];
        let mut out: Vec<Detection> = Vec::new();
        let org_lower = org_name.map(|s| s.to_ascii_lowercase());
        for (asn_num, needle, cat, vendor) in table {
            let asn_hit = asn == Some(*asn_num);
            let org_hit = org_lower
                .as_deref()
                .map(|s| s.contains(needle))
                .unwrap_or(false);
            if asn_hit || org_hit {
                let detail = match (asn, org_name) {
                    (Some(a), Some(o)) => format!("ASN={a} org='{o}'"),
                    (Some(a), None) => format!("ASN={a}"),
                    (None, Some(o)) => format!("org='{o}'"),
                    _ => "no asn/org".into(),
                };
                out.push(Detection::from_single(
                    *cat,
                    *vendor,
                    Evidence::new(EvidenceSource::Asn, detail, 10),
                ));
            }
        }
        out
    }
}

impl Source for AsnSource {
    fn name(&self) -> &'static str {
        "asn"
    }

    fn tier(&self) -> Tier {
        Tier::Cold
    }

    fn analyze(&self, _ctx: &TargetContext<'_>) -> Vec<Detection> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cloudflare_via_asn() {
        let dets = AsnSource::classify(Some(13335), Some("CLOUDFLARENET"));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Cloudflare));
    }

    #[test]
    fn detects_aws_via_asn_only() {
        let dets = AsnSource::classify(Some(16509), None);
        assert!(dets.iter().any(|d| d.vendor == Vendor::Aws));
    }

    #[test]
    fn detects_hetzner_via_org_only() {
        let dets = AsnSource::classify(None, Some("Hetzner Online GmbH"));
        assert!(dets.iter().any(|d| d.vendor == Vendor::Hetzner));
    }

    #[test]
    fn no_detection_on_unknown() {
        let dets = AsnSource::classify(Some(99999), Some("Tiny ISP Inc"));
        assert!(dets.is_empty());
    }
}
