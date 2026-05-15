//! JA3 computation per https://github.com/salesforce/ja3.
//!
//! JA3 string format:
//!   SSLVersion,Cipher,SSLExtension,EllipticCurve,EllipticCurvePointFormat
//! Hash is MD5 of the string.

use md5::{Digest, Md5};
// Crate name in deps is `md-5`; in `use` it resolves as `md5`.

/// Compute JA3 string and hash from parsed ClientHello fields.
/// Caller provides the parsed values; this function does NOT parse
/// raw ClientHello bytes (TLS plumbing is invasive — done in B14).
pub fn compute_ja3(
    tls_version: u16,
    ciphers: &[u16],
    extensions: &[u16],
    curves: &[u16],
    ec_point_formats: &[u8],
) -> (String, String) {
    fn join_u16(items: &[u16]) -> String {
        items
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("-")
    }
    fn join_u8(items: &[u8]) -> String {
        items
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("-")
    }
    let s = format!(
        "{tls_version},{ciphers},{exts},{curves},{ec_fmts}",
        ciphers = join_u16(ciphers),
        exts = join_u16(extensions),
        curves = join_u16(curves),
        ec_fmts = join_u8(ec_point_formats),
    );
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    let digest = hasher.finalize();
    let hash = digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    (s, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ja3_canonical_chrome_example() {
        // Sample inputs from JA3 reference; assert format + non-empty
        // hash. We do not pin a specific hash here because the value
        // table for our profiles lives in B11's catalog.
        let (s, h) = compute_ja3(
            771,
            &[4865, 4866, 4867],
            &[0, 23, 65281],
            &[29, 23, 24],
            &[0],
        );
        assert_eq!(s, "771,4865-4866-4867,0-23-65281,29-23-24,0");
        assert_eq!(h.len(), 32);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn ja3_empty_fields() {
        let (s, h) = compute_ja3(771, &[], &[], &[], &[]);
        assert_eq!(s, "771,,,,");
        assert_eq!(h.len(), 32);
    }

    #[test]
    fn ja3_hash_deterministic() {
        let (_, h1) = compute_ja3(771, &[4865], &[0], &[29], &[0]);
        let (_, h2) = compute_ja3(771, &[4865], &[0], &[29], &[0]);
        assert_eq!(h1, h2);
    }
}
