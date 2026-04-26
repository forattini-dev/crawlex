//! Per-topic `Discoverer` adapters.
//!
//! Each adapter wraps an existing free function in `src/discovery/<topic>.rs`,
//! turning it into a `Discoverer` that participates in the
//! `DiscoveryPipeline`. The free functions stay unchanged — adapters are
//! pure plumbing (read context, call the function, project the return into
//! `Finding`s, respect the budget).
//!
//! Adding a new topic = new file here + one entry in
//! `default_pipeline()` below.

use std::time::Duration;

use crate::discovery::pipeline::DiscoveryPipeline;
use crate::discovery::types::DiscoveryFeatures;

mod crtsh;
mod dns;
mod wayback;
mod whois;

pub use crtsh::CrtShDiscoverer;
pub use dns::DnsDiscoverer;
pub use wayback::WaybackDiscoverer;
pub use whois::WhoisDiscoverer;

/// Build the canonical host-level recon pipeline order:
///   1. **DNS** — resolve A/AAAA/CNAME/MX/TXT/NS/CAA. Cheap, always first
///      so subsequent discoverers can use the resolved IPs.
///   2. **WHOIS** — RDAP lookup for registrar + nameservers.
///   3. **CrtSh** — certificate-transparency subdomain enum. Slow, opt-in.
///   4. **Wayback** — CDX history URLs. Slow, opt-in.
///
/// Per-page extractors (links, asset_refs, sitemap parser, well-known,
/// security.txt, pwa, favicon, js_endpoints, tech_fingerprint) stay as
/// free functions in `src/discovery/*.rs` and are invoked by `crawler.rs`
/// during the per-URL fetch loop. They don't fit the host-level
/// `Discoverer` trait — wrapping them would just add ceremony.
pub fn default_pipeline() -> DiscoveryPipeline {
    DiscoveryPipeline::with_discoverers(vec![
        Box::new(DnsDiscoverer),
        Box::new(WhoisDiscoverer),
        Box::new(CrtShDiscoverer),
        Box::new(WaybackDiscoverer),
    ])
}

/// Convenience: per-module budget that matches the old hard-coded
/// behaviour (30 s — same as `Pipeline::DEFAULT_BUDGET`). Exposed so
/// callers tuning a custom pipeline pin the same number.
pub const DEFAULT_BUDGET: Duration = DiscoveryPipeline::DEFAULT_BUDGET;

/// Default feature set when the operator hasn't asked for opt-in
/// modules. Matches the historical `--no-pwa` / `--no-favicon` /
/// `--no-well-known` defaults (i.e. those are ON unless the operator
/// disables them); `--crtsh`, `--dns`, `--wayback`, `--peer-cert`,
/// `--whois` and `--network-probe` are OFF unless flagged.
pub fn default_features() -> DiscoveryFeatures {
    DiscoveryFeatures {
        crtsh: false,
        dns: false,
        wayback: false,
        peer_cert: false,
        network_probe: false,
        well_known: true,
        robots_paths: true,
        pwa: true,
        security_txt: true,
        favicon: true,
        whois: false,
    }
}
