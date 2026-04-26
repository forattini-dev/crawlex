//! `Discoverer` adapter for Wayback Machine CDX URL enumeration.
//!
//! Wraps `crate::discovery::wayback::wayback_urls`. Every URL the CDX
//! returns becomes a `Finding::Url` so the pipeline downstream can either
//! seed the frontier directly or harvest hosts for subdomain enumeration.

use async_trait::async_trait;

use crate::discovery::pipeline::{Discoverer, DiscoveryError};
use crate::discovery::types::{DiscoveryContext, Finding};
use crate::discovery::wayback;

pub struct WaybackDiscoverer;

#[async_trait]
impl Discoverer for WaybackDiscoverer {
    fn name(&self) -> &'static str {
        "wayback"
    }

    fn enabled(&self, ctx: &DiscoveryContext) -> bool {
        ctx.features.wayback
    }

    async fn discover(&self, ctx: &DiscoveryContext) -> Result<Vec<Finding>, DiscoveryError> {
        let urls = wayback::wayback_urls(&ctx.http, &ctx.target)
            .await
            .map_err(|e| DiscoveryError::Backend {
                name: "wayback",
                message: e.to_string(),
            })?;
        Ok(urls.into_iter().map(Finding::Url).collect())
    }
}
