use bytes::Bytes;
use http::HeaderMap;
use std::io::Read;

use crate::{Error, Result};

/// Decode response bytes to UTF-8 `String` using a 3-tier charset ladder
/// (Firecrawl `engines/fetch/index.ts::decodeHtmlBuffer`, MIT):
///
/// 1. `Content-Type: ...; charset=X` wins when present.
/// 2. Otherwise, scan the first 1 KiB for `<meta charset="X">` or
///    `<meta http-equiv="Content-Type" content="text/html; charset=X">`.
/// 3. Fallback: UTF-8, lossy.
///
/// Returns the decoded `String` and the charset label that won, useful to
/// surface in `fetch.completed` NDJSON events later.
pub fn decode_html_to_string(headers: &HeaderMap, body: &[u8]) -> (String, String) {
    if let Some(label) = charset_from_content_type(headers) {
        if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes()) {
            let (out, _enc, _had_errors) = enc.decode(body);
            return (out.into_owned(), label);
        }
    }
    if let Some(label) = charset_from_meta_tag(&body[..body.len().min(1024)]) {
        if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes()) {
            let (out, _enc, _had_errors) = enc.decode(body);
            return (out.into_owned(), label);
        }
    }
    (String::from_utf8_lossy(body).into_owned(), "utf-8".into())
}

fn charset_from_content_type(headers: &HeaderMap) -> Option<String> {
    let ct = headers.get("content-type")?.to_str().ok()?;
    // "text/html; charset=windows-1252"
    for part in ct.split(';').skip(1) {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("charset=") {
            return Some(v.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

fn charset_from_meta_tag(head: &[u8]) -> Option<String> {
    // Charset labels are ASCII; the document body may not be UTF-8 yet —
    // use `from_utf8_lossy` so a stray non-ASCII byte before the meta tag
    // doesn't prevent detection.
    let s = String::from_utf8_lossy(head);
    let lower = s.to_ascii_lowercase();
    // Form 1: <meta charset="X">
    if let Some(idx) = lower.find("charset=") {
        // Skip past `charset=` to the value; bounded by `"` or `'` or ` ` or `>`.
        let after = &s[idx + "charset=".len()..];
        let after = after.trim_start_matches(|c| c == '"' || c == '\'');
        let end = after
            .find(|c: char| c == '"' || c == '\'' || c == ' ' || c == '>' || c == ';')
            .unwrap_or(after.len());
        let candidate = after[..end].trim();
        if !candidate.is_empty() {
            return Some(candidate.to_string());
        }
    }
    None
}

pub fn decode_body(headers: &HeaderMap, body: Bytes) -> Result<Bytes> {
    decode_body_limited(headers, body, None, usize::MAX)
}

pub fn decode_body_limited(
    headers: &HeaderMap,
    body: Bytes,
    max_decoded_body_bytes: Option<usize>,
    max_decompression_ratio: usize,
) -> Result<Bytes> {
    let Some(enc) = headers.get("content-encoding") else {
        enforce_decode_limits(
            body.len(),
            body.len().max(1),
            max_decoded_body_bytes,
            max_decompression_ratio.max(1),
        )?;
        return Ok(body);
    };
    let Ok(enc) = enc.to_str() else {
        enforce_decode_limits(
            body.len(),
            body.len().max(1),
            max_decoded_body_bytes,
            max_decompression_ratio.max(1),
        )?;
        return Ok(body);
    };
    let encoded_len = body.len().max(1);
    let ratio = max_decompression_ratio.max(1);
    let mut current = body.to_vec();
    // content-encoding may be comma-separated: apply in reverse (outer first).
    let algs: Vec<&str> = enc.split(',').map(|s| s.trim()).collect();
    for alg in algs.iter().rev() {
        current = match alg.to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" => decode_gzip(&current, max_decoded_body_bytes)?,
            "deflate" => decode_deflate(&current, max_decoded_body_bytes)?,
            "br" => decode_brotli(&current, max_decoded_body_bytes)?,
            "zstd" => decode_zstd(&current, max_decoded_body_bytes)?,
            "identity" | "" => current,
            other => {
                return Err(Error::Decompression(format!(
                    "unsupported content-encoding: {other}"
                )));
            }
        };
        enforce_decode_limits(current.len(), encoded_len, max_decoded_body_bytes, ratio)?;
    }
    Ok(Bytes::from(current))
}

fn enforce_decode_limits(
    len: usize,
    encoded_len: usize,
    max_decoded_body_bytes: Option<usize>,
    max_decompression_ratio: usize,
) -> Result<()> {
    if let Some(max) = max_decoded_body_bytes {
        if len > max {
            return Err(Error::DecodedBodyTooLarge { limit: max });
        }
    }
    if len > encoded_len.saturating_mul(max_decompression_ratio) {
        return Err(Error::DecompressionRatioTooLarge {
            encoded: encoded_len,
            decoded: len,
            ratio_limit: max_decompression_ratio,
        });
    }
    Ok(())
}

fn read_limited<R: std::io::Read>(reader: R, max: Option<usize>, label: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match max {
        Some(max) => {
            let mut limited = reader.take(max as u64 + 1);
            limited
                .read_to_end(&mut out)
                .map_err(|e| Error::Http(format!("{label}: {e}")))?;
            if out.len() > max {
                return Err(Error::DecodedBodyTooLarge { limit: max });
            }
        }
        None => {
            let mut reader = reader;
            reader
                .read_to_end(&mut out)
                .map_err(|e| Error::Http(format!("{label}: {e}")))?;
        }
    }
    Ok(out)
}

fn decode_gzip(data: &[u8], max: Option<usize>) -> Result<Vec<u8>> {
    read_limited(flate2::read::MultiGzDecoder::new(data), max, "gzip")
}

fn decode_deflate(data: &[u8], max: Option<usize>) -> Result<Vec<u8>> {
    read_limited(flate2::read::ZlibDecoder::new(data), max, "deflate")
}

fn decode_brotli(data: &[u8], max: Option<usize>) -> Result<Vec<u8>> {
    read_limited(brotli::Decompressor::new(data, 4096), max, "brotli")
}

fn decode_zstd(data: &[u8], max: Option<usize>) -> Result<Vec<u8>> {
    let decoder =
        zstd::stream::Decoder::new(data).map_err(|e| Error::Http(format!("zstd init: {e}")))?;
    read_limited(decoder, max, "zstd")
}
