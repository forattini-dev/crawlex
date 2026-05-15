//! Engine — owns the source registry and dispatches by Tier.
//!
//! Slice B1 of PRD forattini-dev/crawlex#25. `Engine::analyze_hot`
//! runs every registered Hot source over a `TargetContext` and folds
//! the emitted Detections into a `FingerprintReport`.

use std::sync::Arc;

use crate::fingerprint::detection::{Category, Detection, Tier};
use crate::fingerprint::report::{FingerprintReport, Tiers};
use crate::fingerprint::target::sources::{
    AltSvcSource, AntibotMarkerSource, BlockPatternSource, BodyMarkerSource, CookieSource,
    HeaderSource, JsonLdSource, LinkRelSource, MetaTagSource, PeerCertSource, ScriptSrcSource,
    Source, StatusPatternSource, TimingPatternSource, TlsServerHelloSource,
};
use crate::fingerprint::target::TargetContext;

pub struct Engine {
    hot: Vec<Arc<dyn Source>>,
    warm: Vec<Arc<dyn Source>>,
    cold: Vec<Arc<dyn Source>>,
}

impl Engine {
    /// Empty engine with no sources. Used in tests that need to assert
    /// "engine without source yields empty report".
    pub fn new() -> Self {
        Self {
            hot: Vec::new(),
            warm: Vec::new(),
            cold: Vec::new(),
        }
    }

    /// Built-in default — the sources shipped in this slice's
    /// tracer-bullet form. Later slices grow this list.
    pub fn with_defaults() -> Self {
        let mut e = Self::new();
        e.register(Arc::new(HeaderSource::new()));
        e.register(Arc::new(CookieSource::new()));
        e.register(Arc::new(BodyMarkerSource::new()));
        e.register(Arc::new(MetaTagSource::new()));
        e.register(Arc::new(JsonLdSource::new()));
        e.register(Arc::new(ScriptSrcSource::new()));
        e.register(Arc::new(LinkRelSource::new()));
        e.register(Arc::new(AltSvcSource::new()));
        e.register(Arc::new(StatusPatternSource::new()));
        e.register(Arc::new(TlsServerHelloSource::new()));
        e.register(Arc::new(PeerCertSource::new()));
        e.register(Arc::new(TimingPatternSource::new()));
        e.register(Arc::new(AntibotMarkerSource::new()));
        e.register(Arc::new(BlockPatternSource::new()));
        e
    }

    pub fn register(&mut self, source: Arc<dyn Source>) {
        match source.tier() {
            Tier::Hot => self.hot.push(source),
            Tier::Warm => self.warm.push(source),
            Tier::Cold => self.cold.push(source),
        }
    }

    pub fn hot_source_count(&self) -> usize {
        self.hot.len()
    }

    pub fn warm_source_count(&self) -> usize {
        self.warm.len()
    }

    pub fn cold_source_count(&self) -> usize {
        self.cold.len()
    }

    /// Run every Hot source against `ctx`, route emitted Detections
    /// into the right `FingerprintReport` slot.
    pub fn analyze_hot(&self, ctx: &TargetContext<'_>) -> FingerprintReport {
        let host = ctx.host_label();
        let mut report = FingerprintReport::new(host);
        report.tiers_run = Tiers::hot();
        for src in &self.hot {
            for d in src.analyze(ctx) {
                push_detection(&mut report, d);
            }
        }
        report
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::with_defaults()
    }
}

fn push_detection(report: &mut FingerprintReport, d: Detection) {
    match d.category {
        Category::Cdn => report.cdn.push(d),
        Category::Waf => report.waf.push(d),
        Category::Antibot => report.antibot.push(d),
        Category::Cms => report.cms.push(d),
        Category::Ecommerce => report.ecommerce.push(d),
        Category::Frontend => report.frontend.push(d),
        Category::Backend => report.backend.push(d),
        Category::WebServer => report.webserver.push(d),
        Category::ReverseProxyLb => report.proxy_lb.push(d),
        Category::Cache => report.cache.push(d),
        Category::Analytics => report.analytics.push(d),
        Category::TagManager => report.tag_manager.push(d),
        Category::AbTesting => report.ab_testing.push(d),
        Category::Auth => report.auth.push(d),
        Category::Payment => report.payment.push(d),
        Category::Chat => report.chat.push(d),
        Category::DnsHosting => report.dns_hosting.push(d),
        Category::CookiePattern => report.cookie_pattern.push(d),
        Category::Other => report.other.push(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_register_all_hot_sources() {
        // 9 Hot sources after B3: header, cookie, body_marker,
        // meta_tag, json_ld, script_src, link_rel, alt_svc,
        // status_pattern.
        let e = Engine::with_defaults();
        assert_eq!(e.hot_source_count(), 14);
    }

    #[test]
    fn empty_engine_has_no_sources() {
        let e = Engine::new();
        assert_eq!(e.hot_source_count(), 0);
        assert_eq!(e.warm_source_count(), 0);
        assert_eq!(e.cold_source_count(), 0);
    }
}
