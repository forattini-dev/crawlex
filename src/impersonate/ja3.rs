//! ClientHello parser — JA3 + JA4 + Chrome-specific deep inspection.
//!
//! Ported and expanded from `../redblue/src/intelligence/tls-fingerprint.rs`.
//! Adds the fields that matter for Chrome impersonation verification but
//! are absent in vanilla JA3/JA4: ALPS payload bytes, cert_compression
//! algorithm list, ECH extension presence, supported_versions exact list,
//! on-wire extension order (pre-sort), key_share named groups.
//!
//! The harness under `tests/tls_clienthello.rs` captures our crawler's
//! first TLS record against a local canary, parses it here, and asserts
//! the parsed fields match known Chrome values — closed-loop verification
//! that our boringssl knobs actually produce a Chrome ClientHello.

#![allow(dead_code)] // Consumed by the integration test + future telemetry.

use std::fmt::Write;

/// GREASE values per RFC 8701: 0x0a0a, 0x1a1a, 0x2a2a, ..., 0xfafa.
/// High == low, low nibble == 0xA.
fn is_grease(v: u16) -> bool {
    let high = (v >> 8) & 0xff;
    let low = v & 0xff;
    high == low && (low & 0x0f) == 0x0a
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("short read at offset {offset}: need {need} bytes, have {have}")]
    ShortRead {
        offset: usize,
        need: usize,
        have: usize,
    },
    #[error("not a TLS handshake record (byte0={0:#04x})")]
    NotHandshake(u8),
    #[error("not a ClientHello (handshake type={0:#04x})")]
    NotClientHello(u8),
    #[error("malformed extension: type={ext_type:#06x}, declared length {declared}, remaining {remaining}")]
    BadExtensionLen {
        ext_type: u16,
        declared: usize,
        remaining: usize,
    },
}

/// Fully parsed Chrome-relevant ClientHello contents.
#[derive(Debug, Clone, Default)]
pub struct ClientHello {
    pub legacy_version: u16,

    /// Cipher suites in on-wire order (GREASE preserved — caller decides
    /// whether to strip for JA3/JA4).
    pub cipher_suites_raw: Vec<u16>,
    /// Cipher suites with GREASE stripped — the JA3/JA4 input.
    pub cipher_suites: Vec<u16>,

    /// Extension types in on-wire order (GREASE preserved). Chrome
    /// permute_extensions makes this order randomized per-connection, so
    /// on-wire order tests against known-Chrome are unstable; compare the
    /// sorted list via `extensions_sorted` for a stable fingerprint.
    pub extensions_raw: Vec<u16>,
    pub extensions: Vec<u16>,
    pub extensions_sorted: Vec<u16>,

    /// `supported_groups` (ext 10) — includes Kyber/MLKEM, X25519, P-256, ...
    pub supported_groups: Vec<u16>,
    /// `ec_point_formats` (ext 11) — usually \[0\] uncompressed for Chrome.
    pub ec_point_formats: Vec<u8>,
    /// `signature_algorithms` (ext 13) — Chrome M120+ includes ed25519.
    pub signature_algorithms: Vec<u16>,
    /// `supported_versions` (ext 43) — TLS 1.3 expected as first non-GREASE.
    pub supported_versions: Vec<u16>,
    /// `application_layer_protocol_negotiation` (ext 16) — ALPN values as
    /// ASCII strings, e.g. \["h2","http/1.1"\].
    pub alpn: Vec<String>,

    /// `psk_key_exchange_modes` (ext 45).
    pub psk_key_exchange_modes: Vec<u8>,

    /// `key_share` (ext 51) — listed named groups (no key material; we only
    /// care which curves Chrome pre-generates shares for).
    pub key_share_groups: Vec<u16>,

    /// `compress_certificate` (ext 27) — algorithm IDs advertised.
    /// Chrome M131+ ships `brotli(2), zlib(1), zstd(3)` (order varies).
    pub cert_compression_algs: Vec<u16>,

