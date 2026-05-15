//! H2Settings source — Warm tier (B8).
//!
//! Computes an Akamai-style fingerprint from the h2 SETTINGS frame
//! parameters observed on the connection. Today the TargetContext
//! does not carry h2 SETTINGS data; the source is registered with
//! empty `analyze` so the slot is ready when B14 plumbs the frame
//! through.

use crate::fingerprint::detection::{Detection, Tier};
use crate::fingerprint::target::TargetContext;

use super::Source;

#[derive(Default)]
pub struct H2SettingsSource;

impl H2SettingsSource {
    pub fn new() -> Self {
        Self
    }
}

impl Source for H2SettingsSource {
    fn name(&self) -> &'static str {
        "h2_settings"
    }

    fn tier(&self) -> Tier {
        Tier::Warm
    }

    fn analyze(&self, _ctx: &TargetContext<'_>) -> Vec<Detection> {
        // TargetContext does not yet expose h2 SETTINGS frame data.
        // Registered so B14 wiring is a single-file change.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use url::Url;

    #[test]
    fn empty_until_target_context_widens() {
        let h = HeaderMap::new();
        let u: Url = "https://example.com/".parse().unwrap();
        let ctx = TargetContext::http_only(&u, 200, &h, b"");
        assert!(H2SettingsSource::new().analyze(&ctx).is_empty());
    }
}
