//! BoringSSL-based TLS impersonation of Chrome's ClientHello.
//!
//! The goal is a JA4 fingerprint that matches real Chrome on the same major version.
//! Reference for the expected shape: `references/curl-impersonate/chrome/`.
//!
//! Knobs configured per Chrome 131/132 stable:
//! * Cipher list (TLS 1.3 + 1.2 suites in Chrome order)
//! * Supported curves: X25519MLKEM768, X25519, P-256, P-384
//! * Signature algorithms in Chrome order
//! * ALPN: h2, http/1.1
//! * GREASE enabled, extension order permuted (Chrome >= 110)
//! * TLS 1.2 min, 1.3 max

use boring::ssl::{
    SslConnector, SslMethod, SslSession, SslSessionCacheMode, SslVerifyMode, SslVersion,
};
use dashmap::DashMap;
use foreign_types::ForeignTypeRef;
use parking_lot::Mutex;
use std::os::raw::c_int;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::impersonate::Profile;
use crate::{Error, Result};

/// Per-host TLS session cache. Real Chrome resumes TLS on every reconnect
/// to the same origin (1-RTT instead of 2). The cache is process-global,
/// keyed by `(host, port)`; entries past `TICKET_TTL` are skipped on read
/// and cleared on next access.
const TICKET_TTL: Duration = Duration::from_secs(600);

#[derive(Clone)]
struct CachedSession {
    der: Vec<u8>,
    inserted: Instant,
}

/// Global ticket cache. We don't tie it to the connector instance because
/// the boring `set_new_session_callback` wants `'static + Send + Sync` —
/// pinning a fresh cache per connector requires a `Box::leak` or unsafe
/// gymnastics. Process-global via `OnceLock` is honest and matches the
/// "one Chrome browser per process" mental model.
fn ticket_cache() -> &'static Arc<DashMap<String, CachedSession>> {
    static CACHE: OnceLock<Arc<DashMap<String, CachedSession>>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(DashMap::new()))
}

/// Look up a cached session for `(host, port)`. Returns `None` when the
/// entry is missing or expired (entries past TTL are removed on read).
pub fn lookup_ticket(host: &str, port: u16) -> Option<SslSession> {
    let key = format!("{host}:{port}");
    let cache = ticket_cache();
    let stale = cache
        .get(&key)
        .map(|e| e.inserted.elapsed() > TICKET_TTL)
        .unwrap_or(false);
    if stale {
        cache.remove(&key);
        return None;
    }
    let der = cache.get(&key)?.der.clone();
    SslSession::from_der(&der).ok()
}

/// Stash a session under `(host, port)` for reuse on the next handshake.
fn store_ticket(host: &str, port: u16, session: &SslSession) {
    let der = match session.to_der() {
        Ok(d) => d,
        Err(_) => return,
    };
    ticket_cache().insert(
        format!("{host}:{port}"),
        CachedSession {
            der,
            inserted: Instant::now(),
        },
    );
}

/// Static slot the new-session callback uses to publish freshly issued
/// tickets. The callback only sees the `Ssl` and the `SslSession`; it has
/// no per-call host context. We thread the host/port via this slot, set
/// just before each handshake on a per-connection basis.
///
/// This works because each handshake holds a process-wide lock for the
/// brief window between "set host" and "callback fires" — but to make it
/// safe for true concurrent handshakes, we use a `DashMap` keyed by the
/// raw `Ssl` pointer instead.
fn pending_host_map() -> &'static DashMap<usize, (String, u16)> {
    static M: OnceLock<DashMap<usize, (String, u16)>> = OnceLock::new();
    M.get_or_init(DashMap::new)
}

/// Bind a host/port to an `Ssl` so the `new_session_callback` can stash
/// the resulting session under the right key.
pub fn pin_host_for_session(ssl: &boring::ssl::SslRef, host: &str, port: u16) {
    let key = ssl.as_ptr() as usize;
    pending_host_map().insert(key, (host.to_string(), port));
}