    /// `application_settings` (ext 17513 = 0x4469) — ALPS. Non-empty bytes
    /// = we send an h2 SETTINGS payload inside. Chrome's payload is a
    /// real h2 SETTINGS frame body.
    pub alps_payload_by_proto: Vec<(String, Vec<u8>)>,

    /// `encrypted_client_hello` (ext 65037 = 0xfe0d) — presence == ECH
    /// GREASE active (Chrome M117+).
    pub has_ech_ext: bool,
    pub ech_payload_len: usize,

    /// `server_name` (ext 0) — SNI hostname if any.
    pub sni: Option<String>,

    /// `renegotiation_info` (ext 65281) presence.
    pub has_renegotiation_info: bool,
    /// `extended_master_secret` (ext 23) presence.
    pub has_extended_master_secret: bool,
    /// `session_ticket` (ext 35) presence.
    pub has_session_ticket: bool,

    /// Raw ClientHello bytes captured, for byte-diff against known good.
    pub raw: Vec<u8>,
}

impl ClientHello {
    /// Parse a TLS record that starts with `0x16 0x03 0x01` (TLS record
    /// type=Handshake, legacy_version=TLS 1.0). The record may itself be
    /// fragmented across multiple TCP reads; callers pass the concatenated
    /// buffer.
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut ch = ClientHello {
            raw: data.to_vec(),
            ..Default::default()
        };
        let mut o = 0usize;
        need(data, o, 5)?;
        if data[o] != 0x16 {
            return Err(ParseError::NotHandshake(data[o]));
        }
        o += 5; // skip record header (type + legacy_version + record length)

        need(data, o, 4)?;
        if data[o] != 0x01 {
            return Err(ParseError::NotClientHello(data[o]));
        }
        o += 4; // skip handshake header (type + length-u24)

        need(data, o, 2)?;
        ch.legacy_version = u16::from_be_bytes([data[o], data[o + 1]]);
        o += 2;
        need(data, o, 32)?;
        o += 32; // random

        need(data, o, 1)?;
        let sid_len = data[o] as usize;
        o += 1;
        need(data, o, sid_len)?;
        o += sid_len;

        need(data, o, 2)?;
        let cipher_len = u16::from_be_bytes([data[o], data[o + 1]]) as usize;
        o += 2;
        need(data, o, cipher_len)?;
        for i in (0..cipher_len).step_by(2) {
            let c = u16::from_be_bytes([data[o + i], data[o + i + 1]]);
            ch.cipher_suites_raw.push(c);
            if !is_grease(c) {
                ch.cipher_suites.push(c);
            }
        }
        o += cipher_len;

        need(data, o, 1)?;
        let comp_len = data[o] as usize;
        o += 1 + comp_len;

        need(data, o, 2)?;
        let ext_total = u16::from_be_bytes([data[o], data[o + 1]]) as usize;
        o += 2;
        let ext_end = o + ext_total;
        need(data, ext_end, 0)?;

        while o + 4 <= ext_end {
            let t = u16::from_be_bytes([data[o], data[o + 1]]);
            let l = u16::from_be_bytes([data[o + 2], data[o + 3]]) as usize;
            o += 4;
            if o + l > ext_end {
                return Err(ParseError::BadExtensionLen {
                    ext_type: t,
                    declared: l,
                    remaining: ext_end - o,
                });
            }
            let payload = &data[o..o + l];
            ch.extensions_raw.push(t);
            if !is_grease(t) {
                ch.extensions.push(t);
            }
            parse_extension(&mut ch, t, payload)?;
            o += l;
        }

