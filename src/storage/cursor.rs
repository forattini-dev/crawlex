//! Slice 8 — opaque cursor token for the SDK results read path.
//!
//! Token is a URL-safe base64 of a small versioned JSON struct. The
//! raw rowid is wrapped (not concatenated, not exposed verbatim) so
//! callers cannot synthesize cursors from outside knowledge.
//!
//! Versioning rule: decoders accept any `v` in `1..=CURSOR_VERSION`
//! and reject unknown (higher) versions with a clear error. This lets
//! the encoder bump fields under a new version while older clients
//! still issue v1 tokens that the server keeps honoring.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Highest cursor version this build can mint. Bump when adding
/// fields that older decoders would not understand.
pub const CURSOR_VERSION: u32 = 1;

/// Opaque pagination state for `pages list`. Carries the last rowid
/// the consumer has already seen plus the status filter the cursor was
/// minted against — server-side, the filter on the new request must
/// match (mixing filters mid-iteration corrupts ordering).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageCursor {
    pub v: u32,
    pub after_rowid: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl PageCursor {
    pub fn new(after_rowid: i64, status: Option<String>) -> Self {
        Self {
            v: CURSOR_VERSION,
            after_rowid,
            status,
        }
    }

    /// URL-safe base64 of the JSON encoding. No padding.
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("cursor serialize");
        URL_SAFE_NO_PAD.encode(json)
    }

    /// Decode a token. Returns a clear error for malformed tokens or
    /// versions newer than this build understands.
    pub fn decode(token: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token.as_bytes())
            .map_err(|e| Error::Config(format!("cursor base64: {e}")))?;
        // Read `v` first so an unknown version yields a focused error
        // before serde tries (and possibly fails) to fill the full
        // struct.
        let head: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Config(format!("cursor json: {e}")))?;
        let v = head
            .get("v")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| Error::Config("cursor missing version field `v`".into()))?
            as u32;
        if v == 0 || v > CURSOR_VERSION {
            return Err(Error::Config(format!(
                "cursor version {v} not supported (this build understands up to {CURSOR_VERSION})"
            )));
        }
        let cur: PageCursor = serde_json::from_value(head)
            .map_err(|e| Error::Config(format!("cursor decode: {e}")))?;
        Ok(cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_status() {
        let c = PageCursor::new(42, Some("errored".into()));
        let token = c.encode();
        let back = PageCursor::decode(&token).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn roundtrip_no_status() {
        let c = PageCursor::new(0, None);
        let token = c.encode();
        assert_eq!(PageCursor::decode(&token).unwrap(), c);
    }

    #[test]
    fn opaque_token_does_not_leak_rowid_literally() {
        // The rowid `9999999` must not appear as a decimal substring of
        // the base64 token — would tell us the encoder accidentally
        // stringified the rowid into the token surface.
        let token = PageCursor::new(9_999_999, None).encode();
        assert!(!token.contains("9999999"), "raw rowid leaked: {token}");
    }

    #[test]
    fn unknown_version_rejected() {
        let bad = serde_json::json!({"v": 999, "after_rowid": 1});
        let token = URL_SAFE_NO_PAD.encode(bad.to_string());
        let err = PageCursor::decode(&token).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("999"), "{msg}");
        assert!(msg.contains("not supported"), "{msg}");
    }

    #[test]
    fn zero_version_rejected() {
        let bad = serde_json::json!({"v": 0, "after_rowid": 1});
        let token = URL_SAFE_NO_PAD.encode(bad.to_string());
        assert!(PageCursor::decode(&token).is_err());
    }

    #[test]
    fn malformed_base64_rejected() {
        assert!(PageCursor::decode("!!!not base64!!!").is_err());
    }

    #[test]
    fn malformed_json_rejected() {
        let token = URL_SAFE_NO_PAD.encode(b"not json");
        assert!(PageCursor::decode(&token).is_err());
    }
}
