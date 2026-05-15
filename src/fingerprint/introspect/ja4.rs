//! JA4 computation (simplified) per https://github.com/FoxIO-LLC/ja4.
//!
//! JA4 carries protocol/version/ALPN/cipher-count/extension-count plus
//! truncated hashes. We implement the structural parts here; the full
//! JA4 spec has many flavours — this is the `t13d` (TLS 1.3 destination)
//! variant most relevant for our outbound captures.

use sha2::{Digest, Sha256};

/// Compute a JA4-style "t13d" string from parsed ClientHello fields.
///
/// Format (simplified): `t13d{cipher_count:02}{ext_count:02}_{cipher_hash}_{ext_hash}`
///   - `cipher_count` is the count of non-GREASE ciphers
///   - `ext_count` is the count of non-GREASE extensions
///   - `cipher_hash` is sha256 over sorted ciphers (truncated to 12 hex chars)
///   - `ext_hash` is sha256 over sorted extensions (truncated to 12 hex chars)
pub fn compute_ja4(ciphers: &[u16], extensions: &[u16]) -> String {
    let mut sorted_ciphers: Vec<u16> = ciphers
        .iter()
        .copied()
        .filter(|c| !is_grease(*c))
        .collect();
    sorted_ciphers.sort_unstable();
    let mut sorted_exts: Vec<u16> = extensions
        .iter()
        .copied()
        .filter(|e| !is_grease(*e))
        .collect();
    sorted_exts.sort_unstable();

    fn short_hash(items: &[u16]) -> String {
        let joined: String = items
            .iter()
            .map(|x| format!("{:04x}", x))
            .collect::<Vec<_>>()
            .join(",");
        let mut h = Sha256::new();
        h.update(joined.as_bytes());
        let d = h.finalize();
        d.iter()
            .take(6)
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    }

    format!(
        "t13d{:02}{:02}_{}_{}",
        sorted_ciphers.len().min(99),
        sorted_exts.len().min(99),
        short_hash(&sorted_ciphers),
        short_hash(&sorted_exts),
    )
}

/// GREASE values per RFC 8701 — the pattern 0xnAnA.
fn is_grease(v: u16) -> bool {
    (v & 0x0f0f) == 0x0a0a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ja4_t13d_format() {
        let s = compute_ja4(&[4865, 4866, 4867], &[0, 23, 65281]);
        assert!(s.starts_with("t13d"));
        // count fields
        assert!(&s[4..6] == "03"); // 3 ciphers
        assert!(&s[6..8] == "03"); // 3 extensions
        // separator
        assert!(s.contains('_'));
    }

    #[test]
    fn ja4_strips_grease() {
        // 0x0a0a is GREASE
        let s_with = compute_ja4(&[4865, 0x0a0a, 4866], &[0, 0x0a0a, 23]);
        let s_without = compute_ja4(&[4865, 4866], &[0, 23]);
        assert_eq!(s_with, s_without, "GREASE should be filtered");
    }

    #[test]
    fn ja4_is_deterministic() {
        let s1 = compute_ja4(&[4866, 4865], &[23, 0]);
        let s2 = compute_ja4(&[4865, 4866], &[0, 23]);
        assert_eq!(s1, s2, "order-independent via internal sort");
    }
}