        ch.extensions_sorted = ch.extensions.clone();
        ch.extensions_sorted.sort_unstable();
        Ok(ch)
    }

    /// Canonical JA3 string: `version,ciphers,extensions,groups,point_formats`.
    /// GREASE stripped. Extensions in on-wire order (JA3's definition).
    pub fn ja3_string(&self) -> String {
        let mut s = String::with_capacity(256);
        let _ = write!(s, "{}", self.legacy_version);
        s.push(',');
        join_u16(&mut s, &self.cipher_suites, '-');
        s.push(',');
        join_u16(&mut s, &self.extensions, '-');
        s.push(',');
        join_u16(&mut s, &self.supported_groups, '-');
        s.push(',');
        let fmts: Vec<u16> = self.ec_point_formats.iter().map(|&b| b as u16).collect();
        join_u16(&mut s, &fmts, '-');
        s
    }

    /// JA4_a prefix: `<proto><tls><sni><cipher_count><ext_count><alpn>`.
    /// Full JA4 = `<a>_<sha256(sorted_ciphers)[..12]>_<sha256(sorted_ext_sigalgs)[..12]>`
    /// but we return the human-readable leading portion; the hash suffixes
    /// are on `ja4_b` / `ja4_c`.
    pub fn ja4_a(&self) -> String {
        // TLS version = highest value in `supported_versions` if present,
        // otherwise legacy_version. Chrome ships supported_versions with
        // 0x0304 (TLS 1.3) as the real version; legacy_version is 0x0303
        // for compatibility.
        let tls_code = self
            .supported_versions
            .iter()
            .copied()
            .filter(|v| !is_grease(*v))
            .max()
            .unwrap_or(self.legacy_version);
        let tls_pair = match tls_code {
            0x0304 => "13",
            0x0303 => "12",
            0x0302 => "11",
            0x0301 => "10",
            _ => "00",
        };
        let sni = if self.sni.is_some() { 'd' } else { 'i' };
        let proto = 't'; // TCP (QUIC would be 'q')
        let cipher_count = self.cipher_suites.len().min(99);
        let ext_count = self.extensions.len().min(99);
        // ALPN token: first 2 chars of first ALPN offered (Chrome: "h2" -> "h2",
        // "http/1.1" -> "h1"). JA4 spec takes first+last char of the ALPN
        // value; we approximate with the common Chrome case.
        let alpn = match self.alpn.first().map(|s| s.as_str()) {
            Some("h2") => "h2".to_string(),
            Some("http/1.1") => "h1".to_string(),
            Some(other) if other.len() >= 2 => {
                let bytes = other.as_bytes();
                format!("{}{}", bytes[0] as char, bytes[bytes.len() - 1] as char)
            }
            _ => "00".into(),
        };
        format!("{proto}{tls_pair}{sni}{cipher_count:02}{ext_count:02}{alpn}")
    }

    /// Emit a human-readable diff-ready summary used by the test harness
    /// failure messages. Stable across runs (no hashes, no random).
    pub fn summary(&self) -> String {
        let mut s = String::with_capacity(1024);
        let _ = writeln!(s, "JA3   = {}", self.ja3_string());
        let _ = writeln!(s, "JA4_a = {}", self.ja4_a());
        let _ = writeln!(
            s,
            "cipher_suites[{}] = {:?}",
            self.cipher_suites.len(),
            self.cipher_suites
        );
        let _ = writeln!(
            s,
            "extensions_sorted[{}] = {:?}",
            self.extensions_sorted.len(),
            self.extensions_sorted
        );
        let _ = writeln!(s, "supported_groups = {:?}", self.supported_groups);
        let _ = writeln!(s, "signature_algos  = {:?}", self.signature_algorithms);
        let _ = writeln!(s, "supported_versions = {:?}", self.supported_versions);
        let _ = writeln!(s, "alpn = {:?}", self.alpn);
        let _ = writeln!(s, "key_share_groups = {:?}", self.key_share_groups);
        let _ = writeln!(
            s,
            "cert_compression_algs = {:?}",
            self.cert_compression_algs
        );
        let _ = writeln!(
            s,
            "psk_key_exchange_modes = {:?}",
            self.psk_key_exchange_modes
        );
        let _ = writeln!(
            s,
            "alps = {:?}",
            self.alps_payload_by_proto
                .iter()
                .map(|(p, b)| (p.clone(), b.len()))
                .collect::<Vec<_>>()
        );
        let _ = writeln!(
            s,
            "ech_ext present={} payload_len={}",
            self.has_ech_ext, self.ech_payload_len
        );
        let _ = writeln!(
            s,
            "ems={} renego_info={} session_ticket={} sni={:?}",
            self.has_extended_master_secret,
            self.has_renegotiation_info,
            self.has_session_ticket,
            self.sni
        );
        s
    }
}

