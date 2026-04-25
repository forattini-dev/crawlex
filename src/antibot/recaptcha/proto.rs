//! Minimal protobuf encoder for the reCAPTCHA v3 invisible reload payload.
//!
//! The reCAPTCHA reload endpoint accepts a protobuf-encoded body
//! (`Content-Type: application/x-protobuffer`) where the field numbers are
//! known — they're hard-coded in the public grecaptcha JS bundle. We only
//! need encoding (no decoding), and only varint (wire 0), fixed64 (wire 1),
//! and length-delimited (wire 2). This is ~30 lines vs pulling in `prost`
//! + a `.proto` definition that would have to track Google's churn.
//!
//! Port of `recaptchav3/core/protobuf.py` from the reference solver. Field
//! numbers are passed as `u32`; sorting is the caller's responsibility (the
//! reference implementation sorts by field number, which we mirror).
//!
//! Wire format reference: <https://protobuf.dev/programming-guides/encoding/>

use std::collections::BTreeMap;

/// One encoded value. `Bytes` covers strings (caller pre-encoded as UTF-8)
/// and raw bytes; `List` lets a single field number repeat.
pub enum Value {
    Varint(u64),
    Fixed64(f64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
}

impl Value {
    /// Convenience for the common case `Bytes(s.into_bytes())`. Named
    /// `from_string` (not `from_str`) so it doesn't shadow
    /// `std::str::FromStr::from_str` semantics — this constructor cannot
    /// fail, so a `Result`-returning trait shape would be misleading.
    pub fn from_string(s: &str) -> Self {
        Value::Bytes(s.as_bytes().to_vec())
    }

    /// Convenience for nested integers — varint encoding accepts u64 / i64.
    pub fn from_u64(v: u64) -> Self {
        Value::Varint(v)
    }
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn write_field(out: &mut Vec<u8>, field_number: u32, value: &Value) {
    let tag = (field_number as u64) << 3;
    match value {
        Value::Varint(v) => {
            write_varint(out, tag); // wire 0
            write_varint(out, *v);
        }
        Value::Fixed64(f) => {
            write_varint(out, tag | 1);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Bytes(b) => {
            write_varint(out, tag | 2);
            write_varint(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::List(items) => {
            // Repeated field — emit each item with the same tag.
            for item in items {
                write_field(out, field_number, item);
            }
        }
    }
}

/// Encode a sorted-by-field-number map of values into a single protobuf
/// blob. Returns owned `Vec<u8>` ready to ship over HTTP.
pub fn encode(fields: &BTreeMap<u32, Value>) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    for (num, val) in fields {
        write_field(&mut out, *num, val);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip_zero() {
        let mut out = Vec::new();
        write_varint(&mut out, 0);
        assert_eq!(out, vec![0]);
    }

    #[test]
    fn varint_single_byte() {
        let mut out = Vec::new();
        write_varint(&mut out, 0x7f);
        assert_eq!(out, vec![0x7f]);
    }

    #[test]
    fn varint_multi_byte() {
        // 300 = 0xAC 0x02 in varint.
        let mut out = Vec::new();
        write_varint(&mut out, 300);
        assert_eq!(out, vec![0xAC, 0x02]);
    }

    #[test]
    fn encode_string_field() {
        // Field 1, "test" — tag = (1 << 3) | 2 = 0x0A.
        let mut m = BTreeMap::new();
        m.insert(1, Value::from_string("test"));
        let out = encode(&m);
        assert_eq!(out, vec![0x0A, 0x04, b't', b'e', b's', b't']);
    }

    #[test]
    fn encode_int_field() {
        // Field 2, varint 42 — tag = (2 << 3) | 0 = 0x10.
        let mut m = BTreeMap::new();
        m.insert(2, Value::Varint(42));
        let out = encode(&m);
        assert_eq!(out, vec![0x10, 42]);
    }

    #[test]
    fn fields_emitted_in_field_number_order() {
        // Inserting field 5 before field 1 must still produce field 1 first
        // (BTreeMap sorts). This matters because the reference Python
        // implementation sorts before encoding and the receiving server
        // accepts the canonical order.
        let mut m = BTreeMap::new();
        m.insert(5, Value::Varint(1));
        m.insert(1, Value::Varint(2));
        let out = encode(&m);
        // field 1 → tag 0x08, value 2; field 5 → tag 0x28, value 1.
        assert_eq!(out, vec![0x08, 0x02, 0x28, 0x01]);
    }

    #[test]
    fn list_repeats_field_number() {
        let mut m = BTreeMap::new();
        m.insert(
            3,
            Value::List(vec![Value::from_string("a"), Value::from_string("b")]),
        );
        let out = encode(&m);
        // Each repeats: tag 0x1A, len 1, byte; tag 0x1A, len 1, byte.
        assert_eq!(out, vec![0x1A, 0x01, b'a', 0x1A, 0x01, b'b']);
    }
}