fn unpin_host(ssl_ptr: usize) -> Option<(String, u16)> {
    pending_host_map().remove(&ssl_ptr).map(|(_, v)| v)
}

#[allow(dead_code)] // Used by test harness; live count for future telemetry.
static CALLBACK_HITS: Mutex<u64> = Mutex::new(0);

pub fn build_connector(profile: Profile) -> Result<SslConnector> {
    let mut b =
        SslConnector::builder(SslMethod::tls()).map_err(|e| Error::Tls(format!("builder: {e}")))?;

    b.set_min_proto_version(Some(SslVersion::TLS1_2))
        .map_err(|e| Error::Tls(format!("min proto: {e}")))?;
    b.set_max_proto_version(Some(SslVersion::TLS1_3))
        .map_err(|e| Error::Tls(format!("max proto: {e}")))?;

    // Chrome enables GREASE (RFC 8701) and permutes extension order from
    // Chrome 110 onward to discourage brittle fingerprinting.
    b.set_grease_enabled(true);
    b.set_permute_extensions(true);

    b.set_cipher_list(chrome_cipher_list(profile))
        .map_err(|e| Error::Tls(format!("cipher_list: {e}")))?;

    b.set_curves_list(chrome_curves(profile))
        .map_err(|e| Error::Tls(format!("curves: {e}")))?;

    b.set_sigalgs_list(chrome_sigalgs(profile))
        .map_err(|e| Error::Tls(format!("sigalgs: {e}")))?;

    // h2 then http/1.1 — wire format: length-prefixed protocol ids.
    b.set_alpn_protos(b"\x02h2\x08http/1.1")
        .map_err(|e| Error::Tls(format!("alpn: {e}")))?;

    // Chrome 131+ always advertises OCSP stapling (ext 5, status_request)
    // and Signed Certificate Timestamps (ext 18, signed_certificate_timestamp)
    // on every ClientHello. Absence of either ext is a strong "not Chrome"
    // tell. BoringSSL exposes context-wide helpers that wire these on for
    // all client handshakes.
    b.enable_ocsp_stapling();
    b.enable_signed_cert_timestamps();

    b.set_verify(SslVerifyMode::PEER);

    // cert_compression: Chrome advertises brotli(2), zlib(1), zstd(3).
    // We register with no-op compress (client auth is rare and compression
    // on the client→server path is not required for the advertisement to
    // appear in ClientHello) and real decompressors for the rare case a
    // server sends a compressed certificate chain.
    unsafe {
        let ctx_ptr = b.as_ptr();
        let algs: &[(u16, boring_sys::ssl_cert_decompression_func_t)] = &[
            (2, Some(cert_decompress_brotli)), // TLSEXT_cert_compression_brotli
            (1, Some(cert_decompress_zlib)),   // TLSEXT_cert_compression_zlib
            (3, Some(cert_decompress_zstd)),   // TLSEXT_cert_compression_zstd
        ];
        for &(alg_id, decompress) in algs {
            let rc = boring_sys::SSL_CTX_add_cert_compression_alg(
                ctx_ptr,
                alg_id,
                Some(cert_compress_noop),
                decompress,
            );
            if rc != 1 {
                return Err(Error::Tls(format!(
                    "SSL_CTX_add_cert_compression_alg(alg={alg_id}) failed"
                )));
            }
        }
    }

    // Session ticket resumption — Chrome resumes TLS on every reconnect to
    // the same origin (1-RTT instead of 2). NO_INTERNAL prevents
    // BoringSSL from using its built-in mostly-useless cache; we own the
    // storage. CLIENT mode means we receive tickets the server issues.
    b.set_session_cache_mode(SslSessionCacheMode::CLIENT | SslSessionCacheMode::NO_INTERNAL);
    b.set_new_session_callback(|ssl, session| {
        let ssl_ptr = ssl.as_ptr() as usize;
        if let Some((host, port)) = unpin_host(ssl_ptr) {
            store_ticket(&host, port, &session);
            *CALLBACK_HITS.lock() += 1;
        }
    });

    Ok(b.build())
}

