//! h2 SETTINGS frame fingerprint (Akamai-style).
//!
//! Canonical-order serialisation of SETTINGS key/value pairs hashed to
//! a stable identifier. Two clients sending identical settings get the
//! same hash; one frame difference (e.g., initial_window_size) flips
//! the hash.

use sha2::{Digest, Sha256};

/// HTTP/2 SETTINGS parameter codes per RFC 9113.
pub const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
pub const SETTINGS_ENABLE_PUSH: u16 = 0x2;
pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
pub const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
pub const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

/// Compute a stable fingerprint from h2 SETTINGS pairs.
/// Serialises as `key:value;key:value;...` in canonical sorted-by-key
/// order, then sha256. Returns hex digest.
pub fn compute_h2_settings_fingerprint(pairs: &[(u16, u32)]) -> String {
    let mut sorted = pairs.to_vec();
    sorted.sort_by_key(|(k, _)| *k);
    let joined = sorted
        .iter()
        .map(|(k, v)| format!("{}:{}", k, v))
        .collect::<Vec<_>>()
        .join(";");
    let mut h = Sha256::new();
    h.update(joined.as_bytes());
    let d = h.finalize();
    d.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h2_fingerprint_is_deterministic() {
        let pairs = &[
            (SETTINGS_HEADER_TABLE_SIZE, 65536),
            (SETTINGS_MAX_CONCURRENT_STREAMS, 1000),
            (SETTINGS_INITIAL_WINDOW_SIZE, 6291456),
        ];
        let a = compute_h2_settings_fingerprint(pairs);
        let b = compute_h2_settings_fingerprint(pairs);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn h2_fingerprint_order_independent() {
        let p1 = &[(SETTINGS_HEADER_TABLE_SIZE, 65536), (SETTINGS_INITIAL_WINDOW_SIZE, 6291456)];
        let p2 = &[(SETTINGS_INITIAL_WINDOW_SIZE, 6291456), (SETTINGS_HEADER_TABLE_SIZE, 65536)];
        assert_eq!(
            compute_h2_settings_fingerprint(p1),
            compute_h2_settings_fingerprint(p2)
        );
    }

    #[test]
    fn h2_fingerprint_changes_on_value_diff() {
        let p1 = &[(SETTINGS_INITIAL_WINDOW_SIZE, 6291456)];
        let p2 = &[(SETTINGS_INITIAL_WINDOW_SIZE, 65535)];
        assert_ne!(
            compute_h2_settings_fingerprint(p1),
            compute_h2_settings_fingerprint(p2)
        );
    }
}
