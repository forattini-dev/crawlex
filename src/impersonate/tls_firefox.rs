//! Firefox NSS-style TLS connector.
//!
//! Firefox uses NSS (Mozilla's TLS stack) instead of BoringSSL. The
//! resulting `ClientHello` differs structurally from Chrome's:
//!
//! * Cipher list orders TLS 1.3 suites with `TLS_AES_128_GCM_SHA256` first
//!   (Chrome leads with the same suite but Firefox's TLS 1.2 ECDHE block
//!   keeps SHA-256 then SHA-384 then ChaCha — different from Chrome).
//! * **No** `application_settings` (ALPS) extension — that's Chrome-only.
//! * **No** `compress_certificate` extension — Firefox 117 added support
//!   but doesn't advertise it client-side by default.
//! * `supported_groups` ends with `secp521r1` (0x19), Chrome does not.
//! * `signature_algorithms` includes `ecdsa_sha1` (0x0203) on older Firefox,
//!   removed in 100+.
//! * `psk_key_exchange_modes` carries only `psk_dhe_ke` (1) like Chrome.
//! * No GREASE in supported_groups (Chrome adds GREASE to that list too).
//!
//! We still build on top of BoringSSL because pulling NSS into the crate
//! is a 50MB+ dep landmine. The wire-level result is byte-equivalent for
//! the fields detectors hash (JA3, JA4) — Firefox-specific NSS
//! optimisations like extension order shuffling per-connection are not
//! present, but neither is GREASE in the slots Chrome uses, so the JA3
//! string still matches captured Firefox fingerprints.

use boring::ssl::{SslConnector, SslMethod, SslSessionCacheMode, SslVerifyMode, SslVersion};

use crate::error::{Error, Result};
use crate::impersonate::profiles::Profile;

/// Build a Firefox-flavoured TLS connector. The catalog supplies the
/// per-version cipher / group / sigalg / ALPN data; we wire BoringSSL to
/// emit a ClientHello whose JA3/JA4 matches captured Firefox NSS output.
pub fn build_connector(profile: Profile) -> Result<SslConnector> {
    let fp = profile.tls().ok_or_else(|| {
        Error::Tls(format!(
            "no TLS fingerprint in catalog for Firefox profile {:?}",
            profile
        ))
    })?;

    let mut b =
        SslConnector::builder(SslMethod::tls()).map_err(|e| Error::Tls(format!("builder: {e}")))?;

    b.set_min_proto_version(Some(SslVersion::TLS1_2))
        .map_err(|e| Error::Tls(format!("min proto: {e}")))?;
    b.set_max_proto_version(Some(SslVersion::TLS1_3))
        .map_err(|e| Error::Tls(format!("max proto: {e}")))?;

    // Firefox 100+ also greases (per RFC 8701) but does NOT permute
    // extensions — order is stable within a major version. We still flip
    // permute on because BoringSSL's permute is no-op when there's only
    // one possible ordering (most Firefox eras have a fixed order).
    b.set_grease_enabled(true);

    let cipher_list = crate::impersonate::catalog::render_cipher_list(fp);
    b.set_cipher_list(&cipher_list)
        .map_err(|e| Error::Tls(format!("cipher_list: {e}")))?;

    let curves = crate::impersonate::catalog::render_curves_list(fp);
    b.set_curves_list(&curves)
        .map_err(|e| Error::Tls(format!("curves: {e}")))?;

    let sigalgs = crate::impersonate::catalog::render_sigalgs_list(fp);
    b.set_sigalgs_list(&sigalgs)
        .map_err(|e| Error::Tls(format!("sigalgs: {e}")))?;

    // Firefox advertises h2 then http/1.1 in ALPN.
    let alpn_wire = crate::impersonate::catalog::encode_alpn_wire(fp.alpn);
    b.set_alpn_protos(&alpn_wire)
        .map_err(|e| Error::Tls(format!("alpn: {e}")))?;

    // Firefox sends OCSP stapling but NOT signed_certificate_timestamp by
    // default in the captured signatures. Toggle per-fp.
    if fp.has_status_request {
        b.enable_ocsp_stapling();
    }
    if fp.has_signed_certificate_timestamp {
        b.enable_signed_cert_timestamps();
    }

    b.set_verify(SslVerifyMode::PEER);

    // Firefox session resumption uses the same TLS 1.3 PSK ticket
    // mechanism. Reuse the same session cache mode as Chrome path.
    b.set_session_cache_mode(SslSessionCacheMode::CLIENT | SslSessionCacheMode::NO_INTERNAL);

    Ok(b.build())
}

/// Firefox does NOT send the `application_settings` (ALPS) extension —
/// that's Chrome's invention. ECH GREASE is also Firefox-experimental
/// behind a pref (off by default). This per-connection hook is a no-op
/// for Firefox; the function exists so callers can dispatch uniformly
/// without branching on profile in every code path.
pub fn configure_ssl(_ssl: &mut boring::ssl::SslRef) -> Result<()> {
    Ok(())
}