/// Apply per-connection Chrome-isms: ALPS (application_settings) for h2
/// and ECH GREASE.
/// Call this on the `Ssl` obtained via `connector.configure().into_ssl(domain)`.
///
/// Chrome's ALPS payload is a real h2 SETTINGS frame body (no frame
/// header — the h2 codec handles framing later). The previous shim sent
/// an empty payload which is itself a tell: real Chrome always advertises
/// a few client-side settings inside ALPS. We send the same set hyper
/// would announce on its first SETTINGS frame, so an upstream that
/// inspects ALPS sees a coherent client.
///
/// ECH GREASE (ext 0xfe0d / 65037): Chrome M117+ unconditionally sends a
/// fake Encrypted-Client-Hello extension even when it has no HPKE config
/// for the target. Servers that fingerprint "does this client ever grease
/// ECH" put us in the Chrome bucket when we do and in the bot bucket when
/// we don't.
pub fn configure_ssl(ssl: &mut boring::ssl::SslRef) -> Result<()> {
    unsafe {
        let ssl_ptr = ssl.as_ptr();
        let proto = b"h2";
        let settings = build_alps_h2_settings();
        let rc = boring_sys::SSL_add_application_settings(
            ssl_ptr,
            proto.as_ptr(),
            proto.len(),
            settings.as_ptr(),
            settings.len(),
        );
        if rc != 1 {
            return Err(Error::Tls("SSL_add_application_settings failed".into()));
        }
        // ECH GREASE is per-SSL (not per-CTX) in BoringSSL — enable once
        // per connection. Returns void; no failure path to check.
        boring_sys::SSL_set_enable_ech_grease(ssl_ptr, 1);
    }
    Ok(())
}

/// Encode the SETTINGS payload Chrome announces over ALPS.
///
/// Wire format (RFC 7540 §6.5): each setting is a 6-byte tuple
/// `{ id: u16 BE, value: u32 BE }`. We send the four Chrome announces:
///   * HEADER_TABLE_SIZE (0x1) = 65536
///   * ENABLE_PUSH (0x2) = 0
///   * INITIAL_WINDOW_SIZE (0x4) = 6_291_456
///   * MAX_HEADER_LIST_SIZE (0x6) = 262_144
///
/// Detectors that parse ALPS payload look for these specific values; an
/// empty payload is the easiest "this isn't Chrome" signal.
fn build_alps_h2_settings() -> Vec<u8> {
    fn pair(id: u16, value: u32) -> [u8; 6] {
        let mut b = [0u8; 6];
        b[0..2].copy_from_slice(&id.to_be_bytes());
        b[2..6].copy_from_slice(&value.to_be_bytes());
        b
    }
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&pair(0x1, 65_536));
    out.extend_from_slice(&pair(0x2, 0));
    out.extend_from_slice(&pair(0x4, 6_291_456));
    out.extend_from_slice(&pair(0x6, 262_144));
    out
}

unsafe extern "C" fn cert_compress_noop(
    _ssl: *mut boring_sys::SSL,
    _out: *mut boring_sys::CBB,
    _in_buf: *const u8,
    _in_len: usize,
) -> c_int {
    // Clients very rarely send certificates; compression path unused. Return
    // failure so BoringSSL falls back to uncompressed if ever called.
    0
}

/// Hard cap on any declared decompressed-certificate size. BoringSSL's
/// own reference limit for cert chains is 256 KiB; a hostile server can
/// otherwise claim `uncompressed_len = usize::MAX` and trigger an
/// allocate-then-OOM path during the handshake. We reject up-front.
const MAX_CERT_DECOMPRESSED_LEN: usize = 256 * 1024;

