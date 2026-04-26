pub mod asset_refs;
pub mod assets;
pub mod cert;
pub mod dns;
pub mod favicon;
pub mod graph;
pub mod js_endpoints;
pub mod links;
pub mod network_probe;
pub mod pipeline;
pub mod pwa;
pub mod robots_paths;
pub mod security_txt;
pub mod sitemap;
pub mod subdomains;
pub mod tech_fingerprint;
pub mod types;
pub mod wayback;
pub mod well_known;
pub mod whois;

// Adapter modules — one Discoverer adapter per topic. Free functions in
// the topic modules above stay where they are; the adapters wrap them.
pub mod adapters;

pub use assets::{classify_url, classify_with_mime, AssetKind};
pub use graph::DiscoveryGraph;
pub use links::extract_links;
pub use pipeline::{Discoverer, DiscoveryError, DiscoveryPipeline};
pub use types::{DiscoveryContext, DiscoveryFeatures, DnsKind, Finding, SubdomainSource};
