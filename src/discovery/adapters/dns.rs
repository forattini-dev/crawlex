//! `Discoverer` adapter for DNS lookups.
//!
//! Wraps `crate::discovery::dns::lookup` and projects each record type
//! into a typed `Finding`. The original `DnsFacts` struct still exists
//! and is reachable via `dns::lookup` for callers that prefer the
//! aggregated form (e.g. tests, intel reporting).

use async_trait::async_trait;

use crate::discovery::dns;
use crate::discovery::pipeline::{Discoverer, DiscoveryError};
use crate::discovery::types::{DiscoveryContext, DnsKind, Finding, SubdomainSource};

pub struct DnsDiscoverer;

#[async_trait]
impl Discoverer for DnsDiscoverer {
    fn name(&self) -> &'static str {
        "dns"
    }

    fn enabled(&self, ctx: &DiscoveryContext) -> bool {
        ctx.features.dns
    }

    async fn discover(&self, ctx: &DiscoveryContext) -> Result<Vec<Finding>, DiscoveryError> {
        let host = ctx.effective_host();
        let facts = dns::lookup(host).await;
        let mut out = Vec::new();
        for ip in &facts.a {
            out.push(Finding::DnsRecord {
                host: host.into(),
                kind: DnsKind::A,
                value: ip.to_string(),
            });
        }
        for ip in &facts.aaaa {
            out.push(Finding::DnsRecord {
                host: host.into(),
                kind: DnsKind::Aaaa,
                value: ip.to_string(),
            });
        }
        for cname in &facts.cname {
            out.push(Finding::DnsRecord {
                host: host.into(),
                kind: DnsKind::Cname,
                value: cname.clone(),
            });
        }
        for mx in &facts.mx {
            out.push(Finding::DnsRecord {
                host: host.into(),
                kind: DnsKind::Mx,
                value: mx.clone(),
            });
        }
        for txt in &facts.txt {
            out.push(Finding::DnsRecord {
                host: host.into(),
                kind: DnsKind::Txt,
                value: txt.clone(),
            });
        }
        for ns in &facts.ns {
            out.push(Finding::DnsRecord {
                host: host.into(),
                kind: DnsKind::Ns,
                value: ns.clone(),
            });
        }
        for caa in &facts.caa {
            out.push(Finding::DnsRecord {
                host: host.into(),
                kind: DnsKind::Caa,
                value: caa.clone(),
            });
        }
        // Related hosts (CNAME targets, MX exchanges) get a second emission
        // as `Subdomain` so downstream subdomain enumeration sees them
        // without needing to re-scan the DNS results.
        for related in &facts.related_hosts {
            // Only if the related host is actually under the target's
            // registrable domain — emitting a foreign domain (e.g. an MX
            // pointing at outlook.com) as a subdomain is wrong.
            if host_under(related, &ctx.target) {
                out.push(Finding::Subdomain {
                    host: related.clone(),
                    source: SubdomainSource::Dns,
                });
            }
        }
        Ok(out)
    }
}

/// `host_under("api.stone.com.br", "stone.com.br")` → true.
/// Best-effort string suffix check; doesn't parse public-suffix list.
fn host_under(host: &str, target: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    let t = target.trim_end_matches('.').to_ascii_lowercase();
    h == t || h.ends_with(&format!(".{t}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_under_matches_subdomain() {
        assert!(host_under("api.stone.com.br", "stone.com.br"));
        assert!(host_under("stone.com.br", "stone.com.br"));
        assert!(host_under("a.b.c.stone.com.br", "stone.com.br"));
    }

    #[test]
    fn host_under_rejects_unrelated() {
        assert!(!host_under("stone.example.com", "stone.com.br"));
        assert!(!host_under("evilstone.com.br", "stone.com.br"));
    }
}