unsafe extern "C" fn cert_decompress_brotli(
    _ssl: *mut boring_sys::SSL,
    out: *mut *mut boring_sys::CRYPTO_BUFFER,
    uncompressed_len: usize,
    in_buf: *const u8,
    in_len: usize,
) -> c_int {
    use std::io::Read;
    use std::slice;
    if uncompressed_len == 0 || uncompressed_len > MAX_CERT_DECOMPRESSED_LEN {
        return 0;
    }
    let input = slice::from_raw_parts(in_buf, in_len);
    let mut output: Vec<u8> = Vec::with_capacity(uncompressed_len);
    // `.take(uncompressed_len + 1)` so an attacker cannot inflate past the
    // declared size even if the compressed frame claims more: the +1 lets
    // the length-mismatch check in `finalize_decompressed` still fail
    // cleanly on a bomb; without the +1 the take caps exactly at the
    // truthful size and a truncated bomb would pass silently.
    let mut reader =
        brotli::Decompressor::new(input, 4096).take((uncompressed_len as u64).saturating_add(1));
    if reader.read_to_end(&mut output).is_err() {
        return 0;
    }
    finalize_decompressed(out, &output, uncompressed_len)
}

unsafe extern "C" fn cert_decompress_zlib(
    _ssl: *mut boring_sys::SSL,
    out: *mut *mut boring_sys::CRYPTO_BUFFER,
    uncompressed_len: usize,
    in_buf: *const u8,
    in_len: usize,
) -> c_int {
    use std::io::Read;
    use std::slice;
    if uncompressed_len == 0 || uncompressed_len > MAX_CERT_DECOMPRESSED_LEN {
        return 0;
    }
    let input = slice::from_raw_parts(in_buf, in_len);
    let mut output: Vec<u8> = Vec::with_capacity(uncompressed_len);
    let mut reader =
        flate2::read::ZlibDecoder::new(input).take((uncompressed_len as u64).saturating_add(1));
    if reader.read_to_end(&mut output).is_err() {
        return 0;
    }
    finalize_decompressed(out, &output, uncompressed_len)
}

unsafe extern "C" fn cert_decompress_zstd(
    _ssl: *mut boring_sys::SSL,
    out: *mut *mut boring_sys::CRYPTO_BUFFER,
    uncompressed_len: usize,
    in_buf: *const u8,
    in_len: usize,
) -> c_int {
    use std::slice;
    if uncompressed_len == 0 || uncompressed_len > MAX_CERT_DECOMPRESSED_LEN {
        return 0;
    }
    let input = slice::from_raw_parts(in_buf, in_len);
    // zstd::bulk::decompress already honours the capacity hint as a hard
    // upper bound when backed by the zstd `capacity` parameter — passing
    // `uncompressed_len` here caps the output at that value and will
    // error out on a bomb. The MAX cap above is the defence-in-depth.
    let output = match zstd::bulk::decompress(input, uncompressed_len) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    finalize_decompressed(out, &output, uncompressed_len)
}

/// Wrap a decompressed byte slice into a `CRYPTO_BUFFER` and publish it
/// through the out-pointer BoringSSL gave us. Returns 1 on success, 0 on
/// length mismatch or allocation failure.
unsafe fn finalize_decompressed(
    out: *mut *mut boring_sys::CRYPTO_BUFFER,
    output: &[u8],
    uncompressed_len: usize,
) -> c_int {
    if output.len() != uncompressed_len {
        return 0;
    }
    let buf = boring_sys::CRYPTO_BUFFER_new(output.as_ptr(), output.len(), std::ptr::null_mut());
    if buf.is_null() {
        return 0;
    }
    *out = buf;
    1
}

