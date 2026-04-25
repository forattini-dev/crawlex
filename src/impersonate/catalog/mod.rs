//! TLS fingerprint catalog.
//!
//! Static registry of per-browser-version `TlsFingerprint`s sourced from
//! curl-impersonate's MPL-2.0 signature yamls (vendored at
//! `references/curl-impersonate/tests/signatures/`) plus our own
//! locally-captured yamls under `src/impersonate/catalog/captured/`.
//!
//! Generation happens in `build.rs` at compile time — the output lives in
//! `$OUT_DIR/tls_catalog_generated.rs` and is `include!`-ed below. This
//! keeps the wire format (curl-impersonate yaml) the source of truth and
//! avoids duplicating fingerprint data in Rust source.

pub mod eras;

/// One byte position in a TLS list (cipher / extension / supported_group /
/// supported_version) that may be either a fixed value OR a GREASE marker.
/// GREASE positions are stable per profile, but BoringSSL randomises the
/// actual GREASE byte at runtime via `set_grease_enabled(true)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericEntry {
    Greased,
    Value(u16),
}

/// One extension slot in the ClientHello, in on-wire order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionEntry {
    Greased,
    Named { id: u16, name: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Browser {
    Chrome,
    Chromium,
    Firefox,
    Edge,
    Safari,
    Brave,
    Opera,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BrowserOs {
    Linux,
    Windows,
    MacOs,
    Android,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Channel {
    Stable,
    Beta,
    Dev,
    Canary,
    Esr,
}

/// A single browser+version+os fingerprint capturing every byte the JA3/JA4
/// generators care about. Fields mirror the curl-impersonate yaml schema.
#[derive(Debug, Clone, Copy)]
pub struct TlsFingerprint {
    pub name: &'static str,
    pub browser: Browser,
    pub browser_name: &'static str,
    pub major: u16,
    pub version: &'static str,
    pub os: BrowserOs,
    pub os_name: &'static str,

    pub record_version: u16,
    pub handshake_version: u16,
    pub session_id_length: u32,

    /// Cipher suite list including GREASE markers in their captured positions.
    pub ciphersuites: &'static [NumericEntry],
    pub comp_methods: &'static [u8],

    /// Extensions in on-wire order. Some browsers (Chrome 120+) randomise
    /// per connection — but we keep the captured order as the canonical
    /// "what the browser would emit if asked deterministically".
    pub extensions: &'static [ExtensionEntry],

    pub alpn: &'static [&'static str],
    pub alps_alpn: &'static [&'static str],
    pub sig_hash_algs: &'static [u16],
    pub supported_groups: &'static [NumericEntry],
    pub ec_point_formats: &'static [u8],
    pub supported_versions: &'static [NumericEntry],
    pub cert_compress_algs: &'static [u16],
    pub psk_ke_modes: &'static [u8],
    pub key_share_groups: &'static [NumericEntry],

    pub has_status_request: bool,
    pub has_extended_master_secret: bool,
    pub has_renegotiation_info: bool,
    pub has_session_ticket: bool,
    pub has_signed_certificate_timestamp: bool,
    pub has_padding: bool,
    pub has_ech_grease: bool,
}

impl TlsFingerprint {
    /// Cipher list with GREASE entries stripped (JA3/JA4 input form).
    pub fn ciphers_no_grease(&self) -> Vec<u16> {
        self.ciphersuites
            .iter()
            .filter_map(|e| match e {
                NumericEntry::Value(v) => Some(*v),
                NumericEntry::Greased => None,
            })
            .collect()
    }

    /// Extension IDs with GREASE entries stripped (JA3/JA4 input form).
    pub fn extension_ids_no_grease(&self) -> Vec<u16> {
        self.extensions
            .iter()
            .filter_map(|e| match e {
                ExtensionEntry::Named { id, .. } => Some(*id),
                ExtensionEntry::Greased => None,
            })
            .collect()
    }

    /// Supported groups with GREASE entries stripped.
    pub fn supported_groups_no_grease(&self) -> Vec<u16> {
        self.supported_groups
            .iter()
            .filter_map(|e| match e {
                NumericEntry::Value(v) => Some(*v),
                NumericEntry::Greased => None,
            })
            .collect()
    }

    /// Build the JA3 input string: "version,ciphers,extensions,groups,formats".
    /// Returns the canonical pre-MD5 string (consumers compute MD5 separately).
    pub fn ja3_string(&self) -> String {
        let join = |xs: &[u16]| {
            xs.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("-")
        };
        let ciphers = self.ciphers_no_grease();
        let exts = self.extension_ids_no_grease();
        let groups = self.supported_groups_no_grease();
        let fmts: Vec<u16> = self.ec_point_formats.iter().map(|n| *n as u16).collect();
        format!(
            "{},{},{},{},{}",
            self.handshake_version,
            join(&ciphers),
            join(&exts),
            join(&groups),
            join(&fmts)
        )
    }
}

// AUTO-GENERATED — included from $OUT_DIR by build.rs.
include!(concat!(env!("OUT_DIR"), "/tls_catalog_generated.rs"));

/// Look up a fingerprint by its curl-impersonate-style name
/// (`chrome_98.0.4758.102_win10` etc.). Returns None if unknown.
pub fn lookup(name: &str) -> Option<&'static TlsFingerprint> {
    CATALOG.iter().find(|(n, _)| *n == name).map(|(_, fp)| *fp)
}

