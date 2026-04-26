//! `Discoverer` adapter for RDAP/WHOIS lookups.
//!
//! Wraps `crate::discovery::whois::lookup` and projects the
//! `Registration` struct into a typed `Whois` finding.

use async_trait::async_trait;

use crate::discovery::pipeline::{Discoverer, DiscoveryError};
use crate::discovery::types::{DiscoveryContext, Finding};
use crate::discovery::whois;

pub struct WhoisDiscoverer;

#[async_trait]
impl Discoverer for WhoisDiscoverer {
    fn name(&self) -> &'static str {
        "whois"
    }

    fn enabled(&self, ctx: &DiscoveryContext) -> bool {
        ctx.features.whois
    }

    async fn discover(&self, ctx: &DiscoveryContext) -> Result<Vec<Finding>, DiscoveryError> {
        let registration =
            whois::lookup(&ctx.http, &ctx.target)
                .await
                .map_err(|e| DiscoveryError::Backend {
                    name: "whois",
                    message: e.to_string(),
                })?;
        Ok(vec![Finding::Whois {
            registrar: registration.registrar,
            registrant_org: registration.registrant_org,
            created_at: registration.created,
            expires_at: registration.expires,
            nameservers: registration.name_servers,
        }])
    }
}
