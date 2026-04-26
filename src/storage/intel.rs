//! `IntelStorage` — host-level intelligence outputs.
//!
//! The discovery pipeline produces per-host facts (DNS, TLS cert, RDAP
//! registration), per-page asset references, and per-page tech
//! fingerprints. This trait is the sink for those outputs.
//!
//! Default no-ops let backends that don't want to track intel
//! (memory-only test sinks) compile clean.

use crate::storage::HostFacts;
use crate::Result;

/// Discovery / intel outputs sink.
#[async_trait::async_trait]
pub trait IntelStorage: Send + Sync {
    /// Persist a host-level facts bundle (DNS + cert + RDAP). Default no-op.
    async fn save_host_facts(&self, _host: &str, _facts: &HostFacts) -> Result<()> {
        Ok(())
    }

    /// Persist a batch of classified `AssetRef`s extracted from a page.
    /// Default no-op.
    async fn save_asset_refs(
        &self,
        _refs: &[crate::discovery::asset_refs::AssetRef],
    ) -> Result<()> {
        Ok(())
    }

    /// Persist a per-page technology fingerprint report and update any
    /// backend-specific host/domain rollups. Default no-op.
    async fn save_tech_fingerprint(
        &self,
        _report: &crate::discovery::tech_fingerprint::TechFingerprintReport,
    ) -> Result<()> {
        Ok(())
    }
}
