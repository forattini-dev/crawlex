//! Shodan-style favicon fingerprint.
//!
//! Algorithm: base64-encode the favicon bytes with newline every 76 chars,
//! then MurmurHash3 x86 32-bit of the resulting string. Widely used to
//! cluster hosts by stack (cpanel, grafana, specific vendor SPAs, etc.).

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use murmur3::murmur3_32;
use std::io::Cursor;

pub fn favicon_mmh3(favicon_bytes: &[u8]) -> i32 {
    let b64 = STANDARD.encode(favicon_bytes);
    // Shodan inserts a newline after every 76 chars and a trailing newline.
    let mut formatted = String::with_capacity(b64.len() + b64.len() / 76 + 2);
    for (i, c) in b64.chars().enumerate() {
        if i > 0 && i % 76 == 0 {
            formatted.push('\n');
        }
        formatted.push(c);
    }
    formatted.push('\n');
    let mut cursor = Cursor::new(formatted.as_bytes());
    // murmur3_32 returns u32; Shodan stores signed — cast.
    murmur3_32(&mut cursor, 0).unwrap_or(0) as i32
}
