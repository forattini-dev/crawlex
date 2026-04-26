//! `Discoverer` adapter for crt.sh certificate-transparency subdomain enum.
//!
//! Wraps `crate::discovery::subdomains::crtsh_subdomains` and projects each
//! returned hostname into a `Finding::Subdomain`.

use async_trait::async_trait;

use crate::discovery::pipeline::{Discoverer, DiscoveryError};
use crate::discovery::subdomains;
use crate::discovery::types::{DiscoveryContext, Finding, SubdomainSource};

pub struct CrtShDiscoverer;

#[async_trait]
impl Discoverer for CrtShDiscoverer {
    fn name(&self) -> &'static str {
        "crtsh"
    }

    fn enabled(&self, ctx: &DiscoveryContext) -> bool {
        ctx.features.crtsh
    }

    async fn discover(&self, ctx: &DiscoveryContext) -> Result<Vec<Finding>, DiscoveryError> {
        let hosts = subdomains::crtsh_subdomains(&ctx.http, &ctx.target)
            .await
            .map_err(|e| DiscoveryError::Backend {
                name: "crtsh",
                message: e.to_string(),
            })?;
        Ok(hosts
            .into_iter()
            .map(|host| Finding::Subdomain {
                host,
                source: SubdomainSource::CrtSh,
            })
            .collect())
    }
}