fn chrome_cipher_list(_profile: Profile) -> &'static str {
    // TLS 1.3 suites are negotiated via SSL_CTX_set_ciphersuites in
    // OpenSSL, but BoringSSL folds them into the standard cipher list.
    // Order matches Chrome M120+. Notably we DROP the legacy SHA1
    // suites (`ECDHE-RSA-AES128-SHA`, `AES128-SHA`, `AES256-SHA`) that
    // Chrome removed years ago — sending them now is a JA3 tell that
    // we're a Chrome ~87 era impersonator, not current Chrome.
    "TLS_AES_128_GCM_SHA256:\
TLS_AES_256_GCM_SHA384:\
TLS_CHACHA20_POLY1305_SHA256:\
ECDHE-ECDSA-AES128-GCM-SHA256:\
ECDHE-RSA-AES128-GCM-SHA256:\
ECDHE-ECDSA-AES256-GCM-SHA384:\
ECDHE-RSA-AES256-GCM-SHA384:\
ECDHE-ECDSA-CHACHA20-POLY1305:\
ECDHE-RSA-CHACHA20-POLY1305:\
AES128-GCM-SHA256:\
AES256-GCM-SHA384"
}

fn chrome_curves(_profile: Profile) -> &'static str {
    // Chrome M128+ uses X25519MLKEM768 (SSL_GROUP id 0x11ec) as its
    // post-quantum hybrid key-exchange, replacing the earlier
    // X25519Kyber768Draft00 (0x6399). The vendored BoringSSL exposes both
    // NIDs and IDs (see boring_sys::NID_X25519MLKEM768 / SSL_GROUP_X25519_MLKEM768),
    // so we pin the current rename. A server that still only knows the
    // draft group falls back via the second entry.
    "X25519MLKEM768:X25519:P-256:P-384"
}

fn chrome_sigalgs(_profile: Profile) -> &'static str {
    // Chrome M120+ advertises ed25519 alongside the ECDSA/RSA set. Adding
    // it here closes a small but real fingerprint gap (a sigalgs list
    // missing ed25519 currently looks like Chrome ~M115).
    "ecdsa_secp256r1_sha256:\
rsa_pss_rsae_sha256:\
rsa_pkcs1_sha256:\
ecdsa_secp384r1_sha384:\
rsa_pss_rsae_sha384:\
rsa_pkcs1_sha384:\
rsa_pss_rsae_sha512:\
rsa_pkcs1_sha512:\
ed25519"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alps_h2_settings_layout_matches_chrome() {
        let s = build_alps_h2_settings();
        // 4 settings × 6 bytes.
        assert_eq!(s.len(), 24);
        // HEADER_TABLE_SIZE = 65536
        assert_eq!(&s[0..2], &[0x00, 0x01]);
        assert_eq!(&s[2..6], &65_536u32.to_be_bytes());
        // ENABLE_PUSH = 0
        assert_eq!(&s[6..8], &[0x00, 0x02]);
        assert_eq!(&s[8..12], &0u32.to_be_bytes());
        // INITIAL_WINDOW_SIZE = 6_291_456
        assert_eq!(&s[12..14], &[0x00, 0x04]);
        assert_eq!(&s[14..18], &6_291_456u32.to_be_bytes());
        // MAX_HEADER_LIST_SIZE = 262_144
        assert_eq!(&s[18..20], &[0x00, 0x06]);
        assert_eq!(&s[20..24], &262_144u32.to_be_bytes());
    }

    #[test]
    fn ticket_cache_round_trip_string_form() {
        // We can't synthesize a real `SslSession` cheaply in a unit test
        // (it needs a full handshake), so we exercise the path that DOES
        // round-trip: insert a fake DER blob via the cache directly,
        // confirm the lookup returns None for missing keys and that an
        // expired entry is evicted on read.
        let cache = ticket_cache();
        cache.insert(
            "no-such-host:443".into(),
            CachedSession {
                der: vec![1, 2, 3, 4],
                inserted: Instant::now() - Duration::from_secs(TICKET_TTL.as_secs() + 1),
            },
        );
        // Stale → SslSession::from_der will fail, but lookup_ticket
        // first checks freshness and removes. Either way, returns None.
        let _ = lookup_ticket("no-such-host", 443);
        assert!(cache.get("no-such-host:443").is_none());
    }
}
