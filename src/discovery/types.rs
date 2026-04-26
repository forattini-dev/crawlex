//! Shared types for the host-level discovery pipeline.
//!
//! `DiscoveryContext` is the read-only handle each `Discoverer` receives —
//! target identity, shared HTTP client, per-module budget, cancellation
//! signal. `Finding` is the typed result each discoverer emits; the
//! `DiscoveryPipeline` fans them out for downstream consumers (frontier
//! enqueue, storage write, telemetry counter).
//!
//! Why typed instead of `serde_json::Value` payloads: callers downstream
//! (frontier, storage::IntelStorage) match on variants, so a fat enum gives
//! us exhaustive-match enforcement at compile time. New finding type =
//! one new variant + matchers see it everywhere.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use url::Url;

use crate::discovery::cert::PeerCert;
use crate::impersonate::ImpersonateClient;

/// A typed result emitted by a `Discoverer`. Variants intentionally cover
/// the cross-module union of what the existing free-functions return today;
/// new variants land here when we add discoverers, not in random tuples.
#[derive(Debug, Clone)]
pub enum Finding {
    /// A URL to seed into the frontier for crawling. Most discoverers
    /// (sitemap, wayback, robots_paths, well_known, pwa) emit these.
    Url(Url),

    /// A subdomain of the target's registrable domain, harvested via DNS
    /// CNAME, certificate transparency (crt.sh), or wayback archive.
    Subdomain {
        host: String,
        source: SubdomainSource,
    },

    /// One DNS record for the target — A, AAAA, CNAME, MX, TXT, NS, CAA.
    /// Pipeline downstream materialises these into the storage backend's
    /// `dns_records` table or emits them as events.
    DnsRecord {
        host: String,
        kind: DnsKind,
        value: String,
    },

    /// TLS peer certificate observed on the target's HTTPS endpoint.
    PeerCert(PeerCert),

    /// Open TCP port observed during the network probe stage.
    OpenPort {
        ip: IpAddr,
        port: u16,
        banner: Option<String>,
        service: Option<String>,
    },

    /// WHOIS registration details for the target's registrable domain.
    Whois {
        registrar: Option<String>,
        registrant_org: Option<String>,
        created_at: Option<String>,
        expires_at: Option<String>,
        nameservers: Vec<String>,
    },

    /// security.txt directive parsed from `/.well-known/security.txt`.
    /// `key` is the directive name (Contact, Encryption, Policy, …),
    /// `value` is the right-hand side.
    SecurityTxt { key: String, value: String },

    /// Generic key/value fact when a discoverer surfaces something that
    /// doesn't fit the typed variants above (used sparingly — promote to
    /// a typed variant when a second discoverer needs it).
    Fact {
        key: String,
        value: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubdomainSource {
    Dns,
    Cert,
    CrtSh,
    Wayback,
    HtmlBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsKind {
    A,
    Aaaa,
    Cname,
    Mx,
    Txt,
    Ns,
    Caa,
}

/// Read-only handle every `Discoverer` receives. Carries target identity,
/// shared HTTP client, per-module deadline (so a slow DNS resolver doesn't
/// stall the rest of the pipeline), and feature opt-ins.
///
/// Discoverers are expected to honour `budget` — a discoverer that wants
/// 30 s of network IO must wrap its work in `tokio::time::timeout(self.budget, ...)`.
/// The pipeline ALSO wraps each call in the same budget as defence in depth.
#[derive(Clone)]
pub struct DiscoveryContext {
    /// Registrable domain we're investigating (e.g. `stone.com.br`). Most
    /// discoverers query this directly; subdomain enumeration walks out
    /// from here.
    pub target: String,

    /// Optional explicit host (subdomain) the discoverer should focus on
    /// when set. When `None`, fall back to `target`.
    pub host: Option<String>,

    /// Shared HTTP client for discoverers that need to fetch
    /// (sitemap, wayback, well_known, pwa, security_txt, robots, whois).
    pub http: Arc<ImpersonateClient>,

    /// Per-module wall-clock budget. Pipeline enforces via
    /// `tokio::time::timeout`; discoverers that respect their own deadline
    /// short-circuit gracefully when partial results are still useful.
    pub budget: Duration,

    /// Set of feature toggles a caller can flip on/off. The pipeline reads
    /// these to decide whether to invoke optional discoverers (e.g. crt.sh
    /// is opt-in via `--crtsh`, peer cert via `--peer-cert`, etc.).
    pub features: DiscoveryFeatures,
}

impl DiscoveryContext {
    /// Effective host for the current discoverer call: explicit `host`
    /// override, falling back to `target`.
    pub fn effective_host(&self) -> &str {
        self.host.as_deref().unwrap_or(&self.target)
    }
}

/// Per-feature opt-ins. Mirrors the existing CLI flags
/// (`--crtsh`, `--dns`, `--wayback`, `--peer-cert`) so the pipeline can
/// route around modules the operator hasn't enabled.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscoveryFeatures {
    pub crtsh: bool,
    pub dns: bool,
    pub wayback: bool,
    pub peer_cert: bool,
    pub network_probe: bool,
    pub well_known: bool,
    pub robots_paths: bool,
    pub pwa: bool,
    pub security_txt: bool,
    pub favicon: bool,
    pub whois: bool,
}