fn join_u16(s: &mut String, xs: &[u16], sep: char) {
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            s.push(sep);
        }
        let _ = write!(s, "{x}");
    }
}

fn need(data: &[u8], offset: usize, n: usize) -> Result<(), ParseError> {
    if offset + n > data.len() {
        return Err(ParseError::ShortRead {
            offset,
            need: n,
            have: data.len(),
        });
    }
    Ok(())
}

fn parse_extension(ch: &mut ClientHello, t: u16, p: &[u8]) -> Result<(), ParseError> {
    match t {
        0 => {
            // server_name: list length (2), then entries of {type(1), len(2), name}
            if p.len() >= 5 {
                let name_type = p[2];
                if name_type == 0 {
                    let nlen = u16::from_be_bytes([p[3], p[4]]) as usize;
                    if p.len() >= 5 + nlen {
                        ch.sni = Some(String::from_utf8_lossy(&p[5..5 + nlen]).into_owned());
                    }
                }
            }
        }
        10 => {
            // supported_groups: list-len(2) + entries of u16
            if p.len() >= 2 {
                let l = u16::from_be_bytes([p[0], p[1]]) as usize;
                let mut i = 2;
                while i + 2 <= 2 + l && i + 2 <= p.len() {
                    let g = u16::from_be_bytes([p[i], p[i + 1]]);
                    if !is_grease(g) {
                        ch.supported_groups.push(g);
                    }
                    i += 2;
                }
            }
        }
        11 => {
            if !p.is_empty() {
                let l = p[0] as usize;
                for i in 0..l {
                    if 1 + i < p.len() {
                        ch.ec_point_formats.push(p[1 + i]);
                    }
                }
            }
        }
        13 => {
            if p.len() >= 2 {
                let l = u16::from_be_bytes([p[0], p[1]]) as usize;
                let mut i = 2;
                while i + 2 <= 2 + l && i + 2 <= p.len() {
                    ch.signature_algorithms
                        .push(u16::from_be_bytes([p[i], p[i + 1]]));
                    i += 2;
                }
            }
        }
        16 => {
            // ALPN: list-len(2) + entries of {len(1), bytes}
            if p.len() >= 2 {
                let l = u16::from_be_bytes([p[0], p[1]]) as usize;
                let mut i = 2;
                while i < 2 + l && i < p.len() {
                    let item_len = p[i] as usize;
                    i += 1;
                    if i + item_len <= p.len() {
                        ch.alpn
                            .push(String::from_utf8_lossy(&p[i..i + item_len]).into_owned());
                    }
                    i += item_len;
                }
            }
        }
        23 => ch.has_extended_master_secret = true,
        27 => {
            // compress_certificate: list-len(1), then u16 algorithm IDs.
            if !p.is_empty() {
                let l = p[0] as usize;
                let mut i = 1;
                while i + 2 <= 1 + l && i + 2 <= p.len() {
                    ch.cert_compression_algs
                        .push(u16::from_be_bytes([p[i], p[i + 1]]));
                    i += 2;
                }
            }
        }
        35 => ch.has_session_ticket = true,
        43 => {
            // supported_versions (client): list-len(1), u16 versions.
            if !p.is_empty() {
                let l = p[0] as usize;
                let mut i = 1;
                while i + 2 <= 1 + l && i + 2 <= p.len() {
                    let v = u16::from_be_bytes([p[i], p[i + 1]]);
                    ch.supported_versions.push(v);
                    i += 2;
                }
            }
        }
        45 => {
            // psk_key_exchange_modes: len(1), bytes.
            if !p.is_empty() {
                let l = p[0] as usize;
                for i in 0..l {
                    if 1 + i < p.len() {
                        ch.psk_key_exchange_modes.push(p[1 + i]);
                    }
                }
            }
        }
        51 => {
            // key_share: list-len(2), entries of {group:u16, len:u16, bytes}.
            if p.len() >= 2 {
                let l = u16::from_be_bytes([p[0], p[1]]) as usize;
                let mut i = 2;
                while i + 4 <= 2 + l && i + 4 <= p.len() {
                    let g = u16::from_be_bytes([p[i], p[i + 1]]);
                    let kl = u16::from_be_bytes([p[i + 2], p[i + 3]]) as usize;
                    if !is_grease(g) {
                        ch.key_share_groups.push(g);
                    }
                    i += 4 + kl;
                }
            }
        }
        17513 | 17613 => {
            // application_settings (ALPS). Two codepoints exist in the
            // wild: per BoringSSL `TLSEXT_TYPE_application_settings`
            // bindings, 17613 (0x44CD) is the *current* codepoint and
            // 17513 (0x4469, suffixed `_old` in the header) is the legacy
            // one. Chrome and our vendored BoringSSL both emit 17613 via
            // `SSL_add_application_settings`. We accept either so the
            // parser survives a future switch.
            //
            // In *ClientHello* the extension only carries the list of
            // protocol names the client wants to ALPS. The actual h2
            // SETTINGS payload travels later inside EncryptedExtensions
            // (TLS 1.3) or the server's reply; it is never visible in
            // plaintext here. Wire format:
            //   ext_data = { list_len:u16, { name_len:u8, name_bytes }* }
            if p.len() >= 2 {
                let list_len = u16::from_be_bytes([p[0], p[1]]) as usize;
                let end = (2 + list_len).min(p.len());
                let mut i = 2;
                while i < end {
                    let plen = p[i] as usize;
                    i += 1;
                    if i + plen > end {
                        break;
                    }
                    let proto = String::from_utf8_lossy(&p[i..i + plen]).into_owned();
                    i += plen;
                    // Payload Vec stays empty — the ClientHello side of
                    // ALPS doesn't carry one. The unit test under
                    // `impersonate::tls::tests::alps_h2_settings_layout_matches_chrome`
                    // already verifies our build_alps_h2_settings() produces
                    // the exact 24 bytes Chrome advertises for h2.
                    ch.alps_payload_by_proto.push((proto, Vec::new()));
                }
            }
        }
        65037 => {
            ch.has_ech_ext = true;
            ch.ech_payload_len = p.len();
        }
        65281 => ch.has_renegotiation_info = true,
        _ => {}
    }
    Ok(())
}

