//! `TelemetryStorage` — page metrics + passive vendor telemetry.
//!
//! Backends that don't aggregate telemetry inherit the no-ops; SQLite
//! writes `page_metrics` rows + `vendor_telemetry` for the P0-9 passive
//! signal pipeline.

use url::Url;

use crate::Result;

/// Per-page metrics + passive vendor telemetry sink.
#[async_trait::async_trait]
pub trait TelemetryStorage: Send + Sync {
    /// Persist per-page metrics (CPU, network, web vitals). Default no-op.
    async fn save_metrics(&self, _url: &Url, _metrics: &crate::metrics::PageMetrics) -> Result<()> {
        Ok(())
    }

    /// Persist a passive vendor-telemetry observation. Default no-op.
    async fn record_telemetry(
        &self,
        _telem: &crate::antibot::telemetry::VendorTelemetry,
    ) -> Result<()> {
        Ok(())
    }
}
