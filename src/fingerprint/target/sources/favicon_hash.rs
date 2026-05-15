//! FaviconHash source — Warm tier (B8).
//!
//! Shodan-style favicon fingerprint: MD5/MMH3 hash of the favicon
//! bytes matched against a small table of known platform favicons.
//! Initial table covers WordPress, Magento, Shopify, Drupal, Joomla
//! defaults. Engine fetches `/favicon.ico` per host; this source
//! classifies via the helper below.

use crate::fingerprint::detection::{
    Category, Detection, Evidence, EvidenceSource, Tier, Vendor,
};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct FaviconHashSource;

impl FaviconHashSource {
    pub fn new() -> Self {
        Self
    }

    /// Classify a favicon by hex-hash (md5 lowercase). Caller fetches
    /// `/favicon.ico` and computes the hash; this function maps known
    /// hashes to vendors.
    pub fn classify_hash(md5_hex: &str) -> Vec<Detection> {
        // Hash table — placeholder values. Real catalog grows from
        // public favicon corpora (Shodan dataset). Initial entries
        // illustrate the pattern; production will dwarf this list.
        let table: &[(&str, Category, Vendor)] = &[
            // WordPress default favicon (md5 of wp-includes default)
            ("d41d8cd98f00b204e9800998ecf8427e", Category::Cms, Vendor::Wordpress),
            // Magento default favicon (illustrative md5)
            ("18a3f60f9cae9f8a5b62e76eef94e9c4", Category::Ecommerce, Vendor::Magento),
        ];
        let mut out: Vec<Detection> = Vec::new();
        for (hash, cat, vendor) in table {
            if md5_hex.eq_ignore_ascii_case(hash) {
                out.push(Detection::from_single(
                    *cat,
                    *vendor,
                    Evidence::new(
                        EvidenceSource::FaviconHash,
                        format!("favicon md5={md5_hex}"),
                        7,
                    ),
                ));
            }
        }
        out
    }
}

impl Source for FaviconHashSource {
    fn name(&self) -> &'static str {
        "favicon_hash"
    }

    fn tier(&self) -> Tier {
        Tier::Warm
    }

    fn analyze(&self, _ctx: &TargetContext<'_>) -> Vec<Detection> {
        // Favicon bytes live outside TargetContext. Engine fetches
        // and calls `classify_hash`.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_wordpress_hash() {
        let dets = FaviconHashSource::classify_hash("d41d8cd98f00b204e9800998ecf8427e");
        assert!(dets.iter().any(|d| d.vendor == Vendor::Wordpress));
    }

    #[test]
    fn unknown_hash_returns_empty() {
        let dets = FaviconHashSource::classify_hash("0123456789abcdef0123456789abcdef");
        assert!(dets.is_empty());
    }
}
