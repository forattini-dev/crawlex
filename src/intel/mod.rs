//! Target-scoped infrastructure-intel orchestrator (Fase B).
//!
//! Given a "target" (a registrable domain), runs the passive-recon
//! stages that answer "what do I know about this slice of the
//! internet?" without ever crawling a single HTML page:
//!
//!   1. Subdomain enumeration (crt.sh CT logs today; multi-source
//!      planned).
//!   2. DNS records per domain (A/AAAA/CNAME/MX/TXT/NS/CAA via the
//!      existing `discovery::dns` resolver) + wildcard detection.
//!   3. WHOIS/RDAP for the target root (existing `discovery::whois`).
//!   4. Certificate intel per `(subdomain, 443)` via a real TLS
//!      handshake — same boringssl path the crawler uses.
//!
//! Results land in the normalized tables Fase A shipped:
//! `domains`, `dns_records`, `ip_addresses`, `domain_ips`, `certs`,
//! `cert_seen_on`, `whois_records`.
//!
//! The orchestrator is intentionally sequential at this phase — each
//! stage's output feeds the next and parallelism adds complexity the
//! schema doesn't yet amortise. Fase D will add bounded concurrency
//! once `crawlex intel` handles 100+ subdomain targets.

pub mod orchestrator;
pub mod report_html;

pub use orchestrator::{IntelReport, IntelStage, TargetIntelOrchestrator};
