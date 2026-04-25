//! Peer certificate extraction from BoringSSL TLS streams.
//!
//! During a successful handshake the server cert chain is available via
//! `SslRef::peer_certificate()`. We pull CN, SANs, issuer, validity window,
//! and a SHA-256 fingerprint. SANs feed the crawler frontier as new
//! subdomains; the fingerprint lets us cluster hosts by exact cert.

use boring::hash::MessageDigest;
use boring::ssl::SslRef;
use boring::x509::{GeneralNameRef, X509};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerCert {
    pub subject_cn: Option<String>,
    pub issuer_cn: Option<String>,
    pub sans: Vec<String>,
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    pub sha256: Option<String>,
}

pub fn extract(ssl: &SslRef) -> Option<PeerCert> {
    let cert: X509 = ssl.peer_certificate()?;
    let subject_cn = name_cn(cert.subject_name());
    let issuer_cn = name_cn(cert.issuer_name());

    let mut sans: Vec<String> = Vec::new();
    if let Some(stack) = cert.subject_alt_names() {
        for gn in stack.iter() {
            if let Some(dns) = general_name_dns(gn) {
                sans.push(dns.to_ascii_lowercase());
            }
        }
    }
    sans.sort();
    sans.dedup();

    let sha256 = cert
        .digest(MessageDigest::sha256())
        .ok()
        .map(|d| hex::encode(d.as_ref()));

    Some(PeerCert {
        subject_cn,
        issuer_cn,
        sans,
        not_before: Some(cert.not_before().to_string()),
        not_after: Some(cert.not_after().to_string()),
        sha256,
    })
}

fn name_cn(name: &boring::x509::X509NameRef) -> Option<String> {
    for entry in name.entries() {
        if entry.object().nid() == boring::nid::Nid::COMMONNAME {
            if let Ok(s) = entry.data().as_utf8() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn general_name_dns(gn: &GeneralNameRef) -> Option<&str> {
    gn.dnsname()
}
