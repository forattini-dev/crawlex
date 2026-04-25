//! Tests for the 3-tier charset ladder.

use bytes::Bytes;
use crawlex::impersonate::decode::{decode_body_limited, decode_html_to_string};
use crawlex::Error;
use flate2::write::GzEncoder;
use flate2::Compression;
use http::{HeaderMap, HeaderValue};
use std::io::Write;

fn hdr(content_type: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("content-type", HeaderValue::from_str(content_type).unwrap());
    h
}

#[test]
fn content_type_charset_wins() {
    let body = b"\xe9 eacute in latin-1";
    let (s, label) = decode_html_to_string(&hdr("text/html; charset=iso-8859-1"), body);
    assert_eq!(label, "iso-8859-1");
    assert!(s.contains('é'));
}

#[test]
fn meta_charset_used_when_header_absent() {
    // Latin-1 file with a meta charset in the head.
    let head = r#"<!doctype html><html><head><meta charset="iso-8859-1"></head><body>"#;
    let mut body = head.as_bytes().to_vec();
    body.extend_from_slice(b"\xe9");
    body.extend_from_slice(b"</body></html>");
    let h = HeaderMap::new(); // no Content-Type
    let (s, label) = decode_html_to_string(&h, &body);
    assert_eq!(label.to_lowercase(), "iso-8859-1");
    assert!(s.contains('é'));
}

#[test]
fn utf8_fallback_when_nothing_declared() {
    let body = "café".as_bytes();
    let h = HeaderMap::new();
    let (s, label) = decode_html_to_string(&h, body);
    assert_eq!(label, "utf-8");
    assert_eq!(s, "café");
}

#[test]
fn quotes_stripped_around_charset_value() {
    let body = b"<html></html>";
    let (_, label) = decode_html_to_string(&hdr(r#"text/html; charset="utf-8""#), body);
    assert_eq!(label, "utf-8");
}

#[test]
fn decode_body_limited_rejects_large_decompressed_payload() {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&vec![b'a'; 4096]).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut h = HeaderMap::new();
    h.insert("content-encoding", HeaderValue::from_static("gzip"));

    let err = decode_body_limited(&h, Bytes::from(compressed), Some(1024), 100)
        .expect_err("decoded cap should reject");
    assert!(matches!(err, Error::DecodedBodyTooLarge { limit: 1024 }));
    assert_eq!(err.kind(), "decoded-body-too-large");
}

#[test]
fn decode_body_limited_rejects_large_identity_payload() {
    let h = HeaderMap::new();
    let err = decode_body_limited(&h, Bytes::from_static(b"abcdef"), Some(3), 100)
        .expect_err("identity body should still honor decoded cap");
    assert!(matches!(err, Error::DecodedBodyTooLarge { limit: 3 }));
    assert_eq!(err.kind(), "decoded-body-too-large");
}

#[test]
fn decode_body_limited_rejects_large_decompression_ratio() {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&vec![b'a'; 4096]).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut h = HeaderMap::new();
    h.insert("content-encoding", HeaderValue::from_static("gzip"));

    let err = decode_body_limited(&h, Bytes::from(compressed), None, 2)
        .expect_err("ratio cap should reject");
    assert!(matches!(
        err,
        Error::DecompressionRatioTooLarge {
            decoded: 4096,
            ratio_limit: 2,
            ..
        }
    ));
    assert_eq!(err.kind(), "decompression-ratio-too-large");
}
