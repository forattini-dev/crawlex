//! Helpers ported from `recaptchav3/utils.py` of the reference solver.
//! Pure functions — no IO, no async, no random state outside what the caller
//! supplies. Tested via property-style unit tests.

use base64::{engine::general_purpose, Engine as _};
use rand::{Rng, RngExt};

const B36: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Base-36 encode a non-negative integer. Empty input returns `"0"` (matches
/// the Python reference). Used for the grecaptcha `cb` query parameter.
pub fn to_base36(mut num: u64) -> String {
    if num == 0 {
        return "0".to_string();
    }
    let mut buf = Vec::with_capacity(13); // u64 → at most 13 base36 digits
    while num > 0 {
        buf.push(B36[(num % 36) as usize]);
        num /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("base36 alphabet is ASCII")
}

/// Generate the `cb` query parameter the grecaptcha JS sends with anchor
/// requests: two base36 ints concatenated. First is a fresh random 31-bit;
/// second is `random ^ epoch_ms` truncated to 63 bits, abs'd. Caller passes
/// `epoch_ms` so tests can pin the value.
pub fn generate_cb(rng: &mut impl Rng, epoch_ms: u64) -> String {
    let a: u32 = rng.random_range(0..=i32::MAX as u32);
    let r2: u64 = rng.random_range(0..=i64::MAX as u64);
    let b = (r2 ^ epoch_ms) & 0x7FFF_FFFF_FFFF_FFFF;
    format!("{}{}", to_base36(a as u64), to_base36(b))
}

/// Encode the page origin into the `co` query parameter that the anchor
/// endpoint expects: base64 of `scheme://host:443`, with `=` padding
/// replaced by `.`. Port of `encode_co` from the reference.
///
/// Returns `None` when the URL has no host (data:, about:blank).
pub fn encode_co(url: &url::Url) -> Option<String> {
    let host = url.host_str()?;
    let scheme = url.scheme();
    let origin = format!("{}://{}:443", scheme, host);
    let b64 = general_purpose::STANDARD.encode(origin.as_bytes());
    Some(b64.replace('=', "."))
}

/// Per-byte cipher applied to the `oz` JSON before sending to the
/// `api2/reload` endpoint. Port of `scramble_oz`:
/// ```text
/// out[0] = m
/// out[i+1] = (oz[i] + length + (z + m) * (i + m)) % 256
/// ```
/// then base64url-no-pad, prefixed with the literal `'0'`. `m` is a random
/// byte in `[0, 254]`; `z = timestamp_ms % 1_000_000`. Caller supplies both
/// for determinism in tests; runtime path generates them fresh.
pub fn scramble_oz(oz: &[u8], timestamp_ms: u64, m: u8, _rng: &mut impl Rng) -> String {
    let z = (timestamp_ms % 1_000_000) as u32;
    let length = oz.len() as u32;
    let m32 = m as u32;
    let mut out = Vec::with_capacity(oz.len() + 1);
    out.push(m);
    for (i, &b) in oz.iter().enumerate() {
        let i32_ = i as u32;
        // Modular arithmetic over u32 mirrors Python's `% 256` semantics
        // where intermediate overflow doesn't matter — we only keep the
        // low byte. wrapping_add handles the edge cases in u32.
        let mixed = (b as u32)
            .wrapping_add(length)
            .wrapping_add(z.wrapping_add(m32).wrapping_mul(i32_.wrapping_add(m32)));
        out.push((mixed & 0xff) as u8);
    }
    let b64 = general_purpose::URL_SAFE_NO_PAD.encode(&out);
    format!("0{}", b64)
}

/// Generate a random `m` byte for `scramble_oz`. Reference uses
/// `random.randint(0, 254)` — note the inclusive upper bound 254, NOT 255.
pub fn random_m_byte(rng: &mut impl Rng) -> u8 {
    rng.random_range(0u8..=254)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn base36_zero() {
        assert_eq!(to_base36(0), "0");
    }

    #[test]
    fn base36_known_values() {
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
        assert_eq!(to_base36(1295), "zz");
        assert_eq!(to_base36(1296), "100");
    }

    #[test]
    fn base36_max_u31() {
        // 2^31 - 1 = 2147483647 → "zik0zj" in base36.
        assert_eq!(to_base36(2147483647), "zik0zj");
    }

    #[test]
    fn encode_co_basic() {
        let u = url::Url::parse("https://example.com/path?x=1").unwrap();
        let co = encode_co(&u).unwrap();
        // origin = "https://example.com:443"
        // base64 = "aHR0cHM6Ly9leGFtcGxlLmNvbTo0NDM=" → "." padding
        assert_eq!(co, "aHR0cHM6Ly9leGFtcGxlLmNvbTo0NDM.");
    }

    #[test]
    fn encode_co_http_origin() {
        let u = url::Url::parse("http://test.local/").unwrap();
        let co = encode_co(&u).unwrap();
        // origin = "http://test.local:443" (21 bytes, no `=` padding because
        // 21 mod 3 == 0). The reference replaces `=` with `.`, so absence
        // of trailing `=` means absence of trailing `.` here. Verify the
        // round-trip: convert any `.` back to `=` and decode.
        let restored = co.replace('.', "=");
        let decoded = general_purpose::STANDARD.decode(&restored).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "http://test.local:443");
    }

    #[test]
    fn encode_co_no_host_returns_none() {
        let u = url::Url::parse("about:blank").unwrap();
        assert!(encode_co(&u).is_none());
    }

    #[test]
    fn cb_format_two_concatenated_b36() {
        let mut rng = StdRng::seed_from_u64(42);
        let cb = generate_cb(&mut rng, 1_700_000_000_000);
        // All chars ∈ base36 alphabet.
        assert!(cb.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(!cb.is_empty());
    }

    #[test]
    fn cb_deterministic_per_seed() {
        let mut a = StdRng::seed_from_u64(7);
        let mut b = StdRng::seed_from_u64(7);
        assert_eq!(generate_cb(&mut a, 1234), generate_cb(&mut b, 1234));
    }

    #[test]
    fn scramble_oz_starts_with_zero_prefix() {
        let mut rng = StdRng::seed_from_u64(1);
        let s = scramble_oz(b"hello world", 1_700_000_000_000, 42, &mut rng);
        assert!(s.starts_with('0'));
    }

    #[test]
    fn scramble_oz_first_byte_after_prefix_is_m() {
        // After the literal '0' prefix and base64url decode, first byte == m.
        let mut rng = StdRng::seed_from_u64(1);
        let s = scramble_oz(b"abcd", 0, 7, &mut rng);
        let body = &s[1..];
        let decoded = general_purpose::URL_SAFE_NO_PAD.decode(body).unwrap();
        assert_eq!(decoded[0], 7);
    }

    #[test]
    fn scramble_oz_deterministic_for_same_inputs() {
        let mut a = StdRng::seed_from_u64(99);
        let mut b = StdRng::seed_from_u64(99);
        let s1 = scramble_oz(b"payload-bytes", 1234567, 100, &mut a);
        let s2 = scramble_oz(b"payload-bytes", 1234567, 100, &mut b);
        assert_eq!(s1, s2);
    }

    #[test]
    fn scramble_oz_changes_with_m() {
        let mut rng = StdRng::seed_from_u64(0);
        let a = scramble_oz(b"data", 1, 10, &mut rng);
        let b = scramble_oz(b"data", 1, 11, &mut rng);
        assert_ne!(a, b);
    }
}