/// Canonical Chrome-fingerprint summary our TLS stack produces on the wire.
/// One-line digest covering the values a detector buckets on: JA4_a,
/// non-GREASE cipher count, post-quantum hybrid group, cert_compression
/// algorithms, ECH GREASE presence.
///
/// This is the single source of truth for "what Chrome version are we
/// impersonating?" Regression tests under `tests/tls_clienthello.rs`
/// compare this string to the harness-observed ClientHello; any drift
/// (someone upgrading boringssl, removing a cert_compression alg,
/// disabling ECH grease) breaks the test loudly.
pub fn current_chrome_fingerprint_summary(profile: crate::impersonate::Profile) -> &'static str {
    use crate::impersonate::catalog::Browser;
    // The summary string is a coarse digest used as a regression marker
    // in tests. With the catalog-driven Profile enum, every Chrome/Chromium
    // major shares the same shape (TLS 1.3, 11 ciphers, MLKEM768 PQ,
    // brotli/zlib/zstd cert compression, ECH grease) for everything 132+.
    // Older majors (≤131) and Firefox/Safari get their own marker so a
    // regression that re-routes them through Chrome's wire config is
    // visible at test time.
    let (browser, major, _) = profile.parts();
    match (browser, major) {
        (Browser::Chrome | Browser::Chromium, m) if m >= 132 => {
            "t13i1113h2|ciphers=11|pq=X25519MLKEM768|cert_comp=[2,1,3]|ech=1"
        }
        (Browser::Chrome | Browser::Chromium, _) => {
            "t13i1113h2|ciphers=11|pq=X25519Kyber768|cert_comp=[2,1,3]|ech=1"
        }
        (Browser::Firefox, _) => "t13i1014h2|ciphers=12|pq=none|cert_comp=[]|ech=0",
        (Browser::Edge, _) => "t13i1113h2|ciphers=11|pq=X25519MLKEM768|cert_comp=[2,1,3]|ech=1",
        _ => "t13i????h?|ciphers=?|pq=?|cert_comp=?|ech=?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grease_detection_rfc8701() {
        for &g in &[0x0a0au16, 0x1a1a, 0x2a2a, 0xcaca, 0xfafa] {
            assert!(is_grease(g), "{g:#06x}");
        }
        for &g in &[0x0000u16, 0x1301, 0x1a1b, 0x1b1a] {
            assert!(!is_grease(g), "{g:#06x}");
        }
    }

    // A minimal hand-crafted ClientHello: record(5) + handshake(4) + body.
    // Legacy TLS 1.2, 1 cipher (0x1301 TLS_AES_128_GCM_SHA256), empty
    // compression, one extension: server_name "example.com".
    fn tiny_ch() -> Vec<u8> {
        let body_cipher = [0x00, 0x02, 0x13, 0x01];
        let random = [0u8; 32];
        let sid = [0u8; 1];
        let compression = [0x01, 0x00];
        // server_name for "example.com"
        let name = b"example.com";
        let mut sn_ext = Vec::new();
        // list-len(2) + type(1) + name-len(2) + name
        let list_len = 1 + 2 + name.len();
        sn_ext.extend_from_slice(&(list_len as u16).to_be_bytes());
        sn_ext.push(0);
        sn_ext.extend_from_slice(&(name.len() as u16).to_be_bytes());
        sn_ext.extend_from_slice(name);
        let mut extensions = Vec::new();
        extensions.extend_from_slice(&0u16.to_be_bytes()); // ext_type=server_name
        extensions.extend_from_slice(&(sn_ext.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&sn_ext);

        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // legacy_version = TLS1.2
        body.extend_from_slice(&random);
        body.extend_from_slice(&sid);
        body.extend_from_slice(&body_cipher);
        body.extend_from_slice(&compression);
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(&extensions);

        let mut hs = Vec::new();
        hs.push(0x01);
        let body_len = body.len() as u32;
        hs.push(((body_len >> 16) & 0xff) as u8);
        hs.push(((body_len >> 8) & 0xff) as u8);
        hs.push((body_len & 0xff) as u8);
        hs.extend_from_slice(&body);

        let mut rec = Vec::new();
        rec.push(0x16);
        rec.extend_from_slice(&[0x03, 0x01]);
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn parse_tiny_hello_roundtrip() {
        let buf = tiny_ch();
        let ch = ClientHello::parse(&buf).expect("parse");
        assert_eq!(ch.legacy_version, 0x0303);
        assert_eq!(ch.cipher_suites, vec![0x1301]);
        assert_eq!(ch.extensions, vec![0]);
        assert_eq!(ch.sni.as_deref(), Some("example.com"));
    }

    #[test]
    fn parse_rejects_non_handshake() {
        let mut buf = tiny_ch();
        buf[0] = 0x17; // application_data
        assert!(matches!(
            ClientHello::parse(&buf),
            Err(ParseError::NotHandshake(0x17))
        ));
    }
}