/// All registered fingerprints. Useful for coverage tests.
pub fn all() -> impl Iterator<Item = &'static TlsFingerprint> {
    CATALOG.iter().map(|(_, fp)| *fp)
}

// ──────────────────────────────────────────────────────────────────────
// IANA → OpenSSL/BoringSSL name translation
// ──────────────────────────────────────────────────────────────────────
//
// `TlsFingerprint` stores ciphers / curves / sigalgs as u16 IANA codepoints
// (the canonical form for JA3/JA4). BoringSSL's `set_cipher_list`,
// `set_curves_list`, `set_sigalgs_list` take colon-joined OpenSSL names.
// These helpers bridge the two so the connector can accept any catalog
// entry without each call site needing its own translation table.

/// Map a TLS cipher suite ID to BoringSSL's OpenSSL-style name, returning
/// `None` for codepoints BoringSSL doesn't compile in (very old CBC suites,
/// experimental drafts).
pub fn cipher_id_to_openssl_name(id: u16) -> Option<&'static str> {
    Some(match id {
        // TLS 1.3 suites.
        0x1301 => "TLS_AES_128_GCM_SHA256",
        0x1302 => "TLS_AES_256_GCM_SHA384",
        0x1303 => "TLS_CHACHA20_POLY1305_SHA256",
        0x1304 => "TLS_AES_128_CCM_SHA256",
        0x1305 => "TLS_AES_128_CCM_8_SHA256",

        // TLS 1.2 ECDHE-ECDSA AEAD.
        0xc02b => "ECDHE-ECDSA-AES128-GCM-SHA256",
        0xc02c => "ECDHE-ECDSA-AES256-GCM-SHA384",
        0xcca9 => "ECDHE-ECDSA-CHACHA20-POLY1305",

        // TLS 1.2 ECDHE-RSA AEAD.
        0xc02f => "ECDHE-RSA-AES128-GCM-SHA256",
        0xc030 => "ECDHE-RSA-AES256-GCM-SHA384",
        0xcca8 => "ECDHE-RSA-CHACHA20-POLY1305",

        // TLS 1.2 ECDHE CBC (kept for older Chrome eras).
        0xc013 => "ECDHE-RSA-AES128-SHA",
        0xc014 => "ECDHE-RSA-AES256-SHA",
        0xc009 => "ECDHE-ECDSA-AES128-SHA",
        0xc00a => "ECDHE-ECDSA-AES256-SHA",

        // TLS 1.2 RSA AEAD / CBC (legacy fallbacks).
        0x009c => "AES128-GCM-SHA256",
        0x009d => "AES256-GCM-SHA384",
        0x002f => "AES128-SHA",
        0x0035 => "AES256-SHA",

        _ => return None,
    })
}

/// Map a TLS supported_groups ID (named curve) to BoringSSL's OpenSSL-style name.
pub fn group_id_to_openssl_name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0017 => "P-256",
        0x0018 => "P-384",
        0x0019 => "P-521",
        0x001d => "X25519",
        0x001e => "X448",
        0x6399 => "X25519Kyber768Draft00",
        0x11ec => "X25519MLKEM768",
        _ => return None,
    })
}

/// Map a TLS signature_algorithms IANA ID to BoringSSL's name.
pub fn sigalg_id_to_openssl_name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0401 => "rsa_pkcs1_sha256",
        0x0501 => "rsa_pkcs1_sha384",
        0x0601 => "rsa_pkcs1_sha512",
        0x0403 => "ecdsa_secp256r1_sha256",
        0x0503 => "ecdsa_secp384r1_sha384",
        0x0603 => "ecdsa_secp521r1_sha512",
        0x0804 => "rsa_pss_rsae_sha256",
        0x0805 => "rsa_pss_rsae_sha384",
        0x0806 => "rsa_pss_rsae_sha512",
        0x0807 => "ed25519",
        0x0808 => "ed448",
        0x0809 => "rsa_pss_pss_sha256",
        0x080a => "rsa_pss_pss_sha384",
        0x080b => "rsa_pss_pss_sha512",
        _ => return None,
    })
}

/// Render a colon-joined OpenSSL cipher list from the fingerprint's
/// captured cipher IDs (GREASE stripped, unknown IDs skipped with a
/// `tracing::warn`).
pub fn render_cipher_list(fp: &TlsFingerprint) -> String {
    fp.ciphers_no_grease()
        .into_iter()
        .filter_map(|id| match cipher_id_to_openssl_name(id) {
            Some(name) => Some(name),
            None => {
                tracing::warn!(
                    target: "crawlex::impersonate::catalog",
                    cipher_id = format!("0x{:04x}", id),
                    profile = fp.name,
                    "unknown cipher ID — dropping from BoringSSL list"
                );
                None
            }
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// Render a colon-joined supported_groups list (curves) from the catalog.
pub fn render_curves_list(fp: &TlsFingerprint) -> String {
    fp.supported_groups_no_grease()
        .into_iter()
        .filter_map(|id| match group_id_to_openssl_name(id) {
            Some(name) => Some(name),
            None => {
                tracing::warn!(
                    target: "crawlex::impersonate::catalog",
                    group_id = format!("0x{:04x}", id),
                    profile = fp.name,
                    "unknown supported_group ID — dropping from BoringSSL list"
                );
                None
            }
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// Render a colon-joined signature_algorithms list from the catalog.
pub fn render_sigalgs_list(fp: &TlsFingerprint) -> String {
    fp.sig_hash_algs
        .iter()
        .copied()
        .filter_map(|id| match sigalg_id_to_openssl_name(id) {
            Some(name) => Some(name),
            None => {
                tracing::warn!(
                    target: "crawlex::impersonate::catalog",
                    sigalg_id = format!("0x{:04x}", id),
                    profile = fp.name,
                    "unknown sigalg ID — dropping from BoringSSL list"
                );
                None
            }
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// Encode the ALPN extension's wire payload from a list of protocol names:
/// length-prefixed concatenation. Returns `b"\x02h2\x08http/1.1"` for the
/// canonical Chrome list `["h2", "http/1.1"]`.
pub fn encode_alpn_wire(alpn: &[&str]) -> Vec<u8> {
    let mut out = Vec::with_capacity(alpn.iter().map(|s| s.len() + 1).sum());
    for proto in alpn {
        if proto.len() > u8::MAX as usize {
            continue;
        }
        out.push(proto.len() as u8);
        out.extend_from_slice(proto.as_bytes());
    }
    out
}
